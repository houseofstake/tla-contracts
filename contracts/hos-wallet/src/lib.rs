mod error;
mod events;
mod state;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use defuse_wallet::actions::{FunctionCall, NearAction};
use defuse_wallet::contract::Wallet;
use defuse_wallet::events::{Actor, WalletEvent};
use defuse_wallet::{
    ContractError, NearPromise, Request, RequestMessage, State, Timestamp, WalletOp, STATE_KEY,
};
use defuse_wallet_no_sign::NoPublicKey;
use near_sdk::borsh::BorshDeserialize;
use near_sdk::json_types::{U128, U64};
use near_sdk::{
    env, ext_contract, near, require, AccountId, FunctionError, Gas, NearToken, PanicOnDefault,
    Promise,
};

use crate::events::Event;
pub use crate::state::{FreezeState, OperatingState};
use hos_common::RotationCause;

const MIN_TIMEOUT_SECS: u32 = 60;
const MAX_TIMEOUT_SECS: u32 = 2_592_000;
pub const IMPL_VERSION: u32 = 6;
pub const ROTATION_EPOCH: u32 = 4;
const _: () = assert!(
    ROTATION_EPOCH <= IMPL_VERSION,
    "the epoch names the impl version whose migration last reset rotation_seq, so it can never run ahead of the code reporting it"
);
const RENTER_BUFFER: NearToken = NearToken::from_millinear(5);
const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);
const GAS_FOR_FT_TRANSFER: Gas = Gas::from_tgas(10);
const GAS_FOR_SWEEP_RESOLVE: Gas = Gas::from_tgas(5);
/// A revert exists to undo the rotation it follows inside one promise chain,
/// so the window is minutes rather than open ended.
const REVERT_WINDOW_NS: u64 = 5 * 60 * 1_000_000_000;
const SHARDED_ITEM_SPEC: &str = "sharded-item-1.0.0";
const FT_TRANSFER: &str = "ft_transfer";
const NFT_TRANSFER: &str = "nft_transfer";
use hos_common::MAX_AUTHORITY_HOLD_NS;

#[ext_contract(ext_ft)]
pub trait Ft {
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>);
}

/// NEP-641 resolution result. A leaf omits `pending`; this account never does,
/// because it always delegates.
#[near(serializers = [json])]
#[derive(Debug)]
pub struct AuthorizationResolution {
    pub payload: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<PendingAuthorization>,
}

#[near(serializers = [json])]
#[derive(Debug)]
pub struct PendingAuthorization {
    pub account_id: AccountId,
    pub authorization: String,
    pub expect: String,
}

/// The blob this account accepts. It names which owner is authorising, because
/// every entry in `pending` has to resolve: returning all of them would mean a
/// co-owner had to sign before anyone could log in.
#[near(serializers = [json])]
pub struct AuthorizationBlob {
    pub payload: String,
    pub owner: AccountId,
    pub authorization: String,
}

#[near(serializers = [json])]
#[derive(Clone)]
pub struct WalletInit {
    pub owner_account: AccountId,
    pub authority: AccountId,
    pub collection_id: AccountId,
    pub payout_account: AccountId,
    pub lease_until_ns: U64,
    pub timeout_secs: u32,
}

#[near(serializers = [json])]
pub struct NftItemInfo {
    pub spec: String,
    pub init: bool,
    pub status: ItemStatus,
    pub collection_id: AccountId,
    pub token_id: String,
    pub owner_id: AccountId,
    pub rotation_seq: U64,
    pub rotation_epoch: u32,
}

#[near(serializers = [json])]
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum ItemStatus {
    Parked,
    Suspended,
    Expired,
    Frozen,
    Active,
}

#[near(serializers = [json])]
pub struct LeaseView {
    pub authority: AccountId,
    pub payout_account: AccountId,
    pub lease_until_ns: U64,
    pub state: OperatingState,
    pub frozen: FreezeState,
    pub impl_version: u32,
}

#[near(serializers = [json])]
pub struct AgentStatusView {
    pub extension_enabled: bool,
    pub grant: Option<SpendGrant>,
    pub state: OperatingState,
    pub frozen: FreezeState,
    pub lease_until_ns: U64,
    /// The balance floor a spend may not breach. Tracks live storage usage,
    /// so it cannot be derived off chain.
    pub reserve_yocto: U128,
    pub impl_version: u32,
}

/// `receivers` bounds where value may land, for a plain transfer and for the
/// decoded recipient of a token call alike.
#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct SpendGrant {
    pub receivers: BTreeSet<AccountId>,
    pub budget_yocto: U128,
    pub spent_yocto: U128,
    pub tokens: BTreeMap<AccountId, TokenBudget>,
    pub items: BTreeMap<AccountId, BTreeSet<String>>,
    pub expires_at: U64,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct TokenBudget {
    pub budget: U128,
    pub spent: U128,
}

#[near(serializers = [json])]
pub struct TokenAllowance {
    pub token: AccountId,
    pub budget: U128,
}

/// A non-fungible asset has no quantity to meter, so a grant fences the exact
/// items that may leave rather than capping how much value does.
#[near(serializers = [json])]
pub struct ItemAllowance {
    pub collection: AccountId,
    pub token_ids: Vec<String>,
}

/// Unknown fields are refused: an argument this contract cannot read could
/// move value the budget never counted.
#[derive(near_sdk::serde::Deserialize)]
#[serde(crate = "near_sdk::serde", deny_unknown_fields)]
struct FtTransferArgs {
    receiver_id: AccountId,
    amount: U128,
    /// Declared to be permitted, not read.
    #[serde(default)]
    #[allow(dead_code)]
    memo: Option<String>,
}

#[derive(near_sdk::serde::Deserialize)]
#[serde(crate = "near_sdk::serde", deny_unknown_fields)]
struct NftTransferArgs {
    receiver_id: AccountId,
    token_id: String,
    #[serde(default)]
    approval_id: Option<u64>,
    /// Declared to be permitted, not read.
    #[serde(default)]
    #[allow(dead_code)]
    memo: Option<String>,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
pub struct LegacySpendGrant {
    pub receivers: BTreeSet<AccountId>,
    pub budget_yocto: U128,
    pub spent_yocto: U128,
    pub expires_at: U64,
}

#[near(serializers = [borsh])]
pub struct LegacyTenantWallet {
    pub wallet: State<NoPublicKey>,
    pub authority: AccountId,
    pub owner: AccountId,
    pub collection_id: AccountId,
    pub payout_account: AccountId,
    pub lease_until_ns: u64,
    pub state: OperatingState,
    pub frozen: FreezeState,
    pub authority_freeze_until_ns: u64,
    pub spend_grants: BTreeMap<AccountId, LegacySpendGrant>,
    pub revert_to: Option<AccountId>,
    pub revert_until_ns: u64,
    pub rotation_seq: u64,
}

#[near(
    contract_state(key = STATE_KEY),
    contract_metadata(
        standard(standard = "wallet", version = "1.0.0"),
        standard(standard = "wallet-no-sign", version = "1.0.0"),
        standard(standard = "hos-tla-lease", version = "1.0.0"),
    )
)]
#[derive(PanicOnDefault)]
pub struct TenantWallet {
    wallet: State<NoPublicKey>,
    authority: AccountId,
    owner: AccountId,
    collection_id: AccountId,
    payout_account: AccountId,
    lease_until_ns: u64,
    state: OperatingState,
    frozen: FreezeState,
    authority_freeze_until_ns: u64,
    spend_grants: BTreeMap<AccountId, SpendGrant>,
    revert_to: Option<AccountId>,
    revert_until_ns: u64,
    rotation_seq: u64,
}

#[near]
impl Wallet for TenantWallet {
    #[payable]
    fn w_execute_signed(&mut self, _msg: RequestMessage, _proof: String) {
        ContractError::SignatureDisabled.panic();
    }

    #[payable]
    fn w_execute_extension(&mut self, request: Request) {
        if env::attached_deposit().is_zero() {
            ContractError::InsufficientDeposit.panic();
        }
        let extension_id = env::predecessor_account_id();
        if !self.wallet.has_extension(&extension_id) {
            ContractError::ExtensionNotEnabled(extension_id).panic();
        }
        self.wallet.nonces.check_cleanup();
        self.execute_request(request, &Actor::Extension(extension_id.into()));
    }

    fn w_subwallet_id(&self) -> u32 {
        self.wallet.subwallet_id
    }

    fn w_is_signature_allowed(&self) -> bool {
        false
    }

    fn w_public_key(&self) -> String {
        self.wallet.public_key.to_string()
    }

    fn w_is_extension_enabled(&self, account_id: AccountId) -> bool {
        self.wallet.has_extension(&account_id)
    }

    fn w_extensions(&self) -> BTreeSet<AccountId> {
        self.wallet.extensions.clone()
    }

    fn w_timeout_secs(&self) -> u32 {
        u32::try_from(self.wallet.nonces.timeout().as_secs()).unwrap_or(u32::MAX)
    }

    fn w_last_cleaned_at(&self) -> Timestamp {
        self.wallet.nonces.last_cleaned_at()
    }
}

#[near]
impl TenantWallet {
    #[init]
    pub fn hos_init(config: WalletInit) -> Self {
        let WalletInit {
            owner_account,
            authority,
            collection_id,
            payout_account,
            lease_until_ns,
            timeout_secs,
        } = config;
        let current = env::current_account_id();
        require!(
            is_direct_subaccount(&current, &env::predecessor_account_id()),
            error::ONLY_PARENT
        );
        require!(
            lease_until_ns.0 > env::block_timestamp(),
            error::LEASE_IN_PAST
        );
        require!(
            (MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&timeout_secs),
            error::INVALID_TIMEOUT
        );
        require!(authority != current, error::AUTHORITY_IS_SELF);
        require!(payout_account != current, error::PAYOUT_IS_SELF);
        require!(owner_account != current, error::OWNER_IS_SELF);
        require!(owner_account != authority, error::UNAUTHORIZED);
        require!(collection_id != current, error::COLLECTION_IS_SELF);
        Event::WalletInitialized {
            owner: owner_account.clone(),
        }
        .emit();
        let mut wallet = State::new(NoPublicKey)
            .timeout(Duration::from_secs(timeout_secs.into()))
            .extensions([owner_account.clone(), authority.clone()]);
        wallet.signature_enabled = false;
        Self {
            wallet,
            authority,
            owner: owner_account,
            collection_id,
            payout_account,
            lease_until_ns: lease_until_ns.0,
            state: OperatingState::Active,
            frozen: FreezeState::Unfrozen,
            authority_freeze_until_ns: 0,
            spend_grants: BTreeMap::new(),
            revert_to: None,
            revert_until_ns: 0,
            rotation_seq: 0,
        }
    }

    #[init(ignore_state)]
    pub fn hos_migrate(collection_id: AccountId) -> Self {
        let raw = env::storage_read(STATE_KEY).unwrap_or_else(|| env::panic_str(error::NO_STATE));
        let old = LegacyTenantWallet::try_from_slice(&raw)
            .unwrap_or_else(|_| env::panic_str(error::NO_STATE));
        require!(
            env::predecessor_account_id() == old.authority,
            error::ONLY_AUTHORITY
        );
        require!(old.owner != old.authority, error::UNAUTHORIZED);
        require!(
            old.wallet.extensions.contains(&old.owner),
            error::ONLY_OWNER
        );
        require!(
            old.wallet.extensions.contains(&old.authority),
            error::AUTHORITY_PROTECTED
        );
        require!(
            collection_id != env::current_account_id(),
            error::COLLECTION_IS_SELF
        );
        Self {
            wallet: old.wallet,
            authority: old.authority,
            owner: old.owner,
            collection_id,
            payout_account: old.payout_account,
            lease_until_ns: old.lease_until_ns,
            state: old.state,
            frozen: old.frozen,
            authority_freeze_until_ns: old.authority_freeze_until_ns,
            spend_grants: old
                .spend_grants
                .into_iter()
                .map(|(extension, grant)| {
                    (
                        extension,
                        SpendGrant {
                            receivers: grant.receivers,
                            budget_yocto: grant.budget_yocto,
                            spent_yocto: grant.spent_yocto,
                            tokens: BTreeMap::new(),
                            items: BTreeMap::new(),
                            expires_at: grant.expires_at,
                        },
                    )
                })
                .collect(),
            revert_to: old.revert_to,
            revert_until_ns: old.revert_until_ns,
            rotation_seq: old.rotation_seq,
        }
    }

    /// Re-granting raises ceilings without clearing what was already spent.
    /// Revoking is the way to reset the meter.
    #[payable]
    pub fn hos_grant_spend(
        &mut self,
        extension: AccountId,
        receivers: Vec<AccountId>,
        budget_yocto: U128,
        tokens: Vec<TokenAllowance>,
        items: Vec<ItemAllowance>,
        expires_at: U64,
    ) {
        self.assert_owner_caller();
        self.assert_renter_active();
        require!(extension != self.owner, error::UNAUTHORIZED);
        require!(extension != self.authority, error::UNAUTHORIZED);
        require!(
            self.wallet.extensions.contains(&extension),
            error::UNAUTHORIZED
        );
        require!(!receivers.is_empty(), error::EMPTY_GRANT);
        require!(expires_at.0 > env::block_timestamp(), error::GRANT_IN_PAST);
        let held = self.spend_grants.get(&extension);
        let spent_yocto = held.map_or(U128(0), |grant| grant.spent_yocto);
        let budgets = token_budgets(tokens, held, &self.collection_id);
        let fences = item_fences(items, &self.collection_id);
        self.spend_grants.insert(
            extension.clone(),
            SpendGrant {
                receivers: receivers.into_iter().collect(),
                budget_yocto,
                spent_yocto,
                tokens: budgets,
                items: fences,
                expires_at,
            },
        );
        Event::SpendGranted { extension }.emit();
    }

    #[payable]
    pub fn hos_revoke_spend(&mut self, extension: AccountId) {
        self.assert_owner_caller();
        self.spend_grants.remove(&extension);
        Event::SpendRevoked { extension }.emit();
    }

    pub fn hos_spend_grant(&self, extension: AccountId) -> Option<SpendGrant> {
        self.spend_grants.get(&extension).cloned()
    }

    pub fn hos_agent_status(&self, extension: AccountId) -> AgentStatusView {
        AgentStatusView {
            extension_enabled: self.wallet.has_extension(&extension),
            grant: self.spend_grants.get(&extension).cloned(),
            state: self.state,
            frozen: self.effective_frozen(),
            lease_until_ns: U64(self.lease_until_ns),
            reserve_yocto: U128(self.reserve()),
            impl_version: IMPL_VERSION,
        }
    }

    /// NEP-641. Always delegates: the account holds no key, so the owner named
    /// in the blob resolves the authorization instead. The lease authority is
    /// never an eligible owner, so House of Stake cannot prove ownership of a
    /// renter's account.
    ///
    /// `path` is unread. This resolver verifies no signature, so it has no
    /// envelope to bind it against.
    #[allow(unused_variables)]
    pub fn w_resolve_auth(
        &self,
        path: Vec<AccountId>,
        authorization: String,
    ) -> AuthorizationResolution {
        let blob = near_sdk::serde_json::from_str::<AuthorizationBlob>(&authorization)
            .unwrap_or_else(|_| env::panic_str(error::AUTHORIZATION_NOT_JSON));
        require!(blob.owner != self.authority, error::AUTHORITY_NOT_OWNER);
        require!(
            !self.spend_grants.contains_key(&blob.owner),
            error::GRANTEE_NOT_OWNER
        );
        require!(
            self.wallet.extensions.contains(&blob.owner),
            error::NOT_AN_OWNER
        );
        AuthorizationResolution {
            payload: blob.payload.clone(),
            pending: vec![PendingAuthorization {
                account_id: blob.owner,
                authorization: blob.authorization,
                expect: blob.payload,
            }],
        }
    }

    pub fn hos_lease(&self) -> LeaseView {
        LeaseView {
            authority: self.authority.clone(),
            payout_account: self.payout_account.clone(),
            lease_until_ns: U64(self.lease_until_ns),
            state: self.state,
            frozen: self.effective_frozen(),
            impl_version: IMPL_VERSION,
        }
    }

    pub fn nft_item_info(&self) -> NftItemInfo {
        NftItemInfo {
            spec: SHARDED_ITEM_SPEC.to_string(),
            init: self.wallet.extensions.contains(&self.owner),
            status: self.item_status(),
            collection_id: self.collection_id.clone(),
            token_id: env::current_account_id().to_string(),
            owner_id: self.owner.clone(),
            rotation_seq: U64(self.rotation_seq),
            rotation_epoch: ROTATION_EPOCH,
        }
    }

    fn item_status(&self) -> ItemStatus {
        if !self.wallet.extensions.contains(&self.owner) {
            return ItemStatus::Parked;
        }
        self.blocking_condition().unwrap_or(ItemStatus::Active)
    }

    pub fn hos_payout_account(&self) -> AccountId {
        self.payout_account.clone()
    }

    #[payable]
    pub fn hos_set_payout_account(&mut self, payout_account: AccountId, expected_owner: AccountId) {
        self.assert_authority();
        require!(self.owner == expected_owner, error::OWNER_MOVED);
        require!(
            payout_account != env::current_account_id(),
            error::PAYOUT_IS_SELF
        );
        self.payout_account = payout_account.clone();
        Event::PayoutAccountSet { payout_account }.emit();
    }

    #[payable]
    pub fn hos_set_lease(&mut self, lease_until_ns: U64, state: OperatingState) {
        self.assert_authority();
        require!(
            lease_until_ns.0 >= self.lease_until_ns,
            error::LEASE_NOT_MONOTONIC
        );
        require!(state == OperatingState::Active, error::BAD_LEASE_STATE);
        self.lease_until_ns = lease_until_ns.0;
        self.state = state;
        Event::LeaseSet {
            until_ns: lease_until_ns,
            state,
        }
        .emit();
    }

    /// The payout account is read before it is repointed, so the balance goes
    /// to the holder giving the name up rather than the one receiving it.
    #[payable]
    pub fn hos_transfer_ownership(
        &mut self,
        to: Option<AccountId>,
        cause: RotationCause,
        asked_by: Option<AccountId>,
    ) {
        self.assert_authority();
        let previous_owner = self.owner.clone();
        if matches!(cause, RotationCause::Revert) {
            let pinned = self
                .revert_to
                .take()
                .unwrap_or_else(|| env::panic_str(error::NOTHING_TO_REVERT));
            require!(
                env::block_timestamp() < self.revert_until_ns,
                error::REVERT_WINDOW_CLOSED
            );
            require!(to.as_ref() == Some(&pinned), error::REVERT_TARGET_PINNED);
            self.revert_until_ns = 0;
        } else {
            if cause.needs_holder() {
                let holder =
                    asked_by.unwrap_or_else(|| env::panic_str(error::TRANSFER_NOT_REQUESTED));
                require!(holder != self.authority, error::UNAUTHORIZED);
                require!(
                    self.wallet.extensions.contains(&holder),
                    error::TRANSFER_NOT_REQUESTED
                );
            }
            if cause.needs_expiry() {
                require!(self.lease_expired(), error::LEASE_ACTIVE);
            }
            self.revert_to = Some(previous_owner);
            self.revert_until_ns = env::block_timestamp().saturating_add(REVERT_WINDOW_NS);
        }
        self.spend_grants.clear();
        if let Some(next) = to.as_ref() {
            require!(*next != env::current_account_id(), error::SELF_TARGET);
            require!(*next != self.authority, error::UNAUTHORIZED);
        }
        let outgoing = self.payout_account.clone();
        let sweepable = cause.sweeps() && to.as_ref() != Some(&outgoing);
        let authority = self.authority.clone();
        self.rotation_seq = self.rotation_seq.saturating_add(1);
        self.wallet.extensions.retain(|held| *held == authority);
        self.check_lockout();
        if let Some(next) = to {
            self.wallet.extensions.insert(next.clone());
            self.owner = next.clone();
            if cause.repoints_payout() {
                self.payout_account = next.clone();
                Event::PayoutAccountSet {
                    payout_account: next,
                }
                .emit();
            }
        }
        if sweepable {
            self.sweep_to(outgoing);
        }
    }

    #[payable]
    pub fn hos_freeze(&mut self) {
        let caller = env::predecessor_account_id();
        require!(
            caller == self.authority || caller == self.owner,
            error::UNAUTHORIZED
        );
        require!(
            self.wallet.extensions.contains(&caller),
            error::UNAUTHORIZED
        );
        require!(
            self.effective_frozen() == FreezeState::Unfrozen,
            error::FROZEN
        );
        let self_initiated = caller != self.authority;
        self.frozen = if self_initiated {
            self.authority_freeze_until_ns = 0;
            FreezeState::SelfFrozen
        } else {
            self.authority_freeze_until_ns =
                env::block_timestamp().saturating_add(MAX_AUTHORITY_HOLD_NS);
            FreezeState::AuthorityFrozen
        };
        Event::Frozen { self_initiated }.emit();
    }

    #[payable]
    pub fn hos_unfreeze(&mut self) {
        let caller = env::predecessor_account_id();
        require!(
            caller == self.authority || caller == self.owner,
            error::UNAUTHORIZED
        );
        require!(
            self.wallet.extensions.contains(&caller),
            error::UNAUTHORIZED
        );
        let by_authority = caller == self.authority;
        match self.effective_frozen() {
            FreezeState::Unfrozen => env::panic_str(error::NOT_FROZEN),
            FreezeState::SelfFrozen => require!(!by_authority, error::SELF_FROZEN),
            FreezeState::AuthorityFrozen => require!(by_authority, error::AUTHORITY_FROZEN),
        }
        self.frozen = FreezeState::Unfrozen;
        self.authority_freeze_until_ns = 0;
        Event::Unfrozen {}.emit();
    }

    #[payable]
    pub fn hos_sweep_near(&mut self) -> Promise {
        self.assert_sweepable();
        let amount = env::account_balance()
            .as_yoctonear()
            .saturating_sub(self.reserve());
        require!(amount > 0, error::NOTHING_TO_SWEEP);
        let payout_account = self.payout_account.clone();
        Promise::new(payout_account.clone())
            .transfer(NearToken::from_yoctonear(amount))
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_SWEEP_RESOLVE)
                    .hos_resolve_sweep(payout_account, None, U128(amount)),
            )
    }

    #[payable]
    pub fn hos_sweep_ft(&mut self, ft: AccountId, amount: U128) -> Promise {
        self.assert_sweepable();
        require!(amount.0 > 0, error::NOTHING_TO_SWEEP);
        require!(ft != env::current_account_id(), error::SELF_TARGET);
        let payout_account = self.payout_account.clone();
        ext_ft::ext(ft.clone())
            .with_attached_deposit(ONE_YOCTO)
            .with_static_gas(GAS_FOR_FT_TRANSFER)
            .ft_transfer(
                payout_account.clone(),
                amount,
                Some("hos-tla payout".to_string()),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_SWEEP_RESOLVE)
                    .hos_resolve_sweep(payout_account, Some(ft), amount),
            )
    }

    #[private]
    pub fn hos_resolve_sweep(
        &mut self,
        payout_account: AccountId,
        ft: Option<AccountId>,
        amount: U128,
    ) -> bool {
        if !near_sdk::is_promise_success() {
            Event::SweepFailed {
                payout_account,
                ft,
                amount,
            }
            .emit();
            return false;
        }
        match ft {
            Some(ft) => Event::SweptFt {
                payout_account,
                ft,
                amount,
            }
            .emit(),
            None => Event::SweptNear {
                payout_account,
                amount,
            }
            .emit(),
        }
        true
    }
}

impl TenantWallet {
    fn execute_request(&mut self, request: Request, actor: &Actor<'_>) {
        for op in request.internal {
            self.execute_op(op, actor);
        }
        if !request.external.is_empty() {
            self.assert_renter(actor);
            self.assert_renter_active();
            self.charge_spend(actor, &request.external);
            self.assert_within_reserve(&request.external);
            for promise in request.external {
                if promise.receiver_id == env::current_account_id() {
                    ContractError::SelfCallsNotAllowed.panic();
                }
                promise.build().detach();
            }
        }
    }

    fn execute_op(&mut self, op: WalletOp, actor: &Actor<'_>) {
        match op {
            WalletOp::SetSignatureMode { .. } => ContractError::SignatureDisabled.panic(),
            WalletOp::AddExtension { account_id } => self.add_extension(account_id, actor),
            WalletOp::RemoveExtension { account_id } => self.remove_extension(account_id, actor),
        }
    }

    fn add_extension(&mut self, account_id: AccountId, actor: &Actor<'_>) {
        self.assert_extension_editor(actor);
        require!(account_id != env::current_account_id(), error::SELF_TARGET);
        if !self.wallet.extensions.insert(account_id.clone()) {
            ContractError::ExtensionEnabled(account_id).panic();
        }
        WalletEvent::ExtensionAdded {
            account_id: account_id.into(),
            by: actor.as_ref(),
        }
        .emit();
    }

    /// Deviation from upstream: upstream lets any extension remove any other,
    /// which would let a renter evict the authority and escape reclaim.
    fn remove_extension(&mut self, account_id: AccountId, actor: &Actor<'_>) {
        self.assert_extension_editor(actor);
        require!(account_id != self.authority, error::AUTHORITY_PROTECTED);
        if !self.wallet.extensions.remove(&account_id) {
            ContractError::ExtensionNotEnabled(account_id).panic();
        }
        self.spend_grants.remove(&account_id);
        self.check_lockout();
        WalletEvent::ExtensionRemoved {
            account_id: account_id.into(),
            by: actor.as_ref(),
        }
        .emit();
    }

    fn check_lockout(&self) {
        if self.wallet.extensions.is_empty() {
            ContractError::Lockout.panic();
        }
    }

    fn reserve(&self) -> u128 {
        env::storage_byte_cost()
            .as_yoctonear()
            .saturating_mul(env::storage_usage() as u128)
            .saturating_add(RENTER_BUFFER.as_yoctonear())
    }

    fn assert_within_reserve(&self, external: &[NearPromise]) {
        let mut total: u128 = 0;
        for promise in external {
            total = total
                .checked_add(promise.total_deposit().as_yoctonear())
                .unwrap_or_else(|| env::panic_str(error::DEPOSIT_OVERFLOW));
        }
        let balance = env::account_balance().as_yoctonear();
        require!(
            balance.saturating_sub(total) >= self.reserve(),
            error::RESERVE_BREACH
        );
    }

    fn blocking_condition(&self) -> Option<ItemStatus> {
        if self.state != OperatingState::Active {
            return Some(ItemStatus::Suspended);
        }
        if self.lease_expired() {
            return Some(ItemStatus::Expired);
        }
        if self.effective_frozen() != FreezeState::Unfrozen {
            return Some(ItemStatus::Frozen);
        }
        None
    }

    fn assert_renter_active(&self) {
        match self.blocking_condition() {
            None => {}
            Some(ItemStatus::Expired) => env::panic_str(error::LEASE_EXPIRED),
            Some(ItemStatus::Frozen) => env::panic_str(error::FROZEN),
            Some(_) => env::panic_str(error::NOT_ACTIVE),
        }
    }

    fn effective_frozen(&self) -> FreezeState {
        if self.frozen == FreezeState::AuthorityFrozen
            && env::block_timestamp() >= self.authority_freeze_until_ns
        {
            return FreezeState::Unfrozen;
        }
        self.frozen
    }

    fn lease_expired(&self) -> bool {
        env::block_timestamp() >= self.lease_until_ns
    }

    fn assert_authority(&self) {
        require!(
            env::predecessor_account_id() == self.authority,
            error::ONLY_AUTHORITY
        );
    }

    fn assert_sweepable(&self) {
        self.assert_authority();
        require!(self.lease_expired(), error::LEASE_ACTIVE);
    }

    fn sweep_to(&self, outgoing: AccountId) {
        if outgoing == env::current_account_id() {
            return;
        }
        let amount = env::account_balance()
            .as_yoctonear()
            .saturating_sub(env::attached_deposit().as_yoctonear())
            .saturating_sub(self.reserve());
        if amount == 0 {
            return;
        }
        Promise::new(outgoing.clone())
            .transfer(NearToken::from_yoctonear(amount))
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_SWEEP_RESOLVE)
                    .hos_resolve_sweep(outgoing, None, U128(amount)),
            )
            .detach();
    }

    fn assert_renter(&self, actor: &Actor<'_>) {
        require!(
            !Self::is_authority(actor, &self.authority),
            error::ONLY_RENTER
        );
    }

    /// The authority reaches the extension set only once the lease has ended,
    /// which is the reclaim window. While a lease runs, the owner alone
    /// decides who holds their account.
    fn assert_extension_editor(&self, actor: &Actor<'_>) {
        if Self::is_authority(actor, &self.authority) {
            require!(self.lease_expired(), error::EXTENSIONS_LOCKED);
            return;
        }
        self.assert_owner(actor);
        self.assert_renter_active();
    }

    fn charge_spend(&mut self, actor: &Actor<'_>, external: &[NearPromise]) {
        let Actor::Extension(id) = actor else {
            env::panic_str(error::NO_SPEND_GRANT);
        };
        if id.as_ref() == self.owner.as_str() {
            return;
        }
        let extension = id.as_ref().to_owned();
        let collection_id = self.collection_id.clone();
        let grant = self
            .spend_grants
            .get_mut(&extension)
            .unwrap_or_else(|| env::panic_str(error::NO_SPEND_GRANT));
        require!(
            env::block_timestamp() < grant.expires_at.0,
            error::GRANT_EXPIRED
        );
        for promise in external {
            charge_promise(grant, &extension, &collection_id, promise);
        }
    }

    fn assert_owner(&self, actor: &Actor<'_>) {
        let Actor::Extension(id) = actor else {
            env::panic_str(error::ONLY_OWNER);
        };
        require!(id.as_ref() == self.owner.as_str(), error::ONLY_OWNER);
        require!(
            self.wallet.extensions.contains(&self.owner),
            error::ONLY_OWNER
        );
    }

    fn assert_owner_caller(&self) {
        let caller = env::predecessor_account_id();
        require!(caller == self.owner, error::ONLY_OWNER);
        require!(
            self.wallet.extensions.contains(&caller),
            error::UNAUTHORIZED
        );
    }

    fn is_authority(actor: &Actor<'_>, authority: &AccountId) -> bool {
        match actor {
            Actor::Extension(id) => id.as_ref() == authority.as_str(),
            Actor::SignedRequest(_) => false,
        }
    }
}

/// Nothing a grant names may be this account or the collection it belongs to.
/// A name is an account, so what it holds cannot be known when the grant is
/// written, and nothing else in the grant can bound it.
fn assert_grantable(target: &AccountId, collection_id: &AccountId) {
    require!(target != collection_id, error::OWN_COLLECTION_NOT_GRANTABLE);
    require!(*target != env::current_account_id(), error::SELF_TARGET);
}

fn token_budgets(
    tokens: Vec<TokenAllowance>,
    held: Option<&SpendGrant>,
    collection_id: &AccountId,
) -> BTreeMap<AccountId, TokenBudget> {
    let mut budgets = BTreeMap::new();
    for allowance in tokens {
        assert_grantable(&allowance.token, collection_id);
        let spent = held
            .and_then(|grant| grant.tokens.get(&allowance.token))
            .map_or(U128(0), |budget| budget.spent);
        let budget = TokenBudget {
            budget: allowance.budget,
            spent,
        };
        require!(
            budgets.insert(allowance.token, budget).is_none(),
            error::TOKEN_LISTED_TWICE
        );
    }
    budgets
}

fn item_fences(
    items: Vec<ItemAllowance>,
    collection_id: &AccountId,
) -> BTreeMap<AccountId, BTreeSet<String>> {
    let mut fences = BTreeMap::new();
    for allowance in items {
        assert_grantable(&allowance.collection, collection_id);
        require!(!allowance.token_ids.is_empty(), error::EMPTY_ITEM_GRANT);
        let ids: BTreeSet<String> = allowance.token_ids.into_iter().collect();
        require!(
            fences.insert(allowance.collection, ids).is_none(),
            error::COLLECTION_LISTED_TWICE
        );
    }
    fences
}

fn charge_promise(
    grant: &mut SpendGrant,
    extension: &AccountId,
    collection_id: &AccountId,
    promise: &NearPromise,
) {
    require!(
        promise.refund_to.is_none(),
        error::REFUND_TARGET_NOT_ALLOWED
    );
    let call = promise.actions.iter().find_map(|action| match action {
        NearAction::FunctionCall(call) => Some(call),
        _ => None,
    });
    let Some(call) = call else {
        return charge_transfer(grant, extension, promise);
    };
    require!(
        promise.actions.len() == 1,
        error::GRANT_CALL_MUST_STAND_ALONE
    );
    charge_token_call(grant, extension, collection_id, promise, call);
}

fn charge_transfer(grant: &mut SpendGrant, extension: &AccountId, promise: &NearPromise) {
    require!(
        grant.receivers.contains(&promise.receiver_id),
        error::RECEIVER_NOT_GRANTED
    );
    require!(
        promise
            .actions
            .iter()
            .all(|action| matches!(action, NearAction::Transfer(_))),
        error::GRANT_ACTION_NOT_ALLOWED
    );
    let amount = promise.total_deposit().as_yoctonear();
    let spent = grant
        .spent_yocto
        .0
        .checked_add(amount)
        .unwrap_or_else(|| env::panic_str(error::DEPOSIT_OVERFLOW));
    require!(spent <= grant.budget_yocto.0, error::GRANT_CAP_EXCEEDED);
    grant.spent_yocto = U128(spent);
    Event::SpendCharged {
        extension: extension.clone(),
        token: None,
        receiver: promise.receiver_id.clone(),
        amount: U128(amount),
        spent: U128(spent),
    }
    .emit();
}

/// The mandated yocto is protocol overhead rather than spend, so it is
/// required exactly and left out of the NEAR budget.
fn charge_token_call(
    grant: &mut SpendGrant,
    extension: &AccountId,
    collection_id: &AccountId,
    promise: &NearPromise,
    call: &FunctionCall,
) {
    require!(call.deposit == ONE_YOCTO, error::GRANT_CALL_DEPOSIT);
    require!(
        promise.receiver_id != *collection_id,
        error::OWN_COLLECTION_NOT_GRANTABLE
    );
    match call.function_name.as_str() {
        FT_TRANSFER => charge_ft_transfer(grant, extension, promise, call),
        NFT_TRANSFER => charge_nft_transfer(grant, extension, promise, call),
        _ => env::panic_str(error::GRANT_METHOD_NOT_ALLOWED),
    }
}

fn charge_ft_transfer(
    grant: &mut SpendGrant,
    extension: &AccountId,
    promise: &NearPromise,
    call: &FunctionCall,
) {
    let args = near_sdk::serde_json::from_slice::<FtTransferArgs>(&call.args)
        .unwrap_or_else(|_| env::panic_str(error::GRANT_ARGS_UNREADABLE));
    require!(
        grant.receivers.contains(&args.receiver_id),
        error::RECEIVER_NOT_GRANTED
    );
    let budget = grant
        .tokens
        .get_mut(&promise.receiver_id)
        .unwrap_or_else(|| env::panic_str(error::TOKEN_NOT_GRANTED));
    let spent = budget
        .spent
        .0
        .checked_add(args.amount.0)
        .unwrap_or_else(|| env::panic_str(error::DEPOSIT_OVERFLOW));
    require!(spent <= budget.budget.0, error::TOKEN_CAP_EXCEEDED);
    budget.spent = U128(spent);
    Event::SpendCharged {
        extension: extension.clone(),
        token: Some(promise.receiver_id.clone()),
        receiver: args.receiver_id,
        amount: args.amount,
        spent: U128(spent),
    }
    .emit();
}

fn charge_nft_transfer(
    grant: &SpendGrant,
    extension: &AccountId,
    promise: &NearPromise,
    call: &FunctionCall,
) {
    let args = near_sdk::serde_json::from_slice::<NftTransferArgs>(&call.args)
        .unwrap_or_else(|_| env::panic_str(error::GRANT_ARGS_UNREADABLE));
    require!(
        args.approval_id.is_none(),
        error::GRANT_APPROVAL_NOT_ALLOWED
    );
    require!(
        grant.receivers.contains(&args.receiver_id),
        error::RECEIVER_NOT_GRANTED
    );
    let fence = grant
        .items
        .get(&promise.receiver_id)
        .unwrap_or_else(|| env::panic_str(error::COLLECTION_NOT_GRANTED));
    require!(fence.contains(&args.token_id), error::ITEM_NOT_GRANTED);
    Event::ItemSpent {
        extension: extension.clone(),
        collection: promise.receiver_id.clone(),
        token_id: args.token_id,
        receiver: args.receiver_id,
    }
    .emit();
}

fn is_direct_subaccount(child: &AccountId, parent: &AccountId) -> bool {
    child
        .as_str()
        .strip_suffix(parent.as_str())
        .and_then(|prefix| prefix.strip_suffix('.'))
        .is_some_and(|label| !label.is_empty() && !label.contains('.'))
}

#[cfg(test)]
mod tests;
