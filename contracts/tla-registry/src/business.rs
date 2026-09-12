use crate::error::ContractError;
use crate::events::Event;
use crate::fees;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::{U128, U64};
use near_sdk::{env, near, AccountId, Promise};

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn schedule_retraction(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<Promise, ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        let caller = env::predecessor_account_id();
        let now = env::block_timestamp();
        let licensee = {
            let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
            if tla.tla_type != TlaType::Business {
                return Err(ContractError::NotBusinessTla);
            }
            tla.licensee.clone()
        };
        if licensee.as_ref() != Some(&caller) {
            return Err(ContractError::OnlyLicensee);
        }
        let sub = self
            .sub_accounts
            .get_mut(&key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if sub.retraction_at.is_some() {
            return Err(ContractError::RetractionAlreadyScheduled);
        }
        sub.retraction_at = Some(now);
        let ends_at = now.saturating_add(self.fee_config.retraction_notice_ns.0);
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        Ok(crate::rental::retract_wallet_lease_pending(
            &self.hos_extension,
            sub_account,
            ends_at,
            key,
            caller,
        ))
    }

    #[handle_result]
    #[payable]
    pub fn cancel_retraction(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<Promise, ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        let caller = env::predecessor_account_id();
        let retraction_notice_ns = self.fee_config.retraction_notice_ns.0;
        let now = env::block_timestamp();
        let licensee = self
            .tlas
            .get(&tla_id)
            .ok_or(ContractError::TlaNotFound)?
            .licensee
            .clone();
        let sub = self
            .sub_accounts
            .get_mut(&key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if licensee.as_ref() != Some(&caller) {
            return Err(ContractError::OnlyLicensee);
        }
        let retraction_at = sub
            .retraction_at
            .ok_or(ContractError::NoRetractionScheduled)?;
        if now >= retraction_at.saturating_add(retraction_notice_ns) {
            return Err(ContractError::RetractionAlreadyElapsed);
        }
        let expires_at = sub.expires_at;
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        Ok(crate::rental::restore_wallet_lease(
            &self.hos_extension,
            sub_account,
            expires_at,
            key,
            caller,
        ))
    }

    #[private]
    pub fn on_retraction_scheduled(&mut self, key: String, by: AccountId) {
        if near_sdk::is_promise_success() {
            let scheduled_at = self
                .sub_accounts
                .get(&key)
                .and_then(|sub| sub.retraction_at);
            if let Some(at) = scheduled_at {
                Event::SubAccountRetractionScheduled {
                    full_name: key,
                    retraction_at: U64(at),
                    by,
                }
                .emit();
            }
            return;
        }
        if let Some(sub) = self.sub_accounts.get_mut(&key) {
            sub.retraction_at = None;
        }
        Event::LeaseSyncFailed {
            full_name: key,
            intent: "retract".to_string(),
        }
        .emit();
    }

    #[private]
    pub fn on_retraction_canceled(&mut self, key: String, by: AccountId) {
        if !near_sdk::is_promise_success() {
            Event::LeaseSyncFailed {
                full_name: key,
                intent: "restore".to_string(),
            }
            .emit();
            return;
        }
        if let Some(sub) = self.sub_accounts.get_mut(&key) {
            sub.retraction_at = None;
        }
        Event::SubAccountRetractionCanceled { full_name: key, by }.emit();
    }

    pub fn get_business_sub_count(&self, tla_id: AccountId) -> u32 {
        self.business_sub_count.get(&tla_id).copied().unwrap_or(0)
    }

    pub fn get_business_sub_cap(&self, tla_id: AccountId) -> u32 {
        self.effective_business_cap(&tla_id)
    }

    #[handle_result]
    #[payable]
    pub fn set_tla_terms(
        &mut self,
        tla_id: AccountId,
        terms: TlaTerms,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
        if terms.sub_fee_usd_micro.is_some() && tla.tla_type != TlaType::Business {
            return Err(ContractError::NotBusinessTla);
        }
        for fee in [
            terms.allocation_fee_usd_micro,
            terms.tla_rent_usd_micro,
            terms.sub_fee_usd_micro,
        ]
        .into_iter()
        .flatten()
        {
            if fee.0 > crate::pricing::MAX_USD_MICRO {
                return Err(ContractError::FeeExceedsCap);
            }
        }
        let event = Event::TlaTermsSet {
            tla_id: tla_id.clone(),
            allocation_fee_usd_micro: terms.allocation_fee_usd_micro,
            tla_rent_usd_micro: terms.tla_rent_usd_micro,
            sub_fee_usd_micro: terms.sub_fee_usd_micro,
            by: env::predecessor_account_id(),
        };
        if terms.is_standard() {
            self.tla_terms.remove(&tla_id);
        } else {
            self.tla_terms.insert(tla_id, terms);
        }
        event.emit();
        Ok(())
    }

    pub fn get_tla_terms(&self, tla_id: AccountId) -> TlaTerms {
        self.terms_for(&tla_id)
    }

    #[handle_result]
    #[payable]
    pub fn set_business_sub_cap(
        &mut self,
        tla_id: AccountId,
        cap: Option<u32>,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        {
            let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
            if tla.tla_type != TlaType::Business {
                return Err(ContractError::NotBusinessTla);
            }
        }
        match cap {
            Some(value) => {
                self.business_sub_cap_override.insert(tla_id.clone(), value);
            }
            None => {
                self.business_sub_cap_override.remove(&tla_id);
            }
        }
        Event::BusinessSubCapSet {
            tla_id,
            cap,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn get_business_renewal_cost(
        &self,
        tla_id: AccountId,
    ) -> Result<BusinessRenewalCostView, ContractError> {
        let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
        if tla.tla_type != TlaType::Business {
            return Err(ContractError::NotBusinessTla);
        }
        let tla_len = tla_id.as_str().len() as u8;
        let tla_rent = self.quote_usd_to_near(fees::base_rent(tla_len, &self.fee_config))?;
        Ok(BusinessRenewalCostView {
            tla_id,
            tla_rent_yocto: U128(tla_rent),
        })
    }

    pub fn get_retraction_at(&self, tla_id: AccountId, name: String) -> Option<u64> {
        let key = sub_account_key(&tla_id, &name);
        self.sub_accounts.get(&key).and_then(|s| s.retraction_at)
    }
}

impl TlaRegistry {
    pub(crate) fn effective_business_cap(&self, tla_id: &AccountId) -> u32 {
        self.business_sub_cap_override
            .get(tla_id)
            .copied()
            .unwrap_or(self.fee_config.business_max_subs)
    }

    pub(crate) fn business_count_check_and_bump(
        &mut self,
        tla_id: &AccountId,
    ) -> Result<(), ContractError> {
        let cap = self.effective_business_cap(tla_id);
        let count = self.business_sub_count.get(tla_id).copied().unwrap_or(0);
        if count >= cap {
            return Err(ContractError::MaxBusinessSubsReached);
        }
        self.business_sub_count
            .insert(tla_id.clone(), count.saturating_add(1));
        Ok(())
    }

    pub(crate) fn business_count_decrement(&mut self, tla_id: &AccountId) {
        let count = self.business_sub_count.get(tla_id).copied().unwrap_or(0);
        if count == 0 {
            return;
        }
        let next = count.saturating_sub(1);
        if next == 0 {
            self.business_sub_count.remove(tla_id);
        } else {
            self.business_sub_count.insert(tla_id.clone(), next);
        }
    }

    pub(crate) fn business_count_decrement_if_business(&mut self, tla_id: &AccountId) {
        let is_business = self
            .tlas
            .get(tla_id)
            .map(|t| t.tla_type == TlaType::Business)
            .unwrap_or(false);
        if is_business {
            self.business_count_decrement(tla_id);
        }
    }
}
