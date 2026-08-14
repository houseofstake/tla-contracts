use crate::error::ContractError;
use crate::events::Event;
use crate::interfaces::ext_hos_extension;
use crate::lifecycle::effective_sub_lifecycle;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use hos_common::RotationCause;
use near_sdk::{env, near, AccountId, Gas, Promise};

pub(crate) const GAS_FOR_FORCE_TRANSFER: Gas = Gas::from_tgas(45);
pub(crate) const GAS_FOR_TRANSFER_CALLBACK: Gas = Gas::from_tgas(20);

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn transfer_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_owner: AccountId,
    ) -> Result<Promise, ContractError> {
        let (sub_account, from) = self.assert_transferable(&tla_id, &name, &new_owner)?;
        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_FORCE_TRANSFER)
            .force_transfer(
                sub_account,
                Some(new_owner.clone()),
                RotationCause::Transfer,
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_TRANSFER_CALLBACK)
                    .on_sub_account_transferred(tla_id, name, from, new_owner),
            ))
    }

    #[handle_result]
    #[payable]
    pub fn recover_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_owner: AccountId,
    ) -> Result<Promise, ContractError> {
        crate::assert_one_yocto()?;
        self.assert_recovery_authority()?;
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        if self.reclaim_pending.contains_key(&key) {
            return Err(ContractError::ReclaimInProgress);
        }
        let from = self
            .sub_accounts
            .get(&key)
            .ok_or(ContractError::SubAccountNotFound)?
            .owner
            .clone();
        if new_owner == from {
            return Err(ContractError::SameOwner);
        }
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        if new_owner == sub_account {
            return Err(ContractError::TransferToSubAccount);
        }
        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_FORCE_TRANSFER)
            .force_transfer(
                sub_account,
                Some(new_owner.clone()),
                RotationCause::Recovery,
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_TRANSFER_CALLBACK)
                    .on_sub_account_recovered(tla_id, name, from, new_owner),
            ))
    }

    #[private]
    pub fn on_sub_account_recovered(
        &mut self,
        tla_id: AccountId,
        name: String,
        from: AccountId,
        to: AccountId,
        #[callback_result] swapped: Result<bool, near_sdk::PromiseError>,
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !matches!(swapped, Ok(true)) {
            Event::TransferFailed {
                full_name: key,
                from,
                to,
            }
            .emit();
            return;
        }
        if !self.sub_account_reassign(&key, &to, &to) {
            Event::TransferFailed {
                full_name: key,
                from,
                to,
            }
            .emit();
            return;
        }
        self.emit_activity(Event::SubAccountRecovered {
            full_name: key,
            tla_id,
            from,
            to,
        });
    }

    #[private]
    pub fn on_sub_account_transferred(
        &mut self,
        tla_id: AccountId,
        name: String,
        from: AccountId,
        to: AccountId,
        #[callback_result] swapped: Result<bool, near_sdk::PromiseError>,
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !matches!(swapped, Ok(true)) {
            Event::TransferFailed {
                full_name: key,
                from,
                to,
            }
            .emit();
            return;
        }
        if !self.sub_account_reassign(&key, &to, &to) {
            Event::TransferFailed {
                full_name: key,
                from,
                to,
            }
            .emit();
            return;
        }
        self.emit_activity(Event::SubAccountTransferred {
            full_name: key,
            tla_id,
            from,
            to,
        });
    }

    pub(crate) fn assert_transferable(
        &self,
        tla_id: &AccountId,
        name: &str,
        new_owner: &AccountId,
    ) -> Result<(AccountId, AccountId), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        validate_name(name)?;
        let key = sub_account_key(tla_id, name);
        if self.reclaim_pending.contains_key(&key) {
            return Err(ContractError::ReclaimInProgress);
        }
        let owner = self.assert_sellable(&key, tla_id)?;
        if env::predecessor_account_id() != owner {
            return Err(ContractError::OnlyOwner);
        }
        if *new_owner == owner {
            return Err(ContractError::SameOwner);
        }
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        if *new_owner == sub_account {
            return Err(ContractError::TransferToSubAccount);
        }
        Ok((sub_account, owner))
    }

    pub(crate) fn assert_sellable(
        &self,
        key: &str,
        tla_id: &AccountId,
    ) -> Result<AccountId, ContractError> {
        if self.reclaim_pending.contains_key(key) {
            return Err(ContractError::ReclaimInProgress);
        }
        let sub = self
            .sub_accounts
            .get(key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if sub.tla_id != *tla_id {
            return Err(ContractError::SubAccountTlaMismatch);
        }
        if sub.retraction_at.is_some() {
            return Err(ContractError::RetractionPending);
        }
        let tla = self.tlas.get(tla_id).ok_or(ContractError::TlaNotFound)?;
        if tla.tla_type == TlaType::Business {
            return Err(ContractError::BusinessSubNotResellable);
        }
        if tla.status != TlaStatus::Active {
            return Err(ContractError::SubAccountNotSellable);
        }
        if !matches!(
            effective_sub_lifecycle(
                sub,
                tla,
                self.fee_config.retraction_notice_ns.0,
                &self.clock(),
            ),
            LifecycleStatus::Active
        ) {
            return Err(ContractError::SubAccountNotSellable);
        }
        Ok(sub.owner.clone())
    }
}
