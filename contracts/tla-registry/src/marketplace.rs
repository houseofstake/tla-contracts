use crate::admin::MAX_ALLOWLIST_SIZE;
use crate::asset_gate::{ft_balance_fanout, ft_balances_clear, BalanceGate, FT_BALANCE_TGAS};
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
const TRANSFER_GATE_CB_TGAS: u64 = 180;
const GAS_FOR_TRANSFER_GATE_CB: Gas = Gas::from_tgas(TRANSFER_GATE_CB_TGAS);
const _: () = assert!(
    MAX_ALLOWLIST_SIZE as u64 * FT_BALANCE_TGAS + TRANSFER_GATE_CB_TGAS + 20 <= 300,
    "the gate queries every allowlisted token before it dispatches, so widening the allowlist past what one call can fund breaks every transfer"
);

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
        let cause = self.rotation_for_receiver(&new_owner);
        if self.venues.contains(&from) {
            return Ok(self.dispatch_sub_account_transfer(
                sub_account,
                tla_id,
                name,
                from,
                new_owner,
                cause,
            ));
        }
        let allowlist: Vec<AccountId> = self.ft_allowlist.iter().cloned().collect();
        let Some(chain) = ft_balance_fanout(&allowlist, &sub_account) else {
            return Ok(self.dispatch_sub_account_transfer(
                sub_account,
                tla_id,
                name,
                from,
                new_owner,
                cause,
            ));
        };
        Ok(chain.then(
            Self::ext(env::current_account_id())
                .with_static_gas(GAS_FOR_TRANSFER_GATE_CB)
                .on_transfer_balances_checked(
                    sub_account,
                    tla_id,
                    name,
                    from,
                    new_owner,
                    cause,
                    allowlist,
                ),
        ))
    }

    #[private]
    #[handle_result]
    #[allow(clippy::too_many_arguments)]
    pub fn on_transfer_balances_checked(
        &mut self,
        sub_account: AccountId,
        tla_id: AccountId,
        name: String,
        from: AccountId,
        new_owner: AccountId,
        cause: RotationCause,
        allowlist: Vec<AccountId>,
    ) -> Result<Promise, ContractError> {
        if let BalanceGate::Blocked { token, reason } = ft_balances_clear(&allowlist) {
            Event::TransferBlockedByBalance {
                full_name: sub_account.to_string(),
                token,
                reason,
            }
            .emit();
            return Err(ContractError::SubAccountHoldsTokens);
        }
        Ok(self.dispatch_sub_account_transfer(sub_account, tla_id, name, from, new_owner, cause))
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch_sub_account_transfer(
        &self,
        sub_account: AccountId,
        tla_id: AccountId,
        name: String,
        from: AccountId,
        new_owner: AccountId,
        cause: RotationCause,
    ) -> Promise {
        ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_FORCE_TRANSFER)
            .force_transfer(
                sub_account,
                Some(new_owner.clone()),
                cause,
                Some(from.clone()),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_TRANSFER_CALLBACK)
                    .on_sub_account_transferred(tla_id, name, from, new_owner, cause),
            )
    }

    #[handle_result]
    #[payable]
    pub fn recover_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_owner: AccountId,
        expected_owner: AccountId,
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
        if from != expected_owner {
            return Err(ContractError::OwnerMoved);
        }
        if new_owner == from {
            return Err(ContractError::SameOwner);
        }
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        if new_owner == sub_account {
            return Err(ContractError::TransferToSubAccount);
        }
        if self.sub_accounts.contains_key(new_owner.as_str()) {
            return Err(ContractError::TransferToRegisteredName);
        }
        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_FORCE_TRANSFER)
            .force_transfer(
                sub_account,
                Some(new_owner.clone()),
                RotationCause::Recovery,
                None,
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
        let still_theirs = self
            .sub_accounts
            .get(&key)
            .is_some_and(|sub| sub.owner == from);
        if !matches!(swapped, Ok(true)) || !still_theirs {
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
        crate::nft::emit_nft_transfer(&from, &to, &key, None);
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
        cause: RotationCause,
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
        let payout = if cause.repoints_payout() {
            to.clone()
        } else {
            self.sub_accounts
                .get(&key)
                .map_or_else(|| to.clone(), |sub| sub.payout_account.clone())
        };
        if !self.sub_account_reassign(&key, &to, &payout) {
            Event::TransferFailed {
                full_name: key,
                from,
                to,
            }
            .emit();
            return;
        }
        crate::nft::emit_nft_transfer(&from, &to, &key, None);
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
        validate_name(name)?;
        let key = sub_account_key(tla_id, name);
        if self.reclaim_pending.contains_key(&key) {
            return Err(ContractError::ReclaimInProgress);
        }
        let sub = self
            .sub_accounts
            .get(&key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if sub.tla_id != *tla_id {
            return Err(ContractError::SubAccountTlaMismatch);
        }
        let owner = sub.owner.clone();
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
        if !self.venues.contains(&owner) {
            self.assert_not_paused()?;
            self.assert_sellable(&key, tla_id)?;
            if !self.venues.contains(new_owner)
                && self.sub_accounts.contains_key(new_owner.as_str())
            {
                return Err(ContractError::TransferToRegisteredName);
            }
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
        if !matches!(
            effective_sub_lifecycle(
                sub,
                tla,
                self.fee_config.retraction_notice_ns.0,
                &self.clock(),
                self.suspension_expiry(tla_id),
            ),
            LifecycleStatus::Active
        ) {
            return Err(ContractError::SubAccountNotSellable);
        }
        Ok(sub.owner.clone())
    }
}
