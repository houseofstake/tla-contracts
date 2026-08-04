mod error;
mod events;

use crate::error::ContractError;
use crate::events::Event;
use hos_common::OperatingState;
use near_sdk::borsh::BorshSerialize;
use near_sdk::json_types::{U128, U64};
use near_sdk::store::IterableSet;
use near_sdk::{
    env, ext_contract, near, AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise,
    PromiseError, PromiseOrValue,
};

const CONTRACT_VERSION: u8 = 1;

const GAS_FOR_ROTATE: Gas = Gas::from_tgas(30);
const GAS_FOR_ROTATE_CB: Gas = Gas::from_tgas(10);
const GAS_FOR_RESET: Gas = Gas::from_tgas(5);
const GAS_FOR_LEASE: Gas = Gas::from_tgas(8);
const GAS_FOR_BALANCE_QUERY: Gas = Gas::from_tgas(5);
const GAS_FOR_BALANCE_CB: Gas = Gas::from_tgas(105);
const GAS_FOR_STORAGE_DEPOSIT: Gas = Gas::from_tgas(10);
const GAS_FOR_STORAGE_CB: Gas = Gas::from_tgas(90);
const GAS_FOR_SWEEP_CALL: Gas = Gas::from_tgas(40);
const GAS_FOR_PAYOUT_QUERY: Gas = Gas::from_tgas(5);
const GAS_FOR_PAYOUT_CB: Gas = Gas::from_tgas(120);
const GAS_FOR_SETTLE_CB: Gas = Gas::from_tgas(8);

const STORAGE_DEPOSIT_AMOUNT: NearToken =
    NearToken::from_yoctonear(hos_common::FT_STORAGE_DEPOSIT_YOCTO);
const MIN_SWEEP_ATTACHED: NearToken =
    NearToken::from_yoctonear(hos_common::FT_STORAGE_DEPOSIT_YOCTO + 1);

#[allow(dead_code)]
#[ext_contract(ext_ft)]
trait FungibleToken {
    fn ft_balance_of(&self, account_id: AccountId) -> U128;
    fn storage_deposit(&mut self, account_id: Option<AccountId>, registration_only: Option<bool>);
}

#[allow(dead_code)]
#[ext_contract(ext_wallet)]
trait TenantWallet {
    fn hos_set_lease(&mut self, lease_until_ns: U64, state: OperatingState);
    fn hos_transfer_ownership(&mut self, to: Option<AccountId>);
    fn hos_sweep_near(&mut self);
    fn hos_sweep_ft(&mut self, ft: AccountId, amount: U128);
    fn hos_payout_account(&self) -> AccountId;
}

const EXTENSION_CALL_DEPOSIT: NearToken = NearToken::from_yoctonear(1);

#[allow(dead_code)]
#[ext_contract(ext_mpc_recovery)]
trait MpcRecovery {
    fn on_wallet_transferred(&mut self, wallet: AccountId);
}

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Admins,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct HosExtension {
    pub(crate) admins: IterableSet<AccountId>,
    pub(crate) registry: AccountId,
    pub(crate) recovery: AccountId,
    pub(crate) paused: bool,
    pub(crate) version: u8,
}

#[near]
impl HosExtension {
    #[init]
    pub fn new(admin: AccountId, registry: AccountId, recovery: AccountId) -> Self {
        let mut admins = IterableSet::new(StorageKey::Admins);
        admins.insert(admin);
        Self {
            admins,
            registry,
            recovery,
            paused: false,
            version: CONTRACT_VERSION,
        }
    }

    #[handle_result]
    pub fn pause(&mut self) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.paused = true;
        Event::ContractPaused {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn unpause(&mut self) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.paused = false;
        Event::ContractUnpaused {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[payable]
    #[handle_result]
    pub fn add_admin(&mut self, account: AccountId) -> Result<(), ContractError> {
        self.assert_one_yocto()?;
        self.assert_admin()?;
        if !self.admins.insert(account.clone()) {
            return Ok(());
        }
        Event::AdminAdded {
            account,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[payable]
    #[handle_result]
    pub fn remove_admin(&mut self, account: AccountId) -> Result<(), ContractError> {
        self.assert_one_yocto()?;
        self.assert_admin()?;
        if self.admins.len() <= 1 {
            return Err(ContractError::CannotRemoveLastAdmin);
        }
        if !self.admins.remove(&account) {
            return Ok(());
        }
        Event::AdminRemoved {
            account,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[payable]
    #[handle_result]
    pub fn upgrade(&mut self, code: Vec<u8>) -> Result<Promise, ContractError> {
        self.assert_one_yocto()?;
        self.assert_admin()?;
        if code.is_empty() {
            return Err(ContractError::EmptyCode);
        }
        Event::Upgraded {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(Promise::new(env::current_account_id()).deploy_contract(code))
    }

    #[handle_result]
    pub fn skim(&mut self, amount: U128, to: AccountId) -> Result<Promise, ContractError> {
        self.assert_admin()?;
        let reserve = env::storage_byte_cost()
            .as_yoctonear()
            .saturating_mul(env::storage_usage() as u128);
        let available = env::account_balance()
            .as_yoctonear()
            .saturating_sub(reserve);
        if amount.0 > available {
            return Err(ContractError::InsufficientBalance);
        }
        Event::BalanceSkimmed {
            amount,
            to: to.clone(),
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(Promise::new(to).transfer(NearToken::from_yoctonear(amount.0)))
    }

    #[handle_result]
    pub fn push_lease(
        &mut self,
        wallet: AccountId,
        lease_until_ns: U64,
        state: OperatingState,
    ) -> Result<Promise, ContractError> {
        self.assert_registry()?;
        self.assert_not_paused()?;
        Ok(ext_wallet::ext(wallet)
            .with_static_gas(GAS_FOR_LEASE)
            .with_attached_deposit(EXTENSION_CALL_DEPOSIT)
            .hos_set_lease(lease_until_ns, state))
    }

    #[handle_result]
    pub fn force_transfer(
        &mut self,
        wallet: AccountId,
        new_owner: Option<AccountId>,
        park: bool,
    ) -> Result<Promise, ContractError> {
        self.assert_registry()?;
        self.assert_not_paused()?;
        if park && new_owner.is_some() {
            return Err(ContractError::ParkTakesNoOwner);
        }
        if !park && new_owner.is_none() {
            return Err(ContractError::TransferNeedsOwner);
        }
        Event::ForceTransferRequested {
            wallet: wallet.clone(),
            new_owner: new_owner.clone(),
            park,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(ext_wallet::ext(wallet.clone())
            .with_static_gas(GAS_FOR_ROTATE)
            .with_attached_deposit(EXTENSION_CALL_DEPOSIT)
            .hos_transfer_ownership(new_owner)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_ROTATE_CB)
                    .after_force_swap(wallet),
            ))
    }

    #[private]
    pub fn after_force_swap(
        &mut self,
        wallet: AccountId,
        #[callback_result] swapped: Result<(), PromiseError>,
    ) -> bool {
        let transferred = swapped.is_ok();
        if transferred {
            Event::ForceTransferCompleted {
                wallet: wallet.clone(),
            }
            .emit();
            let _ = ext_mpc_recovery::ext(self.recovery.clone())
                .with_static_gas(GAS_FOR_RESET)
                .on_wallet_transferred(wallet);
        } else {
            Event::ForceTransferVoided { wallet }.emit();
        }
        transferred
    }

    #[payable]
    #[handle_result]
    pub fn sweep_near(&mut self, wallet: AccountId) -> Result<Promise, ContractError> {
        self.assert_registry()?;
        self.assert_not_paused()?;
        Event::NearSweepRequested {
            wallet: wallet.clone(),
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(ext_wallet::ext(wallet)
            .with_static_gas(GAS_FOR_SWEEP_CALL)
            .with_attached_deposit(EXTENSION_CALL_DEPOSIT)
            .hos_sweep_near())
    }

    #[payable]
    #[handle_result]
    pub fn sweep_ft(&mut self, wallet: AccountId, ft: AccountId) -> Result<Promise, ContractError> {
        self.assert_registry()?;
        self.assert_not_paused()?;
        if env::attached_deposit() != MIN_SWEEP_ATTACHED {
            return Err(ContractError::InsufficientDeposit);
        }
        Event::SweepRequested {
            wallet: wallet.clone(),
            ft: ft.clone(),
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(ext_wallet::ext(wallet.clone())
            .with_static_gas(GAS_FOR_PAYOUT_QUERY)
            .hos_payout_account()
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_PAYOUT_CB)
                    .after_payout_for_sweep(wallet, ft),
            ))
    }

    #[private]
    pub fn after_payout_for_sweep(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        #[callback_result] payout: Result<AccountId, PromiseError>,
    ) -> PromiseOrValue<bool> {
        let Ok(destination) = payout else {
            return self.abort_and_refund(Event::SweepFailed {
                wallet,
                ft,
                reason: "payout_query_failed".to_string(),
            });
        };
        PromiseOrValue::Promise(
            ext_ft::ext(ft.clone())
                .with_static_gas(GAS_FOR_BALANCE_QUERY)
                .ft_balance_of(wallet.clone())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_FOR_BALANCE_CB)
                        .after_balance_for_sweep(wallet, ft, destination),
                ),
        )
    }

    #[private]
    pub fn after_balance_for_sweep(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        #[callback_result] balance: Result<U128, PromiseError>,
    ) -> PromiseOrValue<bool> {
        let balance = match balance {
            Ok(v) => v.0,
            Err(_) => {
                return self.abort_and_refund(Event::SweepSkipped {
                    wallet,
                    ft,
                    reason: "balance_query_failed".to_string(),
                });
            }
        };
        if balance == 0 {
            return self.abort_and_refund(Event::SweepSkipped {
                wallet,
                ft,
                reason: "zero_balance".to_string(),
            });
        }
        PromiseOrValue::Promise(
            ext_ft::ext(ft.clone())
                .with_static_gas(GAS_FOR_STORAGE_DEPOSIT)
                .with_attached_deposit(STORAGE_DEPOSIT_AMOUNT)
                .storage_deposit(Some(destination.clone()), Some(true))
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_FOR_STORAGE_CB)
                        .after_storage_for_sweep(wallet, ft, destination, U128(balance)),
                ),
        )
    }

    #[private]
    pub fn after_storage_for_sweep(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        balance: U128,
    ) -> PromiseOrValue<bool> {
        if !near_sdk::is_promise_success() {
            return self.abort_and_refund(Event::SweepFailed {
                wallet,
                ft,
                reason: "storage_deposit_failed".to_string(),
            });
        }
        PromiseOrValue::Promise(
            ext_wallet::ext(wallet.clone())
                .with_static_gas(GAS_FOR_SWEEP_CALL)
                .with_attached_deposit(EXTENSION_CALL_DEPOSIT)
                .hos_sweep_ft(ft.clone(), balance)
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_FOR_SETTLE_CB)
                        .after_sweep_settled(wallet, ft, destination, balance),
                ),
        )
    }

    #[private]
    pub fn after_sweep_settled(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        amount: U128,
        #[callback_result] settled: Result<(), PromiseError>,
    ) -> bool {
        if settled.is_ok() {
            Event::SweepDispatched {
                wallet,
                ft,
                destination,
                amount,
            }
            .emit();
            true
        } else {
            Event::SweepFailed {
                wallet,
                ft,
                reason: "authority_execute_failed".to_string(),
            }
            .emit();
            false
        }
    }

    pub fn get_version(&self) -> u8 {
        self.version
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn get_admins(&self) -> Vec<AccountId> {
        self.admins.iter().cloned().collect()
    }

    pub fn get_registry(&self) -> AccountId {
        self.registry.clone()
    }

    pub fn get_recovery(&self) -> AccountId {
        self.recovery.clone()
    }

    pub fn min_sweep_attached(&self) -> U128 {
        U128(MIN_SWEEP_ATTACHED.as_yoctonear())
    }
}

impl HosExtension {
    fn assert_admin(&self) -> Result<(), ContractError> {
        if !self.admins.contains(&env::predecessor_account_id()) {
            return Err(ContractError::OnlyAdmin);
        }
        Ok(())
    }

    fn assert_one_yocto(&self) -> Result<(), ContractError> {
        if env::attached_deposit() != NearToken::from_yoctonear(1) {
            return Err(ContractError::RequiresOneYocto);
        }
        Ok(())
    }

    fn assert_registry(&self) -> Result<(), ContractError> {
        if env::predecessor_account_id() != self.registry {
            return Err(ContractError::OnlyRegistry);
        }
        Ok(())
    }

    fn assert_not_paused(&self) -> Result<(), ContractError> {
        if self.paused {
            return Err(ContractError::Paused);
        }
        Ok(())
    }

    fn abort_and_refund(&self, event: Event) -> PromiseOrValue<bool> {
        event.emit();
        let _ = Promise::new(self.registry.clone()).transfer(MIN_SWEEP_ATTACHED);
        PromiseOrValue::Value(false)
    }
}

#[cfg(test)]
mod tests;
