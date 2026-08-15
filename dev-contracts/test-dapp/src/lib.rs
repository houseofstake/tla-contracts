use near_sdk::borsh::BorshSerialize;
use near_sdk::json_types::U128;
use near_sdk::store::LookupMap;
use near_sdk::{env, near, AccountId, BorshStorageKey, PanicOnDefault};

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Positions,
    Actions,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct TestDapp {
    token: AccountId,
    positions: LookupMap<AccountId, u128>,
    actions: LookupMap<AccountId, u32>,
}

#[near]
impl TestDapp {
    #[init]
    pub fn new(token: AccountId) -> Self {
        Self {
            token,
            positions: LookupMap::new(StorageKey::Positions),
            actions: LookupMap::new(StorageKey::Actions),
        }
    }

    pub fn ft_on_transfer(&mut self, sender_id: AccountId, amount: U128, msg: String) -> U128 {
        if env::predecessor_account_id() != self.token {
            env::panic_str("unknown_token");
        }
        let _ = msg;
        let current = self.positions.get(&sender_id).copied().unwrap_or(0);
        self.positions
            .insert(sender_id, current.saturating_add(amount.0));
        U128(0)
    }

    /// `msg` drives the branch under test: "return" asks for the token back,
    /// "panic" fails the receiver outright, anything else keeps it.
    pub fn nft_on_transfer(
        &mut self,
        sender_id: AccountId,
        previous_owner_id: AccountId,
        token_id: String,
        msg: String,
    ) -> bool {
        let _ = (sender_id, token_id);
        match msg.as_str() {
            "panic" => env::panic_str("receiver_rejected"),
            "return" => true,
            _ => {
                let current = self.actions.get(&previous_owner_id).copied().unwrap_or(0);
                self.actions.insert(previous_owner_id, current + 1);
                false
            }
        }
    }

    pub fn get_position(&self, account_id: AccountId) -> U128 {
        U128(self.positions.get(&account_id).copied().unwrap_or(0))
    }

    pub fn record_session_action(&mut self) {
        let who = env::predecessor_account_id();
        let current = self.actions.get(&who).copied().unwrap_or(0);
        self.actions.insert(who, current.saturating_add(1));
    }

    pub fn get_session_actions(&self, account_id: AccountId) -> u32 {
        self.actions.get(&account_id).copied().unwrap_or(0)
    }
}
