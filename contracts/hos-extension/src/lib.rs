mod error;
mod events;
mod legacy;

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
const STATE_VERSION: u16 = 2;
use hos_common::MAX_AUTHORITY_HOLD_NS;
const UPGRADE_DELAY_NS: u64 = 48 * 60 * 60 * 1_000_000_000;

const ROTATE_CB_TGAS: u64 = 20;
const RESET_TGAS: u64 = 5;
const RESET_CALLBACK_TGAS: u64 = 5;
const ROTATE_CB_FRAME_TGAS: u64 = 8;
const GAS_FOR_ROTATE: Gas = Gas::from_tgas(30);
const GAS_FOR_ROTATE_CB: Gas = Gas::from_tgas(ROTATE_CB_TGAS);
const GAS_FOR_RESET: Gas = Gas::from_tgas(RESET_TGAS);
const GAS_FOR_RESET_CALLBACK: Gas = Gas::from_tgas(RESET_CALLBACK_TGAS);
const _: () = assert!(
    RESET_TGAS + RESET_CALLBACK_TGAS + ROTATE_CB_FRAME_TGAS <= ROTATE_CB_TGAS,
    "after_force_swap schedules the recovery reset out of its own static gas, so a budget that only covers the reservations leaves the rotation to survive on whatever the caller happens to leave unspent"
);
const GAS_FOR_LEASE: Gas = Gas::from_tgas(8);
const GAS_FOR_BALANCE_QUERY: Gas = Gas::from_tgas(5);
const GAS_FOR_BALANCE_CB: Gas = Gas::from_tgas(105);
const GAS_FOR_STORAGE_DEPOSIT: Gas = Gas::from_tgas(10);
const GAS_FOR_STORAGE_CB: Gas = Gas::from_tgas(90);
const GAS_FOR_SWEEP_CALL: Gas = Gas::from_tgas(40);
const GAS_FOR_PAYOUT_QUERY: Gas = Gas::from_tgas(5);
const GAS_FOR_PAYOUT_CB: Gas = Gas::from_tgas(120);
const GAS_FOR_SETTLE_CB: Gas = Gas::from_tgas(8);

/// Held back by `skim` on top of storage already used. A reset that cannot be
/// recorded is a reset nobody retries, so the pending set has to have room to
/// grow after a treasury sweep has taken everything else.
const SKIM_BUFFER: NearToken = NearToken::from_millinear(100);
const MAX_PAGE_LIMIT: u64 = 500;
const MAX_ADMINS: u32 = 32;
const GAS_FOR_SKIM_CB: Gas = Gas::from_tgas(5);
const GAS_FOR_SEAL_CB: Gas = Gas::from_tgas(5);

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
    fn hos_retract_lease(&mut self, lease_until_ns: U64);
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
    fn on_wallet_transferred(&mut self, wallet: AccountId) -> bool;
}

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Admins,
    RecoveryResetPending,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct HosExtension {
    pub(crate) state_version: u16,
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
    pub(crate) recovery_reset_pending: IterableSet<AccountId>,
    pub(crate) upgrade_proven: bool,
    pub(crate) pending_council: Option<AccountId>,
    pub(crate) pending_council_at: Option<u64>,
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
            state_version: STATE_VERSION,
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
            recovery_reset_pending: IterableSet::new(StorageKey::RecoveryResetPending),
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
            Some(1) => hos_common::try_state_read::<legacy::HosExtensionV1>()
                .map(Self::from)
                .unwrap_or_else(|| env::panic_str(error::NO_STATE)),
            Some(_) => env::panic_str(error::STATE_VERSION_UNKNOWN),
            None => env::panic_str(error::NO_STATE),
        };
        current.version = CONTRACT_VERSION;
        current.upgrade_proven = true;
        Event::Upgraded {
            by: env::predecessor_account_id(),
        }
        .emit();
        current
    }

    pub fn get_council(&self) -> AccountId {
        self.council.clone()
    }

    #[payable]
    #[handle_result]
    pub fn approve_council_rotation(
        &mut self,
        new_council: AccountId,
    ) -> Result<(), ContractError> {
        self.assert_one_yocto()?;
        self.assert_council()?;
        if new_council == self.council {
            return Err(ContractError::CouncilUnchanged);
        }
        if new_council == env::current_account_id() {
            return Err(ContractError::CouncilIsSelf);
        }
        self.pending_council = Some(new_council.clone());
        self.pending_council_at = Some(env::block_timestamp());
        Event::CouncilRotationApproved {
            new_council,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[payable]
    #[handle_result]
    pub fn cancel_council_rotation(&mut self) -> Result<(), ContractError> {
        self.assert_one_yocto()?;
        self.assert_council()?;
        if self.pending_council.take().is_none() {
            return Err(ContractError::NoCouncilRotationPending);
        }
        self.pending_council_at = None;
        Event::CouncilRotationCancelled {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[payable]
    #[handle_result]
    pub fn commit_council_rotation(&mut self) -> Result<(), ContractError> {
        self.assert_one_yocto()?;
        let pending = self
            .pending_council
            .clone()
            .ok_or(ContractError::NoCouncilRotationPending)?;
        if env::predecessor_account_id() != pending {
            return Err(ContractError::OnlyPendingCouncil);
        }
        let approved_at = self
            .pending_council_at
            .ok_or(ContractError::NoCouncilRotationPending)?;
        if env::block_timestamp() < approved_at.saturating_add(UPGRADE_DELAY_NS) {
            return Err(ContractError::CouncilRotationTooYoung);
        }
        self.council = pending.clone();
        self.pending_council = None;
        self.pending_council_at = None;
        Event::CouncilRotated {
            new_council: pending,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    pub fn pending_council(&self) -> Option<(AccountId, U64)> {
        self.pending_council
            .clone()
            .zip(self.pending_council_at.map(U64))
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
        if self.admins.len() >= MAX_ADMINS {
            return Err(ContractError::AdminSetFull);
        }
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

    #[private]
    pub fn after_recovery_reset(
        &mut self,
        wallet: AccountId,
        #[callback_result] reset: Result<bool, PromiseError>,
    ) {
        if matches!(reset, Ok(true)) {
            self.recovery_reset_pending.remove(&wallet);
            return;
        }
        self.recovery_reset_pending.insert(wallet.clone());
        Event::RecoveryResetPending { wallet }.emit();
    }

    #[handle_result]
    pub fn retry_recovery_reset(&mut self, wallet: AccountId) -> Result<Promise, ContractError> {
        if !self.recovery_reset_pending.contains(&wallet) {
            return Err(ContractError::NoPendingReset);
        }
        Ok(ext_mpc_recovery::ext(self.recovery.clone())
            .with_static_gas(GAS_FOR_RESET)
            .on_wallet_transferred(wallet.clone())
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_RESET_CALLBACK)
                    .after_recovery_reset(wallet),
            ))
    }

    pub fn pending_recovery_resets(
        &self,
        from_index: Option<u64>,
        limit: Option<u64>,
    ) -> Vec<AccountId> {
        let skip = usize::try_from(u128::from(from_index.unwrap_or(0))).unwrap_or(usize::MAX);
        let take = limit.unwrap_or(MAX_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT);
        self.recovery_reset_pending
            .iter()
            .skip(skip)
            .take(take as usize)
            .cloned()
            .collect()
    }

    pub fn pending_recovery_reset_count(&self) -> u32 {
        self.recovery_reset_pending.len()
    }

    #[payable]
    #[handle_result]
    pub fn seal(&mut self, public_key: PublicKey) -> Result<Promise, ContractError> {
        self.assert_one_yocto()?;
        self.assert_council()?;
        if !self.upgrade_proven {
            return Err(ContractError::UpgradeNotProven);
        }
        let by = env::predecessor_account_id();
        let key: String = (&public_key).into();
        Ok(Promise::new(env::current_account_id())
            .delete_key(public_key)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_SEAL_CB)
                    .after_seal(key, by),
            ))
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

    #[payable]
    #[handle_result]
    pub fn skim(&mut self, amount: U128) -> Result<Promise, ContractError> {
        self.assert_one_yocto()?;
        self.assert_admin()?;
        let to = self.treasury.clone();
        let reserve = env::storage_byte_cost()
            .as_yoctonear()
            .saturating_mul(env::storage_usage() as u128)
            .saturating_add(SKIM_BUFFER.as_yoctonear());
        let available = env::account_balance()
            .as_yoctonear()
            .saturating_sub(reserve);
        if amount.0 > available {
            return Err(ContractError::InsufficientBalance);
        }
        let by = env::predecessor_account_id();
        Ok(Promise::new(to.clone())
            .transfer(NearToken::from_yoctonear(amount.0))
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_SKIM_CB)
                    .after_skim(amount, to, by),
            ))
    }

    #[private]
    pub fn after_skim(&mut self, amount: U128, to: AccountId, by: AccountId) -> bool {
        if !near_sdk::is_promise_success() {
            Event::SkimFailed { amount, to, by }.emit();
            return false;
        }
        Event::BalanceSkimmed { amount, to, by }.emit();
        true
    }

    #[handle_result]
    pub fn push_lease(
        &mut self,
        wallet: AccountId,
        lease_until_ns: U64,
        state: OperatingState,
    ) -> Result<Promise, ContractError> {
        self.assert_registry()?;
        Ok(ext_wallet::ext(wallet)
            .with_static_gas(GAS_FOR_LEASE)
            .with_attached_deposit(EXTENSION_CALL_DEPOSIT)
            .hos_set_lease(lease_until_ns, state))
    }

    #[handle_result]
    pub fn retract_lease(
        &mut self,
        wallet: AccountId,
        lease_until_ns: U64,
    ) -> Result<Promise, ContractError> {
        self.assert_registry()?;
        Ok(ext_wallet::ext(wallet)
            .with_static_gas(GAS_FOR_LEASE)
            .with_attached_deposit(EXTENSION_CALL_DEPOSIT)
            .hos_retract_lease(lease_until_ns))
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
                .on_wallet_transferred(wallet.clone())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_FOR_RESET_CALLBACK)
                        .after_recovery_reset(wallet),
                );
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
    pub fn sweep_ft(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        refund_to: AccountId,
    ) -> Result<Promise, ContractError> {
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
                    .after_payout_for_sweep(wallet, ft, refund_to),
            ))
    }

    #[private]
    pub fn after_payout_for_sweep(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        refund_to: AccountId,
        #[callback_result] payout: Result<AccountId, PromiseError>,
    ) -> PromiseOrValue<bool> {
        let Ok(destination) = payout else {
            return self.abort_and_refund(
                Event::SweepFailed {
                    wallet,
                    ft,
                    reason: "payout_query_failed".to_string(),
                },
                refund_to,
            );
        };
        PromiseOrValue::Promise(
            ext_ft::ext(ft.clone())
                .with_static_gas(GAS_FOR_BALANCE_QUERY)
                .ft_balance_of(wallet.clone())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_FOR_BALANCE_CB)
                        .after_balance_for_sweep(wallet, ft, destination, refund_to),
                ),
        )
    }

    #[private]
    pub fn after_balance_for_sweep(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        refund_to: AccountId,
        #[callback_result] balance: Result<U128, PromiseError>,
    ) -> PromiseOrValue<bool> {
        let balance = match balance {
            Ok(v) => v.0,
            Err(_) => {
                return self.abort_and_refund(
                    Event::SweepSkipped {
                        wallet,
                        ft,
                        reason: "balance_query_failed".to_string(),
                    },
                    refund_to,
                );
            }
        };
        if balance == 0 {
            return self.abort_and_refund(
                Event::SweepSkipped {
                    wallet,
                    ft,
                    reason: "zero_balance".to_string(),
                },
                refund_to,
            );
        }
        PromiseOrValue::Promise(
            ext_ft::ext(ft.clone())
                .with_static_gas(GAS_FOR_STORAGE_DEPOSIT)
                .with_attached_deposit(STORAGE_DEPOSIT_AMOUNT)
                .storage_deposit(Some(destination.clone()), Some(true))
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_FOR_STORAGE_CB)
                        .after_storage_for_sweep(wallet, ft, destination, refund_to, U128(balance)),
                ),
        )
    }

    #[private]
    pub fn after_storage_for_sweep(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        refund_to: AccountId,
        balance: U128,
    ) -> PromiseOrValue<bool> {
        if !near_sdk::is_promise_success() {
            return self.abort_and_refund(
                Event::SweepFailed {
                    wallet,
                    ft,
                    reason: "storage_deposit_failed".to_string(),
                },
                refund_to,
            );
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

    fn abort_and_refund(&self, event: Event, refund_to: AccountId) -> PromiseOrValue<bool> {
        event.emit();
        let _ = Promise::new(refund_to).transfer(MIN_SWEEP_ATTACHED);
        PromiseOrValue::Value(false)
    }
}

#[cfg(test)]
mod tests;
