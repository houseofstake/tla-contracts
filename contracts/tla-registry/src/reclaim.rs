use crate::admin::MAX_ALLOWLIST_SIZE;
use crate::asset_gate::{
    ft_balance_fanout, ft_balances_clear, BalanceGate, FT_BALANCE_TGAS, GATE_CALLER_FRAME_TGAS,
};
use crate::error::ContractError;
use crate::events::Event;
use crate::interfaces::ext_hos_extension;
use crate::lifecycle::effective_sub_lifecycle;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use hos_common::RotationCause;
use near_sdk::{env, near, AccountId, Gas, NearToken, Promise, PromiseError, PromiseOrValue};

const GAS_FOR_HOS_SWEEP: Gas = Gas::from_tgas(120);
const FORCE_TRANSFER_TGAS: u64 = 45;
const FINALIZE_CB_TGAS: u64 = 10;
const PARK_FRAME_TGAS: u64 = 25;
const GAS_FOR_HOS_FORCE_TRANSFER: Gas = Gas::from_tgas(FORCE_TRANSFER_TGAS);
const GAS_FOR_FINALIZE_CB: Gas = Gas::from_tgas(FINALIZE_CB_TGAS);
const PARK_CHAIN_TGAS: u64 = FORCE_TRANSFER_TGAS + FINALIZE_CB_TGAS;
const BALANCES_CB_TOTAL_TGAS: u64 = PARK_CHAIN_TGAS + PARK_FRAME_TGAS;
const GAS_FOR_BALANCES_CB_TOTAL: Gas = Gas::from_tgas(BALANCES_CB_TOTAL_TGAS);
const _: () = assert!(
    BALANCES_CB_TOTAL_TGAS >= PARK_CHAIN_TGAS + PARK_FRAME_TGAS,
    "the balances callback parks out of its own static budget, so a longer park chain silently starves the reclaim it was checking balances for"
);
const _: () = assert!(
    MAX_ALLOWLIST_SIZE as u64 * FT_BALANCE_TGAS + BALANCES_CB_TOTAL_TGAS + GATE_CALLER_FRAME_TGAS
        <= 300,
    "the gate queries every allowlisted token before it dispatches, so widening the allowlist past what one call can fund breaks every reclaim"
);

pub(crate) const SWEEP_ATTACHED_REQUIRED: NearToken =
    NearToken::from_yoctonear(hos_common::FT_STORAGE_DEPOSIT_YOCTO + 1);

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn reclaim_sweep_near(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<Promise, ContractError> {
        crate::assert_one_yocto()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        let (sub_account, _destination) = self.resolve_sweepable(&tla_id, &key)?;
        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_HOS_SWEEP)
            .sweep_near(sub_account))
    }

    #[handle_result]
    #[payable]
    pub fn reclaim_sweep_ft(
        &mut self,
        tla_id: AccountId,
        name: String,
        ft: AccountId,
    ) -> Result<Promise, ContractError> {
        validate_name(&name)?;
        if env::attached_deposit() < SWEEP_ATTACHED_REQUIRED {
            return Err(ContractError::InsufficientPayment);
        }
        let key = sub_account_key(&tla_id, &name);
        if !self.sweepable_tokens.contains(&ft) {
            return Err(ContractError::TokenNotInAllowlist);
        }
        let (sub_account, _destination) = self.resolve_sweepable(&tla_id, &key)?;
        let caller = env::predecessor_account_id();
        self.refund_excess(
            &caller,
            env::attached_deposit().as_yoctonear(),
            SWEEP_ATTACHED_REQUIRED.as_yoctonear(),
        );

        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_HOS_SWEEP)
            .with_attached_deposit(SWEEP_ATTACHED_REQUIRED)
            .sweep_ft(sub_account, ft, caller))
    }

    #[handle_result]
    pub fn reclaim_finalize(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<Promise, ContractError> {
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        if self.reclaim_pending.contains_key(&key) {
            return Err(ContractError::ReclaimInProgress);
        }
        let (sub_account, destination) = self.resolve_reclaimable(&tla_id, &key)?;
        if let Some(ends_at) = self.start_notice_if_term_is_live(&key)? {
            Event::SubAccountRetractionScheduled {
                full_name: key,
                retraction_at: near_sdk::json_types::U64(ends_at),
                by: env::predecessor_account_id(),
            }
            .emit();
            return Ok(crate::rental::retract_wallet_lease(
                &self.hos_extension,
                sub_account,
                ends_at,
            ));
        }
        self.reclaim_pending.insert(key.clone(), true);

        let allowlist: Vec<AccountId> = self.ft_allowlist.iter().cloned().collect();
        let Some(chain) = ft_balance_fanout(&allowlist, &sub_account) else {
            return Ok(self.park_wallet(sub_account, tla_id, name, destination));
        };
        Ok(chain.then(
            Self::ext(env::current_account_id())
                .with_static_gas(GAS_FOR_BALANCES_CB_TOTAL)
                .on_balances_checked(tla_id, name, destination, allowlist),
        ))
    }

    #[private]
    pub fn on_balances_checked(
        &mut self,
        tla_id: AccountId,
        name: String,
        destination: AccountId,
        allowlist: Vec<AccountId>,
    ) -> PromiseOrValue<()> {
        let key = sub_account_key(&tla_id, &name);
        if let BalanceGate::Blocked { token, reason } = ft_balances_clear(&allowlist) {
            self.reclaim_pending.remove(&key);
            Event::ReclaimFinalizeBlocked {
                full_name: key,
                token,
                reason,
            }
            .emit();
            return PromiseOrValue::Value(());
        }
        let sub_account = match self.resolve_reclaimable(&tla_id, &key) {
            Ok((sub_account, _)) => sub_account,
            Err(_) => {
                self.reclaim_pending.remove(&key);
                Event::ReclaimFinalizeBlocked {
                    full_name: key,
                    token: None,
                    reason: "no_longer_reclaimable".to_string(),
                }
                .emit();
                return PromiseOrValue::Value(());
            }
        };
        PromiseOrValue::Promise(self.park_wallet(sub_account, tla_id, name, destination))
    }

    #[private]
    pub fn on_reclaim_finalized(
        &mut self,
        tla_id: AccountId,
        name: String,
        destination: AccountId,
        #[callback_result] parked: Result<bool, PromiseError>,
    ) {
        let key = sub_account_key(&tla_id, &name);
        self.reclaim_pending.remove(&key);
        if !matches!(parked, Ok(true)) {
            Event::ReclaimFinalizeBlocked {
                full_name: key,
                token: None,
                reason: "park_failed".to_string(),
            }
            .emit();
            return;
        }
        let Some(removed) = self.sub_account_remove(&key) else {
            return;
        };
        crate::nft::emit_nft_burn(&removed.owner, &key);
        self.sub_account_count = self.sub_account_count.saturating_sub(1);
        self.business_count_decrement_if_business(&tla_id);
        let now = env::block_timestamp();
        self.parked_names.insert(
            key.clone(),
            ParkedEntry {
                tla_id: tla_id.clone(),
                parked_at: now,
            },
        );
        self.emit_activity(Event::SubAccountReclaimed {
            full_name: key,
            tla_id,
            swept_to: destination,
        });
    }
}

impl TlaRegistry {
    pub(crate) fn resolve_reclaimable(
        &self,
        tla_id: &AccountId,
        key: &str,
    ) -> Result<(AccountId, AccountId), ContractError> {
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        let sub = self
            .sub_accounts
            .get(key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if sub.tla_id != *tla_id {
            return Err(ContractError::SubAccountTlaMismatch);
        }
        let tla = self.tlas.get(tla_id).ok_or(ContractError::TlaNotFound)?;
        if !matches!(
            effective_sub_lifecycle(
                sub,
                tla,
                self.fee_config.retraction_notice_ns.0,
                &self.clock(),
                self.suspension_expiry(tla_id),
            ),
            LifecycleStatus::Reclaimable
        ) {
            return Err(ContractError::SubAccountNotReclaimable);
        }
        Ok((sub_account, sub.payout_account.clone()))
    }

    fn start_notice_if_term_is_live(&mut self, key: &str) -> Result<Option<u64>, ContractError> {
        let notice = self.fee_config.retraction_notice_ns.0;
        let now = env::block_timestamp();
        let Some(sub) = self.sub_accounts.get_mut(key) else {
            return Ok(None);
        };
        if now >= sub.expires_at {
            return Ok(None);
        }
        match sub.retraction_at {
            Some(started) if now >= started.saturating_add(notice) => Ok(None),
            Some(_) => Err(ContractError::RetractionPending),
            None => {
                sub.retraction_at = Some(now);
                Ok(Some(now.saturating_add(notice)))
            }
        }
    }

    fn resolve_sweepable(
        &self,
        tla_id: &AccountId,
        key: &str,
    ) -> Result<(AccountId, AccountId), ContractError> {
        if self.parked_names.contains_key(key) {
            let sub_account: AccountId = key
                .parse()
                .map_err(|_| ContractError::InvalidSubAccountId)?;
            return Ok((sub_account, self.treasury.clone()));
        }
        self.resolve_reclaimable(tla_id, key)
    }

    pub(crate) fn park_wallet(
        &self,
        sub_account: AccountId,
        tla_id: AccountId,
        name: String,
        destination: AccountId,
    ) -> Promise {
        let finalize = Self::ext(env::current_account_id())
            .with_static_gas(GAS_FOR_FINALIZE_CB)
            .on_reclaim_finalized(tla_id, name, destination);
        ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_HOS_FORCE_TRANSFER)
            .force_transfer(sub_account, None, RotationCause::Reclaim, None)
            .then(finalize)
    }
}
