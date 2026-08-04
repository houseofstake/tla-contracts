mod error;
mod events;
mod state;

use std::collections::BTreeSet;
use std::time::Duration;

use defuse_wallet::contract::Wallet;
use defuse_wallet::events::{Actor, WalletEvent};
use defuse_wallet::{
    ContractError, NearPromise, Request, RequestMessage, State, Timestamp, WalletOp, STATE_KEY,
};
use defuse_wallet_no_sign::NoPublicKey;
use near_sdk::json_types::{U128, U64};
use near_sdk::{
    env, ext_contract, near, require, AccountId, FunctionError, Gas, NearToken, PanicOnDefault,
    Promise,
};

use crate::events::Event;
pub use crate::state::{FreezeState, OperatingState};

const MIN_TIMEOUT_SECS: u32 = 60;
const MAX_TIMEOUT_SECS: u32 = 2_592_000;
const IMPL_VERSION: u32 = 3;
const RENTER_BUFFER: NearToken = NearToken::from_millinear(5);
const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);
const GAS_FOR_FT_TRANSFER: Gas = Gas::from_tgas(10);

#[ext_contract(ext_ft)]
pub trait Ft {
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>);
}

/// NEP-641 resolution result. Wire shape is internally tagged on `status`.
#[near(serializers = [json])]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
#[derive(Debug)]
pub enum AuthorizationResolution {
    Resolved {
        payload: String,
    },
    Pending {
        payload: String,
        pending_authorizations: Vec<PendingAuthorization>,
    },
    Invalid {
        error_kind: ErrorKind,
        error_message: String,
    },
}

#[near(serializers = [json])]
#[derive(Debug)]
pub struct PendingAuthorization {
    pub account_id: AccountId,
    pub purpose: String,
    pub authorization: String,
}

#[near(serializers = [json])]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorKind {
    InvalidInput,
    InvalidSignature,
}

#[near(serializers = [json])]
pub struct AuthorizationBlob {
    pub message: String,
}

#[near(serializers = [json])]
#[derive(Clone)]
pub struct WalletInit {
    pub owner_account: AccountId,
    pub authority: AccountId,
    pub payout_account: AccountId,
    pub lease_until_ns: U64,
    pub timeout_secs: u32,
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
    payout_account: AccountId,
    lease_until_ns: u64,
    state: OperatingState,
    frozen: FreezeState,
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
        Event::WalletInitialized {
            owner: owner_account.clone(),
        }
        .emit();
        let mut wallet = State::new(NoPublicKey)
            .timeout(Duration::from_secs(timeout_secs.into()))
            .extensions([owner_account, authority.clone()]);
        wallet.signature_enabled = false;
        Self {
            wallet,
            authority,
            payout_account,
            lease_until_ns: lease_until_ns.0,
            state: OperatingState::Active,
            frozen: FreezeState::Unfrozen,
        }
    }

    /// NEP-641. Always delegates: the account holds no key, so its owner
    /// accounts resolve the authorization instead.
    ///
    /// `recipient` is unread but must keep its name, which is the JSON field.
    #[allow(unused_variables)]
    pub fn w_resolve_auth(
        &self,
        purpose: String,
        recipient: String,
        authorization: String,
    ) -> AuthorizationResolution {
        let Ok(blob) = near_sdk::serde_json::from_str::<AuthorizationBlob>(&authorization) else {
            return AuthorizationResolution::Invalid {
                error_kind: ErrorKind::InvalidInput,
                error_message: "authorization is not valid json".to_string(),
            };
        };
        let owners: Vec<PendingAuthorization> = self
            .wallet
            .extensions
            .iter()
            .filter(|held| **held != self.authority)
            .map(|owner| PendingAuthorization {
                account_id: owner.clone(),
                purpose: purpose.clone(),
                authorization: authorization.clone(),
            })
            .collect();
        if owners.is_empty() {
            return AuthorizationResolution::Invalid {
                error_kind: ErrorKind::InvalidSignature,
                error_message: "no owner can authorise this account".to_string(),
            };
        }
        AuthorizationResolution::Pending {
            payload: blob.message,
            pending_authorizations: owners,
        }
    }

    pub fn hos_lease(&self) -> LeaseView {
        LeaseView {
            authority: self.authority.clone(),
            payout_account: self.payout_account.clone(),
            lease_until_ns: U64(self.lease_until_ns),
            state: self.state,
            frozen: self.frozen,
            impl_version: IMPL_VERSION,
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

    /// Moves payout with ownership and evicts co-owners.
    #[payable]
    pub fn hos_transfer_ownership(&mut self, to: Option<AccountId>) {
        self.assert_authority();
        if let Some(next) = to.as_ref() {
            require!(*next != env::current_account_id(), error::SELF_TARGET);
            require!(*next != self.authority, error::UNAUTHORIZED);
        }
        let authority = self.authority.clone();
        self.wallet.extensions.retain(|held| *held == authority);
        if let Some(next) = to {
            self.wallet.extensions.insert(next.clone());
            self.payout_account = next.clone();
            Event::PayoutAccountSet {
                payout_account: next,
            }
            .emit();
        }
    }

    #[payable]
    pub fn hos_freeze(&mut self) {
        let caller = env::predecessor_account_id();
        require!(
            self.wallet.extensions.contains(&caller),
            error::UNAUTHORIZED
        );
        require!(self.frozen == FreezeState::Unfrozen, error::FROZEN);
        let self_initiated = caller != self.authority;
        self.frozen = if self_initiated {
            FreezeState::SelfFrozen
        } else {
            FreezeState::AuthorityFrozen
        };
        Event::Frozen { self_initiated }.emit();
    }

    #[payable]
    pub fn hos_unfreeze(&mut self) {
        let caller = env::predecessor_account_id();
        require!(
            self.wallet.extensions.contains(&caller),
            error::UNAUTHORIZED
        );
        let by_authority = caller == self.authority;
        match self.frozen {
            FreezeState::Unfrozen => env::panic_str(error::NOT_FROZEN),
            FreezeState::SelfFrozen => require!(!by_authority, error::SELF_FROZEN),
            FreezeState::AuthorityFrozen => require!(by_authority, error::AUTHORITY_FROZEN),
        }
        self.frozen = FreezeState::Unfrozen;
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
        self.assert_renter_or_authority(actor);
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
        self.assert_renter_or_authority(actor);
        require!(account_id != self.authority, error::AUTHORITY_PROTECTED);
        if !self.wallet.extensions.remove(&account_id) {
            ContractError::ExtensionNotEnabled(account_id).panic();
        }
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
        require!(self.frozen == FreezeState::Unfrozen, error::FROZEN);
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

    fn assert_renter(&self, actor: &Actor<'_>) {
        require!(
            !Self::is_authority(actor, &self.authority),
            error::ONLY_RENTER
        );
    }

    fn assert_renter_or_authority(&self, actor: &Actor<'_>) {
        if Self::is_authority(actor, &self.authority) {
            return;
        }
        self.assert_renter_active();
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
