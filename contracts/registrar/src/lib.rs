mod error;
mod events;

use near_sdk::json_types::{Base58CryptoHash, U64};
use near_sdk::serde_json::json;
use near_sdk::{
    env, near, require, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
    PromiseOrValue, PublicKey,
};

use crate::events::Event;
use hos_common::MintOutcome;

const WA_INIT_GAS: Gas = Gas::from_tgas(15);
const ON_MINTED_GAS: Gas = Gas::from_tgas(20);
const CALLBACK_GAS: Gas = Gas::from_tgas(10);
const MAX_LABEL_LEN: u8 = 60;
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
    pub min_label_len: u8,
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
    registry: AccountId,
    council: AccountId,
    wallet_impl: AccountId,
    hos_extension: AccountId,
    recovery: AccountId,
    chain_id: String,
    min_balance: NearToken,
    min_label_len: u8,
    wallet_timeout_secs: u32,
    approved_code_hash: Option<[u8; 32]>,
    approved_at: Option<u64>,
    config_epoch: u32,
}

#[near(serializers = [borsh])]
pub struct LegacyRegistrar {
    registry: AccountId,
    council: AccountId,
    wallet_impl: AccountId,
    hos_extension: AccountId,
    recovery: AccountId,
    chain_id: String,
    min_balance: NearToken,
    min_label_len: u8,
    wallet_timeout_secs: u32,
    approved_code_hash: Option<[u8; 32]>,
    approved_at: Option<u64>,
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
            (1..=MAX_LABEL_LEN).contains(&config.min_label_len),
            error::BAD_MIN_LABEL_LEN
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
            registry: config.registry,
            council: config.council,
            wallet_impl: config.wallet_impl,
            hos_extension: config.hos_extension,
            recovery: config.recovery,
            chain_id: config.chain_id,
            min_balance: config.min_balance,
            min_label_len: config.min_label_len,
            wallet_timeout_secs: config.wallet_timeout_secs,
            approved_code_hash: None,
            approved_at: None,
            config_epoch: 0,
        }
    }

    #[private]
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        Event::SelfUpgraded {}.emit();
        let Some(old) = hos_common::try_state_read::<LegacyRegistrar>() else {
            let mut current = hos_common::try_state_read::<Self>()
                .unwrap_or_else(|| env::panic_str(error::NO_STATE));
            current.config_epoch = current.config_epoch.saturating_add(1);
            return current;
        };
        Self {
            registry: old.registry,
            council: old.council,
            wallet_impl: old.wallet_impl,
            hos_extension: old.hos_extension,
            recovery: old.recovery,
            chain_id: old.chain_id,
            min_balance: old.min_balance,
            min_label_len: old.min_label_len,
            wallet_timeout_secs: old.wallet_timeout_secs,
            approved_code_hash: None,
            approved_at: None,
            config_epoch: 1,
        }
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
            name.len() >= self.min_label_len as usize,
            error::NAME_TOO_SHORT
        );
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
    pub fn set_min_label_len(&mut self, min_label_len: u8) {
        assert_one_yocto();
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        require!(
            (1..=MAX_LABEL_LEN).contains(&min_label_len),
            error::BAD_MIN_LABEL_LEN
        );
        self.config_epoch = self.config_epoch.saturating_add(1);
        self.min_label_len = min_label_len;
        Event::MinLabelLenSet { min_label_len }.emit();
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
        Event::Sealed {
            public_key: (&public_key).into(),
            by: env::predecessor_account_id(),
        }
        .emit();
        Promise::new(env::current_account_id()).delete_key(public_key)
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

    pub fn registry(&self) -> &AccountId {
        &self.registry
    }

    pub fn min_balance(&self) -> NearToken {
        self.min_balance
    }

    pub fn min_label_len(&self) -> u8 {
        self.min_label_len
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
