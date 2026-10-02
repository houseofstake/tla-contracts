use crate::callbacks::MintSettlement;
use crate::error::ContractError;
use crate::events::Event;
use crate::fees;
use crate::rental::GAS_FOR_CALLBACK;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use hos_common::MintOutcome;
use near_sdk::json_types::{U128, U64};
use near_sdk::{env, near, AccountId, Promise, PromiseError};

const RESALE_PREFIX: &[u8] = b"biz_resale:";
const WHITELIST_PREFIX: &[u8] = b"biz_wl:";
const CLAIMED_PREFIX: &[u8] = b"biz_claimed:";
pub(crate) const MAX_WHITELIST_BATCH: usize = 100;

fn resale_key(tla_id: &AccountId) -> Vec<u8> {
    let mut key = RESALE_PREFIX.to_vec();
    key.extend_from_slice(tla_id.as_str().as_bytes());
    key
}

fn whitelist_key(tla_id: &AccountId, account: &AccountId) -> Vec<u8> {
    let mut key = WHITELIST_PREFIX.to_vec();
    key.extend_from_slice(tla_id.as_str().as_bytes());
    key.push(b'|');
    key.extend_from_slice(account.as_str().as_bytes());
    key
}

fn claimed_key(full_name: &str) -> Vec<u8> {
    let mut key = CLAIMED_PREFIX.to_vec();
    key.extend_from_slice(full_name.as_bytes());
    key
}

fn assert_whitelist_batch(accounts: &[AccountId]) -> Result<(), ContractError> {
    if accounts.is_empty() {
        return Err(ContractError::EmptyBatch);
    }
    if accounts.len() > MAX_WHITELIST_BATCH {
        return Err(ContractError::BatchTooLarge);
    }
    Ok(())
}

fn resale_enabled(tla_id: &AccountId) -> bool {
    env::storage_has_key(&resale_key(tla_id))
}

pub(crate) fn licensee_governed(tla_id: &AccountId, tla: &TlaEntry) -> bool {
    tla.tla_type == TlaType::Business && !resale_enabled(tla_id)
}

pub(crate) fn licensee_sets_payout(tla_id: &AccountId, tla: &TlaEntry, full_name: &str) -> bool {
    licensee_governed(tla_id, tla) && !env::storage_has_key(&claimed_key(full_name))
}

pub(crate) fn forget_claim(full_name: &str) {
    env::storage_remove(&claimed_key(full_name));
}

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
        self.assert_may_retract(&tla_id, licensee.as_ref(), &caller)?;
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
        self.assert_may_retract(&tla_id, licensee.as_ref(), &caller)?;
        let sub = self
            .sub_accounts
            .get_mut(&key)
            .ok_or(ContractError::SubAccountNotFound)?;
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
    #[payable]
    pub fn enable_business_resale(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin_or_council()?;
        let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
        if tla.tla_type != TlaType::Business {
            return Err(ContractError::NotBusinessTla);
        }
        let key = resale_key(&tla_id);
        if env::storage_has_key(&key) {
            return Ok(());
        }
        env::storage_write(&key, &[1]);
        Event::BusinessResaleEnabled {
            tla_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    pub fn is_business_resale_enabled(&self, tla_id: AccountId) -> bool {
        resale_enabled(&tla_id)
    }

    #[handle_result]
    #[payable]
    pub fn admin_whitelist_add(
        &mut self,
        tla_id: AccountId,
        accounts: Vec<AccountId>,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        assert_whitelist_batch(&accounts)?;
        let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
        if tla.tla_type != TlaType::Business {
            return Err(ContractError::NotBusinessTla);
        }
        let mut added = Vec::new();
        for account in accounts {
            if !env::storage_write(&whitelist_key(&tla_id, &account), &[1]) {
                added.push(account);
            }
        }
        Event::BusinessWhitelistAdded {
            tla_id,
            accounts: added,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn admin_whitelist_remove(
        &mut self,
        tla_id: AccountId,
        accounts: Vec<AccountId>,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        assert_whitelist_batch(&accounts)?;
        let mut removed = Vec::new();
        for account in accounts {
            if env::storage_remove(&whitelist_key(&tla_id, &account)) {
                removed.push(account);
            }
        }
        Event::BusinessWhitelistRemoved {
            tla_id,
            accounts: removed,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    pub fn is_whitelisted(&self, tla_id: AccountId, account_id: AccountId) -> bool {
        env::storage_has_key(&whitelist_key(&tla_id, &account_id))
    }

    #[handle_result]
    #[payable]
    pub fn claim_business_name(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<Promise, ContractError> {
        self.assert_not_paused()?;
        validate_mintable_name(&tla_id, &name)?;
        let claimer = env::predecessor_account_id();
        let entry = whitelist_key(&tla_id, &claimer);
        if !env::storage_has_key(&entry) {
            return Err(ContractError::NotWhitelisted);
        }
        self.assert_business_accepting(&tla_id)?;
        let key = sub_account_key(&tla_id, &name);
        if self.sub_accounts.contains_key(&key) || self.parked_names.contains_key(&key) {
            return Err(ContractError::SubAccountNameTaken);
        }
        let attached = env::attached_deposit().as_yoctonear();
        if attached < self.fee_config.account_creation_deposit_yocto.0 {
            return Err(ContractError::InsufficientPayment);
        }
        self.business_count_check_and_bump(&tla_id)?;
        env::storage_remove(&entry);
        env::storage_write(&claimed_key(&key), &[1]);
        let expires_at = self.open_lease(key, &tla_id, &claimer, &claimer);
        Ok(self
            .create_leased_account(&tla_id, &name, &claimer, &claimer, expires_at)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_CALLBACK)
                    .on_business_name_claimed(MintSettlement {
                        tla_id,
                        name,
                        owner: claimer.clone(),
                        payer: claimer,
                        rent_yocto: U128(0),
                        attached_yocto: U128(attached),
                        order_id: None,
                    }),
            ))
    }

    #[private]
    pub fn on_business_name_claimed(
        &mut self,
        settlement: MintSettlement,
        #[callback_result] outcome: Result<MintOutcome, PromiseError>,
    ) {
        if matches!(outcome, Ok(MintOutcome::CreationFailed)) {
            env::storage_write(&whitelist_key(&settlement.tla_id, &settlement.owner), &[1]);
        }
        self.on_sub_account_created(settlement, outcome);
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
    fn assert_business_accepting(&self, tla_id: &AccountId) -> Result<(), ContractError> {
        let tla = self.tlas.get(tla_id).ok_or(ContractError::TlaNotFound)?;
        if tla.tla_type != TlaType::Business {
            return Err(ContractError::NotBusinessTla);
        }
        if !tla.accepting_rentals(self.suspension_expiry(tla_id)) {
            return Err(ContractError::TlaNotAcceptingRentals);
        }
        Ok(())
    }

    fn assert_may_retract(
        &self,
        tla_id: &AccountId,
        licensee: Option<&AccountId>,
        caller: &AccountId,
    ) -> Result<(), ContractError> {
        if resale_enabled(tla_id) {
            if self.is_admin_or_council(caller) {
                return Ok(());
            }
            return Err(ContractError::OnlyAdminOrCouncil);
        }
        if licensee != Some(caller) {
            return Err(ContractError::OnlyLicensee);
        }
        Ok(())
    }

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
