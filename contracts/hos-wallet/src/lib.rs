mod error;
mod events;
mod state;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

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
const IMPL_VERSION: u32 = 3;
const RENTER_BUFFER: NearToken = NearToken::from_millinear(5);
const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);
const GAS_FOR_FT_TRANSFER: Gas = Gas::from_tgas(10);
/// A revert exists to undo the rotation it follows inside one promise chain,
/// so the window is minutes rather than open ended.
const REVERT_WINDOW_NS: u64 = 5 * 60 * 1_000_000_000;
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

/// What an item answers about itself, without anyone having to trust the
/// collection's index. Mirrors TEP-62's `get_nft_data`, which reports
/// `(init?, index, collection, owner, content)` from the item rather than the
/// collection.
#[near(serializers = [json])]
pub struct NftItemInfo {
    pub init: bool,
    pub collection_id: AccountId,
    pub token_id: String,
    pub owner_id: AccountId,
    pub parent_id: String,
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
    pub impl_version: u32,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct SpendGrant {
    pub receivers: BTreeSet<AccountId>,
    pub budget_yocto: U128,
    pub spent_yocto: U128,
    pub expires_at: U64,
}

#[near(serializers = [borsh])]
pub struct LegacyTenantWallet {
    wallet: State<NoPublicKey>,
    authority: AccountId,
    owner: AccountId,
    payout_account: AccountId,
    lease_until_ns: u64,
    state: OperatingState,
    frozen: FreezeState,
    authority_freeze_until_ns: u64,
    transfer_armed: bool,
    spend_grants: BTreeMap<AccountId, SpendGrant>,
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
        self.wallet
            .nonces
            .timeout()
            .as_secs()
            .try_into()
            .unwrap_or_else(|_| unreachable!())
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
        }
    }

    /// `collection_id` names the index this account belongs to. It confers no
    /// authority, so naming the wrong one cannot move or take the account: the
    /// pairing check simply fails and the item is refused.
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
            spend_grants: old.spend_grants,
            revert_to: None,
            revert_until_ns: 0,
        }
    }

    #[payable]
    pub fn hos_grant_spend(
        &mut self,
        extension: AccountId,
        receivers: Vec<AccountId>,
        budget_yocto: U128,
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
        self.spend_grants.insert(
            extension.clone(),
            SpendGrant {
                receivers: receivers.into_iter().collect(),
                budget_yocto,
                spent_yocto: U128(0),
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

    /// The item's own account of itself. A caller pairs this with the
    /// collection's `nft_token` and refuses the item unless both agree, so
    /// neither side is trusted alone. `parent_id` needs no lookup to check:
    /// only that account could have created this one, which is what makes a
    /// forged collection claim detectable.
    pub fn nft_item_info(&self) -> NftItemInfo {
        let token_id = env::current_account_id();
        let parent_id = token_id
            .as_str()
            .split_once('.')
            .map(|(_, parent)| parent.to_string())
            .unwrap_or_default();
        NftItemInfo {
            init: self.wallet.extensions.contains(&self.owner),
            collection_id: self.collection_id.clone(),
            token_id: token_id.to_string(),
            owner_id: self.owner.clone(),
            parent_id,
        }
    }

    pub fn hos_payout_account(&self) -> AccountId {
        self.payout_account.clone()
    }

    #[payable]
    pub fn hos_set_lease(&mut self, lease_until_ns: U64, state: OperatingState) {
        self.assert_authority();
        require!(
            lease_until_ns.0 >= self.lease_until_ns,
            error::LEASE_NOT_MONOTONIC
        );
        require!(state != OperatingState::Parked, error::BAD_LEASE_STATE);
        self.lease_until_ns = lease_until_ns.0;
        self.state = state;
        Event::LeaseSet {
            until_ns: lease_until_ns,
            state,
        }
        .emit();
    }

    /// Moves payout with ownership, evicts co-owners, and returns any balance
    /// above the reserve to the outgoing payout account.
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
        self.wallet.extensions.retain(|held| *held == authority);
        if let Some(next) = to {
            self.wallet.extensions.insert(next.clone());
            self.owner = next.clone();
            self.payout_account = next.clone();
            Event::PayoutAccountSet {
                payout_account: next,
            }
            .emit();
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
        Event::SweptNear {
            payout_account: self.payout_account.clone(),
            amount: U128(amount),
        }
        .emit();
        Promise::new(self.payout_account.clone()).transfer(NearToken::from_yoctonear(amount))
    }

    #[payable]
    pub fn hos_sweep_ft(&mut self, ft: AccountId, amount: U128) -> Promise {
        self.assert_sweepable();
        require!(amount.0 > 0, error::NOTHING_TO_SWEEP);
        require!(ft != env::current_account_id(), error::SELF_TARGET);
        Event::SweptFt {
            payout_account: self.payout_account.clone(),
            ft: ft.clone(),
            amount,
        }
        .emit();
        ext_ft::ext(ft)
            .with_attached_deposit(ONE_YOCTO)
            .with_static_gas(GAS_FOR_FT_TRANSFER)
            .ft_transfer(
                self.payout_account.clone(),
                amount,
                Some("hos-tla payout".to_string()),
            )
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

    fn assert_renter_active(&self) {
        require!(self.state == OperatingState::Active, error::NOT_ACTIVE);
        require!(!self.lease_expired(), error::LEASE_EXPIRED);
        require!(
            self.effective_frozen() == FreezeState::Unfrozen,
            error::FROZEN
        );
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
        Event::SweptNear {
            payout_account: outgoing.clone(),
            amount: U128(amount),
        }
        .emit();
        Promise::new(outgoing)
            .transfer(NearToken::from_yoctonear(amount))
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
        let holder = self
            .spend_grants
            .keys()
            .find(|held| held.as_str() == id.as_ref().as_str())
            .cloned()
            .unwrap_or_else(|| env::panic_str(error::NO_SPEND_GRANT));
        let mut total: u128 = 0;
        for promise in external {
            total = total
                .checked_add(promise.total_deposit().as_yoctonear())
                .unwrap_or_else(|| env::panic_str(error::DEPOSIT_OVERFLOW));
        }
        let grant = self
            .spend_grants
            .get_mut(&holder)
            .unwrap_or_else(|| unreachable!());
        require!(
            env::block_timestamp() < grant.expires_at.0,
            error::GRANT_EXPIRED
        );
        for promise in external {
            require!(
                grant.receivers.contains(&promise.receiver_id),
                error::RECEIVER_NOT_GRANTED
            );
            require!(
                promise.refund_to.is_none(),
                error::REFUND_TARGET_NOT_ALLOWED
            );
        }
        let spent = grant
            .spent_yocto
            .0
            .checked_add(total)
            .unwrap_or_else(|| env::panic_str(error::DEPOSIT_OVERFLOW));
        require!(spent <= grant.budget_yocto.0, error::GRANT_CAP_EXCEEDED);
        grant.spent_yocto = U128(spent);
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

fn is_direct_subaccount(child: &AccountId, parent: &AccountId) -> bool {
    child
        .as_str()
        .strip_suffix(parent.as_str())
        .and_then(|prefix| prefix.strip_suffix('.'))
        .is_some_and(|label| !label.is_empty() && !label.contains('.'))
}

#[cfg(test)]
mod tests;
