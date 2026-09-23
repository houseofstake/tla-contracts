mod error;
mod events;
mod legacy;

use near_sdk::json_types::{Base58CryptoHash, U64};
use near_sdk::serde_json::json;
use near_sdk::{
    env, near, require, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
    PromiseOrValue, PublicKey,
};

use crate::events::Event;
use hos_common::MintOutcome;

const WA_INIT_TGAS: u64 = 15;
const ON_MINTED_TGAS: u64 = 20;
const CALLBACK_TGAS: u64 = 10;
const MINT_FRAME_TGAS: u64 = 20;
const ON_MINTED_FRAME_TGAS: u64 = 5;
const WA_INIT_GAS: Gas = Gas::from_tgas(WA_INIT_TGAS);
const ON_MINTED_GAS: Gas = Gas::from_tgas(ON_MINTED_TGAS);
const CALLBACK_GAS: Gas = Gas::from_tgas(CALLBACK_TGAS);
const _: () = assert!(
    WA_INIT_TGAS + ON_MINTED_TGAS + MINT_FRAME_TGAS <= hos_common::MINT_CALL_TGAS,
    "the registry funds the whole mint from one static budget, so the batch, its callback and this frame must fit inside it"
);
const _: () = assert!(
    CALLBACK_TGAS + ON_MINTED_FRAME_TGAS <= ON_MINTED_TGAS,
    "on_minted must always afford its refund hop, or a failed batch reports no outcome and strands the name"
);
const STATE_VERSION: u16 = 2;
const ACCOUNT_STORAGE_FLOOR: NearToken = NearToken::from_millinear(7);
const UPGRADE_DELAY_NS: u64 = 48 * 60 * 60 * 1_000_000_000;

#[near(serializers = [json])]
#[derive(Clone)]
pub struct RegistrarConfig {
    pub registry: AccountId,
    pub council: AccountId,
    pub wallet_impl: AccountId,
    pub hos_extension: AccountId,
    pub recovery: AccountId,
    pub chain_id: String,
    pub min_balance: NearToken,
    pub wallet_timeout_secs: u32,
}

#[near(serializers = [json])]
pub struct RegistrarView {
    pub council: AccountId,
    pub wallet_impl: AccountId,
    pub hos_extension: AccountId,
    pub recovery: AccountId,
    pub chain_id: String,
    pub wallet_timeout_secs: u32,
    pub registry: AccountId,
    pub config_epoch: u32,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Registrar {
    state_version: u16,
    registry: AccountId,
    council: AccountId,
    wallet_impl: AccountId,
    hos_extension: AccountId,
    recovery: AccountId,
    chain_id: String,
    min_balance: NearToken,
    wallet_timeout_secs: u32,
    approved_code_hash: Option<[u8; 32]>,
    approved_at: Option<u64>,
    config_epoch: u32,
    upgrade_proven: bool,
    pending_council: Option<AccountId>,
    pending_council_at: Option<u64>,
}

#[near]
impl Registrar {
    #[init]
    pub fn new(config: RegistrarConfig) -> Self {
        require!(
            config.chain_id == "mainnet" || config.chain_id == "testnet",
            error::BAD_CHAIN_ID
        );
        require!(
            config.min_balance >= ACCOUNT_STORAGE_FLOOR,
            error::BAD_MIN_BALANCE
        );
        require!(
            config.council != env::current_account_id(),
            error::COUNCIL_IS_SELF
        );
        require!(
            config.registry != env::current_account_id(),
            error::REGISTRY_IS_SELF
        );
        Self {
            state_version: STATE_VERSION,
            registry: config.registry,
            council: config.council,
            wallet_impl: config.wallet_impl,
            hos_extension: config.hos_extension,
            recovery: config.recovery,
            chain_id: config.chain_id,
            min_balance: config.min_balance,
            wallet_timeout_secs: config.wallet_timeout_secs,
            approved_code_hash: None,
            approved_at: None,
            config_epoch: 0,
            upgrade_proven: false,
            pending_council: None,
            pending_council_at: None,
        }
    }

    #[private]
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        let mut current = match hos_common::state_version() {
            Some(STATE_VERSION) => hos_common::try_state_read::<Self>()
                .unwrap_or_else(|| env::panic_str(error::NO_STATE)),
            Some(1) => hos_common::try_state_read::<legacy::RegistrarV1>()
                .map(Self::from)
                .unwrap_or_else(|| env::panic_str(error::NO_STATE)),
            Some(_) => env::panic_str(error::STATE_VERSION_UNKNOWN),
            None => env::panic_str(error::NO_STATE),
        };
        current.config_epoch = current.config_epoch.saturating_add(1);
        current.upgrade_proven = true;
        Event::SelfUpgraded {}.emit();
        current
    }

    #[payable]
    pub fn create_sub_account(
        &mut self,
        name: String,
        owner_account: AccountId,
        payout_account: AccountId,
        lease_until_ns: u64,
    ) -> Promise {
        require!(
            env::predecessor_account_id() == self.registry,
            error::ONLY_REGISTRY
        );
        require!(!name.is_empty() && !name.contains('.'), error::INVALID_NAME);
        require!(
            lease_until_ns > env::block_timestamp(),
            error::LEASE_IN_PAST
        );
        let funding = env::attached_deposit();
        require!(funding >= self.min_balance, error::INSUFFICIENT_DEPOSIT);

        let account: AccountId = format!("{}.{}", name, env::current_account_id())
            .parse()
            .unwrap_or_else(|_| env::panic_str(error::INVALID_NAME));

        require!(payout_account != account, error::PAYOUT_IS_SELF);
        require!(owner_account != account, error::OWNER_ACCOUNT_IS_SELF);

        let init_args = json!({
            "config": {
                "owner_account": owner_account,
                "authority": self.hos_extension,
                "collection_id": self.registry,
                "payout_account": payout_account,
                "lease_until_ns": U64(lease_until_ns),
                "timeout_secs": self.wallet_timeout_secs,
            }
        })
        .to_string()
        .into_bytes();

        Promise::new(account.clone())
            .create_account()
            .transfer(funding)
            .use_global_contract_by_account_id(self.wallet_impl.clone())
            .function_call(
                "hos_init".to_string(),
                init_args,
                NearToken::ZERO,
                WA_INIT_GAS,
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(ON_MINTED_GAS)
                    .on_minted(account, owner_account, funding),
            )
    }

    #[private]
    pub fn on_minted(
        &mut self,
        account: AccountId,
        owner_account: AccountId,
        funding: NearToken,
        #[callback_result] result: Result<(), PromiseError>,
    ) -> PromiseOrValue<MintOutcome> {
        match result {
            Ok(()) => {
                Event::SubAccountMinted {
                    account,
                    owner: owner_account,
                }
                .emit();
                PromiseOrValue::Value(MintOutcome::Active)
            }
            Err(_) => {
                Event::MintFailed { account }.emit();
                PromiseOrValue::Promise(
                    Promise::new(self.registry.clone()).transfer(funding).then(
                        Self::ext(env::current_account_id())
                            .with_static_gas(CALLBACK_GAS)
                            .on_creation_failed(),
                    ),
                )
            }
        }
    }

    #[private]
    pub fn on_creation_failed(&self) -> MintOutcome {
        MintOutcome::CreationFailed
    }

    #[payable]
    pub fn set_min_balance(&mut self, min_balance: NearToken) {
        assert_one_yocto();
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        require!(min_balance >= ACCOUNT_STORAGE_FLOOR, error::BAD_MIN_BALANCE);
        self.config_epoch = self.config_epoch.saturating_add(1);
        self.min_balance = min_balance;
        Event::MinBalanceSet { min_balance }.emit();
    }

    #[payable]
    pub fn approve_upgrade(&mut self, code_hash: Base58CryptoHash) {
        assert_one_yocto();
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        self.approved_code_hash = Some(code_hash.into());
        self.approved_at = Some(env::block_timestamp());
        Event::UpgradeApproved {
            hash: String::from(&code_hash),
            by: env::predecessor_account_id(),
        }
        .emit();
    }

    #[payable]
    pub fn approve_council_rotation(&mut self, new_council: AccountId) {
        assert_one_yocto();
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        require!(new_council != self.council, error::COUNCIL_UNCHANGED);
        require!(
            new_council != env::current_account_id(),
            error::COUNCIL_IS_SELF
        );
        self.pending_council = Some(new_council.clone());
        self.pending_council_at = Some(env::block_timestamp());
        Event::CouncilRotationApproved {
            new_council,
            by: env::predecessor_account_id(),
        }
        .emit();
    }

    #[payable]
    pub fn cancel_council_rotation(&mut self) {
        assert_one_yocto();
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        require!(
            self.pending_council.take().is_some(),
            error::NO_COUNCIL_ROTATION_PENDING
        );
        self.pending_council_at = None;
        Event::CouncilRotationCancelled {
            by: env::predecessor_account_id(),
        }
        .emit();
    }

    #[payable]
    pub fn commit_council_rotation(&mut self) {
        assert_one_yocto();
        let pending = self
            .pending_council
            .clone()
            .unwrap_or_else(|| env::panic_str(error::NO_COUNCIL_ROTATION_PENDING));
        require!(
            env::predecessor_account_id() == pending,
            error::ONLY_PENDING_COUNCIL
        );
        let approved_at = self
            .pending_council_at
            .unwrap_or_else(|| env::panic_str(error::NO_COUNCIL_ROTATION_PENDING));
        require!(
            env::block_timestamp() >= approved_at.saturating_add(UPGRADE_DELAY_NS),
            error::COUNCIL_ROTATION_TOO_YOUNG
        );
        self.council = pending.clone();
        self.pending_council = None;
        self.pending_council_at = None;
        Event::CouncilRotated {
            new_council: pending,
            by: env::predecessor_account_id(),
        }
        .emit();
    }

    pub fn pending_council(&self) -> Option<(AccountId, U64)> {
        self.pending_council
            .clone()
            .zip(self.pending_council_at.map(U64))
    }

    #[payable]
    pub fn upgrade_self(&mut self, code: near_sdk::json_types::Base64VecU8) -> Promise {
        assert_one_yocto();
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        let code = code.0;
        require!(!code.is_empty(), error::EMPTY_CODE);
        let approved = self
            .approved_code_hash
            .unwrap_or_else(|| env::panic_str(error::NO_APPROVED_HASH));
        require!(env::sha256_array(&code) == approved, error::HASH_MISMATCH);
        let approved_at = self
            .approved_at
            .unwrap_or_else(|| env::panic_str(error::NO_APPROVED_HASH));
        require!(
            env::block_timestamp() >= approved_at.saturating_add(UPGRADE_DELAY_NS),
            error::APPROVAL_TOO_YOUNG
        );
        self.approved_code_hash = None;
        self.approved_at = None;
        hos_common::deploy_and_migrate(code)
    }

    #[payable]
    pub fn seal(&mut self, public_key: PublicKey) -> Promise {
        assert_one_yocto();
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        require!(self.upgrade_proven, error::UPGRADE_NOT_PROVEN);
        let by = env::predecessor_account_id();
        let key: String = (&public_key).into();
        Promise::new(env::current_account_id())
            .delete_key(public_key)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(CALLBACK_GAS)
                    .after_seal(key, by),
            )
    }

    #[private]
    pub fn after_seal(&mut self, public_key: String, by: AccountId) -> bool {
        if !near_sdk::is_promise_success() {
            Event::SealFailed { public_key, by }.emit();
            return false;
        }
        Event::Sealed { public_key, by }.emit();
        true
    }

    pub fn approved_upgrade_hash(&self) -> Option<Base58CryptoHash> {
        self.approved_code_hash.map(Into::into)
    }

    pub fn approved_upgrade_at(&self) -> Option<U64> {
        self.approved_at.map(U64)
    }

    pub fn upgrade_delay_ns(&self) -> U64 {
        U64(UPGRADE_DELAY_NS)
    }

    pub fn upgrade_proven(&self) -> bool {
        self.upgrade_proven
    }

    pub fn state_version(&self) -> u16 {
        self.state_version
    }

    pub fn registry(&self) -> &AccountId {
        &self.registry
    }

    pub fn min_balance(&self) -> NearToken {
        self.min_balance
    }

    pub fn config(&self) -> RegistrarView {
        RegistrarView {
            council: self.council.clone(),
            wallet_impl: self.wallet_impl.clone(),
            hos_extension: self.hos_extension.clone(),
            recovery: self.recovery.clone(),
            chain_id: self.chain_id.clone(),
            wallet_timeout_secs: self.wallet_timeout_secs,
            registry: self.registry.clone(),
            config_epoch: self.config_epoch,
        }
    }

    pub fn config_epoch(&self) -> u32 {
        self.config_epoch
    }
}

fn assert_one_yocto() {
    require!(
        env::attached_deposit() == NearToken::from_yoctonear(1),
        error::REQUIRES_ONE_YOCTO
    );
}

#[cfg(test)]
mod tests;
