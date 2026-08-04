use near_sdk::borsh::BorshSerialize;
use near_sdk::json_types::U128;
use near_sdk::store::LookupMap;
use near_sdk::{
    env, ext_contract, near, AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise,
    PromiseError,
};

const ON_TRANSFER_GAS: Gas = Gas::from_tgas(40);
const RESOLVE_GAS: Gas = Gas::from_tgas(15);

#[ext_contract(ext_ft_receiver)]
pub trait FtReceiver {
    fn ft_on_transfer(&mut self, sender_id: AccountId, amount: U128, msg: String) -> U128;
}

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Balances,
    Registered,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct TestFt {
    pub(crate) balances: LookupMap<AccountId, u128>,
    pub(crate) registered: LookupMap<AccountId, bool>,
    pub(crate) total_supply: u128,
    pub(crate) refuse_storage_deposit: bool,
}

#[near]
impl TestFt {
    #[init]
    pub fn new(owner: AccountId, total_supply: U128) -> Self {
        let mut balances = LookupMap::new(StorageKey::Balances);
        balances.insert(owner.clone(), total_supply.0);
        let mut registered = LookupMap::new(StorageKey::Registered);
        registered.insert(owner, true);
        Self {
            balances,
            registered,
            total_supply: total_supply.0,
            refuse_storage_deposit: false,
        }
    }

    pub fn ft_balance_of(&self, account_id: AccountId) -> U128 {
        U128(self.balances.get(&account_id).copied().unwrap_or(0))
    }

    pub fn ft_total_supply(&self) -> U128 {
        U128(self.total_supply)
    }

    #[payable]
    pub fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>) {
        if env::attached_deposit() != NearToken::from_yoctonear(1) {
            env::panic_str("requires_one_yocto");
        }
        let sender = env::predecessor_account_id();
        if !self.registered.contains_key(&receiver_id) {
            env::panic_str("receiver_not_registered");
        }
        let sender_balance = self.balances.get(&sender).copied().unwrap_or(0);
        if sender_balance < amount.0 {
            env::panic_str("not_enough_balance");
        }
        self.balances
            .insert(sender.clone(), sender_balance.saturating_sub(amount.0));
        let receiver_balance = self.balances.get(&receiver_id).copied().unwrap_or(0);
        self.balances
            .insert(receiver_id, receiver_balance.saturating_add(amount.0));
        let _ = memo;
    }

    #[payable]
    pub fn ft_transfer_call(
        &mut self,
        receiver_id: AccountId,
        amount: U128,
        memo: Option<String>,
        msg: String,
    ) -> Promise {
        if env::attached_deposit() != NearToken::from_yoctonear(1) {
            env::panic_str("requires_one_yocto");
        }
        let sender = env::predecessor_account_id();
        if !self.registered.contains_key(&receiver_id) {
            env::panic_str("receiver_not_registered");
        }
        let sender_balance = self.balances.get(&sender).copied().unwrap_or(0);
        if sender_balance < amount.0 {
            env::panic_str("not_enough_balance");
        }
        self.balances
            .insert(sender.clone(), sender_balance.saturating_sub(amount.0));
        let receiver_balance = self.balances.get(&receiver_id).copied().unwrap_or(0);
        self.balances.insert(
            receiver_id.clone(),
            receiver_balance.saturating_add(amount.0),
        );
        let _ = memo;
        ext_ft_receiver::ext(receiver_id.clone())
            .with_static_gas(ON_TRANSFER_GAS)
            .ft_on_transfer(sender.clone(), amount, msg)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(RESOLVE_GAS)
                    .ft_resolve_transfer(sender, receiver_id, amount),
            )
    }

    #[private]
    pub fn ft_resolve_transfer(
        &mut self,
        sender_id: AccountId,
        receiver_id: AccountId,
        amount: U128,
        #[callback_result] result: Result<U128, PromiseError>,
    ) -> U128 {
        let unused = match result {
            Ok(value) => value.0.min(amount.0),
            Err(_) => amount.0,
        };
        if unused == 0 {
            return amount;
        }
        let receiver_balance = self.balances.get(&receiver_id).copied().unwrap_or(0);
        let refund = unused.min(receiver_balance);
        self.balances
            .insert(receiver_id, receiver_balance.saturating_sub(refund));
        let sender_balance = self.balances.get(&sender_id).copied().unwrap_or(0);
        self.balances
            .insert(sender_id, sender_balance.saturating_add(refund));
        U128(amount.0.saturating_sub(refund))
    }

    #[payable]
    pub fn storage_deposit(
        &mut self,
        account_id: Option<AccountId>,
        registration_only: Option<bool>,
    ) {
        if self.refuse_storage_deposit {
            env::panic_str("storage_deposit_refused");
        }
        let target = account_id.unwrap_or_else(env::predecessor_account_id);
        let _ = registration_only;
        self.registered.insert(target, true);
    }

    pub fn storage_balance_of(&self, account_id: AccountId) -> Option<U128> {
        if self.registered.contains_key(&account_id) {
            Some(U128(1_250_000_000_000_000_000_000))
        } else {
            None
        }
    }

    pub fn is_registered(&self, account_id: AccountId) -> bool {
        self.registered.contains_key(&account_id)
    }

    pub fn set_refuse_storage_deposit(&mut self, value: bool) {
        self.refuse_storage_deposit = value;
    }

    pub fn mint(&mut self, account_id: AccountId, amount: U128) {
        if !self.registered.contains_key(&account_id) {
            self.registered.insert(account_id.clone(), true);
        }
        let cur = self.balances.get(&account_id).copied().unwrap_or(0);
        self.balances
            .insert(account_id, cur.saturating_add(amount.0));
        self.total_supply = self.total_supply.saturating_add(amount.0);
    }
}
