mod error;
mod events;

use near_sdk::json_types::U64;
use near_sdk::serde_json::json;
use near_sdk::{
    env, near, require, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
    PromiseOrValue,
};

use crate::events::Event;
use hos_common::MintOutcome;

const WA_INIT_GAS: Gas = Gas::from_tgas(15);
const ON_MINTED_GAS: Gas = Gas::from_tgas(20);
const CALLBACK_GAS: Gas = Gas::from_tgas(10);
const MAX_LABEL_LEN: u8 = 60;
const ACCOUNT_STORAGE_FLOOR: NearToken = NearToken::from_millinear(7);

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

    pub fn set_min_label_len(&mut self, min_label_len: u8) {
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        require!(
            (1..=MAX_LABEL_LEN).contains(&min_label_len),
            error::BAD_MIN_LABEL_LEN
        );
        self.min_label_len = min_label_len;
        Event::MinLabelLenSet { min_label_len }.emit();
    }

    pub fn set_min_balance(&mut self, min_balance: NearToken) {
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        require!(min_balance >= ACCOUNT_STORAGE_FLOOR, error::BAD_MIN_BALANCE);
        self.min_balance = min_balance;
        Event::MinBalanceSet { min_balance }.emit();
    }

    pub fn upgrade_self(&mut self, code: Vec<u8>) -> Promise {
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        require!(!code.is_empty(), error::EMPTY_CODE);
        Event::SelfUpgraded {}.emit();
        Promise::new(env::current_account_id()).deploy_contract(code)
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;
    use std::str::FromStr;

    const REGISTRY: &str = "tla-registry.testnet";
    const COUNCIL: &str = "council.testnet";
    const IMPL: &str = "w.hos.testnet";
    const EXTENSION: &str = "hos-extension.testnet";
    const RECOVERY: &str = "mpc-recovery.testnet";
    const PAYOUT: &str = "payout.testnet";
    const OWNER_ACC: &str = "renter.testnet";
    const TLA: &str = "tla.testnet";
    const TS: u64 = 1_000_000_000_000;
    const YEAR_NS: u64 = 31_536_000_000_000_000;

    fn acc(s: &str) -> AccountId {
        AccountId::from_str(s).unwrap()
    }

    fn ctx(predecessor: &str, deposit: u128) {
        testing_env!(VMContextBuilder::new()
            .current_account_id(acc(TLA))
            .predecessor_account_id(acc(predecessor))
            .attached_deposit(NearToken::from_yoctonear(deposit))
            .block_timestamp(TS)
            .build());
    }

    fn deploy() -> Registrar {
        ctx(COUNCIL, 0);
        Registrar::new(RegistrarConfig {
            registry: acc(REGISTRY),
            council: acc(COUNCIL),
            wallet_impl: acc(IMPL),
            hos_extension: acc(EXTENSION),
            recovery: acc(RECOVERY),
            chain_id: "testnet".to_string(),
            min_balance: NearToken::from_millinear(100),
            min_label_len: 3,
            wallet_timeout_secs: 3600,
        })
    }

    fn min() -> u128 {
        NearToken::from_millinear(100).as_yoctonear()
    }

    fn lease() -> u64 {
        TS + YEAR_NS
    }

    #[test]
    fn registry_can_mint() {
        let mut c = deploy();
        ctx(REGISTRY, min());
        let _ = c.create_sub_account("alice".to_string(), acc(OWNER_ACC), acc(PAYOUT), lease());
    }

    #[test]
    #[should_panic(expected = "only registry")]
    fn outsider_cannot_mint() {
        let mut c = deploy();
        ctx("attacker.testnet", min());
        let _ = c.create_sub_account("alice".to_string(), acc(OWNER_ACC), acc(PAYOUT), lease());
    }

    #[test]
    #[should_panic(expected = "name shorter than the minimum label length")]
    fn short_name_rejected() {
        let mut c = deploy();
        ctx(REGISTRY, min());
        let _ = c.create_sub_account("ab".to_string(), acc(OWNER_ACC), acc(PAYOUT), lease());
    }

    #[test]
    #[should_panic(expected = "invalid sub-account name")]
    fn dotted_name_rejected() {
        let mut c = deploy();
        ctx(REGISTRY, min());
        let _ = c.create_sub_account(
            "alice.bob".to_string(),
            acc(OWNER_ACC),
            acc(PAYOUT),
            lease(),
        );
    }

    #[test]
    #[should_panic(expected = "invalid sub-account name")]
    fn empty_name_rejected() {
        let mut c = deploy();
        ctx(REGISTRY, min());
        let _ = c.create_sub_account(String::new(), acc(OWNER_ACC), acc(PAYOUT), lease());
    }

    #[test]
    #[should_panic(expected = "deposit below minimum balance")]
    fn underfunded_mint_rejected() {
        let mut c = deploy();
        ctx(REGISTRY, min() - 1);
        let _ = c.create_sub_account("alice".to_string(), acc(OWNER_ACC), acc(PAYOUT), lease());
    }

    #[test]
    #[should_panic(expected = "lease_until_ns must be in the future")]
    fn past_lease_rejected() {
        let mut c = deploy();
        ctx(REGISTRY, min());
        let _ = c.create_sub_account("alice".to_string(), acc(OWNER_ACC), acc(PAYOUT), TS);
    }

    #[test]
    fn mint_success_reports_active() {
        let mut c = deploy();
        ctx(TLA, 0);
        let out = c.on_minted(
            acc("alice.tla.testnet"),
            acc(OWNER_ACC),
            NearToken::from_millinear(100),
            Ok(()),
        );
        assert!(matches!(out, PromiseOrValue::Value(MintOutcome::Active)));
    }

    #[test]
    fn mint_failure_refunds_registry() {
        let mut c = deploy();
        ctx(TLA, 0);
        let out = c.on_minted(
            acc("alice.tla.testnet"),
            acc(OWNER_ACC),
            NearToken::from_millinear(100),
            Err(PromiseError::Failed),
        );
        assert!(matches!(out, PromiseOrValue::Promise(_)));
    }

    #[test]
    fn council_sets_min_label_len() {
        let mut c = deploy();
        ctx(COUNCIL, 0);
        c.set_min_label_len(4);
        assert_eq!(c.min_label_len(), 4);
    }

    #[test]
    #[should_panic(expected = "only council")]
    fn registry_cannot_set_min_label_len() {
        let mut c = deploy();
        ctx(REGISTRY, 0);
        c.set_min_label_len(4);
    }

    #[test]
    #[should_panic(expected = "min label length out of bounds")]
    fn zero_min_label_len_rejected() {
        let mut c = deploy();
        ctx(COUNCIL, 0);
        c.set_min_label_len(0);
    }

    #[test]
    fn council_sets_min_balance_to_the_storage_floor() {
        let mut c = deploy();
        ctx(COUNCIL, 0);
        c.set_min_balance(ACCOUNT_STORAGE_FLOOR);
        assert_eq!(c.min_balance(), ACCOUNT_STORAGE_FLOOR);
    }

    #[test]
    #[should_panic(expected = "only council")]
    fn registry_cannot_set_min_balance() {
        let mut c = deploy();
        ctx(REGISTRY, 0);
        c.set_min_balance(NearToken::from_millinear(50));
    }

    #[test]
    #[should_panic(expected = "min balance below the account storage floor")]
    fn min_balance_below_storage_floor_rejected() {
        let mut c = deploy();
        ctx(COUNCIL, 0);
        c.set_min_balance(NearToken::from_millinear(1));
    }

    #[test]
    #[should_panic(expected = "min balance below the account storage floor")]
    fn init_below_storage_floor_rejected() {
        ctx(COUNCIL, 0);
        let _ = Registrar::new(RegistrarConfig {
            registry: acc(REGISTRY),
            council: acc(COUNCIL),
            wallet_impl: acc(IMPL),
            hos_extension: acc(EXTENSION),
            recovery: acc(RECOVERY),
            chain_id: "testnet".to_string(),
            min_balance: NearToken::from_millinear(1),
            min_label_len: 3,
            wallet_timeout_secs: 3600,
        });
    }

    #[test]
    #[should_panic(expected = "only council")]
    fn outsider_cannot_upgrade() {
        let mut c = deploy();
        ctx("attacker.testnet", 0);
        let _ = c.upgrade_self(vec![1]);
    }

    #[test]
    fn config_roundtrips() {
        let c = deploy();
        let view = c.config();
        assert_eq!(view.council, acc(COUNCIL));
        assert_eq!(view.wallet_impl, acc(IMPL));
        assert_eq!(view.hos_extension, acc(EXTENSION));
        assert_eq!(view.recovery, acc(RECOVERY));
        assert_eq!(view.chain_id, "testnet");
        assert_eq!(view.wallet_timeout_secs, 3600);
    }
}
