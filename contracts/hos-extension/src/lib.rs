mod error;
mod events;

use crate::error::ContractError;
use crate::events::Event;
use hos_common::{OperatingState, RotationCause};
use near_sdk::borsh::BorshSerialize;
use near_sdk::json_types::{Base58CryptoHash, Base64VecU8, U128, U64};
use near_sdk::store::IterableSet;
use near_sdk::{
    env, ext_contract, near, AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise,
    PromiseError, PromiseOrValue, PublicKey,
};

const CONTRACT_VERSION: u8 = 1;
use hos_common::MAX_AUTHORITY_HOLD_NS;
const UPGRADE_DELAY_NS: u64 = 48 * 60 * 60 * 1_000_000_000;

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
    fn hos_transfer_ownership(
        &mut self,
        to: Option<AccountId>,
        cause: RotationCause,
        asked_by: Option<AccountId>,
    );
    fn hos_sweep_near(&mut self);
    fn hos_sweep_ft(&mut self, ft: AccountId, amount: U128);
    fn hos_payout_account(&self) -> AccountId;
    fn hos_set_payout_account(&mut self, payout_account: AccountId, expected_owner: AccountId);
    fn hos_migrate(collection_id: AccountId);
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
    pub(crate) treasury: AccountId,
    pub(crate) approved_code_hash: Option<[u8; 32]>,
    pub(crate) approved_at: Option<u64>,
    pub(crate) council: AccountId,
    pub(crate) paused_until_ns: u64,
}

#[near(serializers = [borsh])]
pub struct LegacyHosExtension {
    admins: IterableSet<AccountId>,
    registry: AccountId,
    recovery: AccountId,
    paused: bool,
    version: u8,
    treasury: AccountId,
    approved_code_hash: Option<[u8; 32]>,
    approved_at: Option<u64>,
    council: AccountId,
}

#[near]
impl HosExtension {
    #[init]
    pub fn new(
        admin: AccountId,
        registry: AccountId,
        recovery: AccountId,
        treasury: AccountId,
        council: AccountId,
    ) -> Self {
        near_sdk::require!(
            council != env::current_account_id(),
            "council must not be this account, which ends with no keys"
        );
        let mut admins = IterableSet::new(StorageKey::Admins);
        admins.insert(admin);
        Self {
            admins,
            registry,
            recovery,
            paused: false,
            version: CONTRACT_VERSION,
            treasury,
            approved_code_hash: None,
            approved_at: None,
            council,
            paused_until_ns: 0,
        }
    }

    #[private]
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        Event::Upgraded {
            by: env::predecessor_account_id(),
        }
        .emit();
        let Some(old) = hos_common::try_state_read::<LegacyHosExtension>() else {
            return hos_common::try_state_read::<Self>()
                .unwrap_or_else(|| env::panic_str("no state to migrate"));
        };
        Self {
            admins: old.admins,
            registry: old.registry,
            recovery: old.recovery,
            paused: old.paused,
            version: CONTRACT_VERSION,
            treasury: old.treasury,
            approved_code_hash: old.approved_code_hash,
            approved_at: old.approved_at,
            council: old.council,
            paused_until_ns: if old.paused {
                env::block_timestamp().saturating_add(MAX_AUTHORITY_HOLD_NS)
            } else {
                0
            },
        }
    }

    pub fn get_council(&self) -> AccountId {
        self.council.clone()
    }

    #[handle_result]
    pub fn pause(&mut self) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.paused = true;
        self.paused_until_ns = env::block_timestamp().saturating_add(MAX_AUTHORITY_HOLD_NS);
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
        self.paused_until_ns = 0;
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
        self.assert_council()?;
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
        self.assert_council()?;
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
    pub fn approve_upgrade(&mut self, code_hash: Base58CryptoHash) -> Result<(), ContractError> {
        self.assert_one_yocto()?;
        self.assert_council()?;
        self.approved_code_hash = Some(code_hash.into());
        self.approved_at = Some(env::block_timestamp());
        Event::UpgradeApproved {
            hash: (&code_hash).into(),
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[payable]
    #[handle_result]
    pub fn upgrade(&mut self, code: Base64VecU8) -> Result<Promise, ContractError> {
        self.assert_one_yocto()?;
        self.assert_admin()?;
        let code = code.0;
        if code.is_empty() {
            return Err(ContractError::EmptyCode);
        }
        let approved = self
            .approved_code_hash
            .ok_or(ContractError::NoApprovedHash)?;
        if env::sha256_array(&code) != approved {
            return Err(ContractError::HashMismatch);
        }
        let approved_at = self.approved_at.ok_or(ContractError::NoApprovedHash)?;
        if env::block_timestamp() < approved_at.saturating_add(UPGRADE_DELAY_NS) {
            return Err(ContractError::ApprovalTooYoung);
        }
        self.approved_code_hash = None;
        self.approved_at = None;
        Ok(hos_common::deploy_and_migrate(code))
    }

    #[payable]
    #[handle_result]
    pub fn seal(&mut self, public_key: PublicKey) -> Result<Promise, ContractError> {
        self.assert_one_yocto()?;
        self.assert_council()?;
        Event::Sealed {
            public_key: (&public_key).into(),
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(Promise::new(env::current_account_id()).delete_key(public_key))
    }

    #[payable]
    #[handle_result]
    pub fn skim(&mut self, amount: U128) -> Result<Promise, ContractError> {
        self.assert_one_yocto()?;
        self.assert_admin()?;
        let to = self.treasury.clone();
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
    pub fn set_payout(
        &mut self,
        wallet: AccountId,
        payout_account: AccountId,
        expected_owner: AccountId,
    ) -> Result<Promise, ContractError> {
        self.assert_registry()?;
        self.assert_not_paused()?;
        Ok(ext_wallet::ext(wallet)
            .with_static_gas(GAS_FOR_LEASE)
            .with_attached_deposit(EXTENSION_CALL_DEPOSIT)
            .hos_set_payout_account(payout_account, expected_owner))
    }

    #[handle_result]
    pub fn migrate_wallet(&mut self, wallet: AccountId) -> Result<Promise, ContractError> {
        self.assert_council()?;
        Ok(ext_wallet::ext(wallet)
            .with_static_gas(GAS_FOR_LEASE)
            .hos_migrate(self.registry.clone()))
    }

    #[handle_result]
    pub fn force_transfer(
        &mut self,
        wallet: AccountId,
        new_owner: Option<AccountId>,
        cause: RotationCause,
        asked_by: Option<AccountId>,
    ) -> Result<Promise, ContractError> {
        self.assert_registry()?;
        self.assert_not_paused()?;
        if cause.parks() && new_owner.is_some() {
            return Err(ContractError::ParkTakesNoOwner);
        }
        if !cause.parks() && new_owner.is_none() {
            return Err(ContractError::TransferNeedsOwner);
        }
        if cause.needs_holder() && asked_by.is_none() {
            return Err(ContractError::TransferNeedsHolder);
        }
        Event::ForceTransferRequested {
            wallet: wallet.clone(),
            new_owner: new_owner.clone(),
            park: cause.parks(),
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(ext_wallet::ext(wallet.clone())
            .with_static_gas(GAS_FOR_ROTATE)
            .with_attached_deposit(EXTENSION_CALL_DEPOSIT)
            .hos_transfer_ownership(new_owner, cause, asked_by)
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
        self.effective_paused()
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

    pub fn approved_upgrade_hash(&self) -> Option<Base58CryptoHash> {
        self.approved_code_hash.map(Base58CryptoHash::from)
    }

    pub fn approved_upgrade_at(&self) -> Option<U64> {
        self.approved_at.map(U64)
    }

    pub fn upgrade_delay_ns(&self) -> U64 {
        U64(UPGRADE_DELAY_NS)
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

    fn assert_council(&self) -> Result<(), ContractError> {
        if env::predecessor_account_id() != self.council {
            return Err(ContractError::OnlyCouncil);
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
        if self.effective_paused() {
            return Err(ContractError::Paused);
        }
        Ok(())
    }

    fn effective_paused(&self) -> bool {
        self.paused && env::block_timestamp() < self.paused_until_ns
    }

    fn abort_and_refund(&self, event: Event) -> PromiseOrValue<bool> {
        event.emit();
        let _ = Promise::new(self.registry.clone()).transfer(MIN_SWEEP_ATTACHED);
        PromiseOrValue::Value(false)
    }
}

#[cfg(test)]
mod tests;
