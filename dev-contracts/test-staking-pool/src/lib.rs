use near_sdk::borsh::BorshSerialize;
use near_sdk::json_types::U128;
use near_sdk::store::LookupMap;
use near_sdk::{env, near, AccountId, BorshStorageKey, NearToken, PanicOnDefault, Promise};

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Staked,
    Unstaked,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct TestStakingPool {
    staked: LookupMap<AccountId, u128>,
    unstaked: LookupMap<AccountId, u128>,
}

#[near]
impl TestStakingPool {
    #[init]
    pub fn new() -> Self {
        Self {
            staked: LookupMap::new(StorageKey::Staked),
            unstaked: LookupMap::new(StorageKey::Unstaked),
        }
    }

    #[payable]
    pub fn deposit_and_stake(&mut self) {
        let amount = env::attached_deposit().as_yoctonear();
        if amount == 0 {
            env::panic_str("no_deposit");
        }
        let who = env::predecessor_account_id();
        let current = self.staked.get(&who).copied().unwrap_or(0);
        self.staked.insert(who, current.saturating_add(amount));
    }

    pub fn unstake(&mut self, amount: U128) {
        let who = env::predecessor_account_id();
        let staked = self.staked.get(&who).copied().unwrap_or(0);
        if staked < amount.0 {
            env::panic_str("not_enough_staked");
        }
        self.staked.insert(who.clone(), staked - amount.0);
        let pending = self.unstaked.get(&who).copied().unwrap_or(0);
        self.unstaked.insert(who, pending.saturating_add(amount.0));
    }

    pub fn withdraw(&mut self, amount: U128) -> Promise {
        let who = env::predecessor_account_id();
        let pending = self.unstaked.get(&who).copied().unwrap_or(0);
        if pending < amount.0 {
            env::panic_str("not_enough_unstaked");
        }
        self.unstaked.insert(who.clone(), pending - amount.0);
        Promise::new(who).transfer(NearToken::from_yoctonear(amount.0))
    }

    pub fn get_account_staked_balance(&self, account_id: AccountId) -> U128 {
        U128(self.staked.get(&account_id).copied().unwrap_or(0))
    }

    pub fn get_account_unstaked_balance(&self, account_id: AccountId) -> U128 {
        U128(self.unstaked.get(&account_id).copied().unwrap_or(0))
    }

    pub fn get_account_total_balance(&self, account_id: AccountId) -> U128 {
        let staked = self.staked.get(&account_id).copied().unwrap_or(0);
        let pending = self.unstaked.get(&account_id).copied().unwrap_or(0);
        U128(staked.saturating_add(pending))
    }
}
