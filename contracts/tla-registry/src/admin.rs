use crate::error::ContractError;
use crate::events::Event;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::{U128, U64};
use near_sdk::{env, near, AccountId};
use std::collections::BTreeSet;

pub(crate) const MAX_ALLOWLIST_SIZE: u32 = 16;
pub(crate) const MANUAL_ORDER_RELEASE_AFTER_NS: u64 = 3_600_000_000_000;
const LOG_BUDGET_BYTES: usize = 16_384;
const LOG_ENVELOPE_BYTES: usize = 512;
pub(crate) const MAX_TLA_BATCH: usize = 650;
const MAX_SWEEPABLE_SIZE: u32 = 64;
const MAX_VENUES: u32 = 8;
const MAX_AUTHORITY_SET: u32 = 32;
const GAS_FOR_SEAL_CB: near_sdk::Gas = near_sdk::Gas::from_tgas(5);
const MIN_RETRACTION_NOTICE_NS: u64 = 24 * 60 * 60 * 1_000_000_000;
const _: () = assert!(
    MIN_RETRACTION_NOTICE_NS > hos_common::MIN_LEASE_RETRACT_NOTICE_NS,
    "the registry pushes now plus the notice and the wallet re-checks it a block later against its own floor, so the two being equal makes every retraction unpushable"
);

fn assert_batch_fits(ids: &[&AccountId]) -> Result<(), ContractError> {
    if ids.is_empty() {
        return Err(ContractError::EmptyBatch);
    }
    if ids.len() > MAX_TLA_BATCH {
        return Err(ContractError::BatchTooLarge);
    }
    let logged: usize = ids.iter().map(|id| id.len() + 3).sum();
    if logged.saturating_add(LOG_ENVELOPE_BYTES) > LOG_BUDGET_BYTES {
        return Err(ContractError::BatchTooLarge);
    }
    Ok(())
}

fn assert_no_repeats(ids: &[&AccountId]) -> Result<(), ContractError> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(*id) {
            return Err(ContractError::DuplicateInBatch);
        }
    }
    Ok(())
}

impl TlaRegistry {
    fn check_registration(
        &self,
        tla_id: &AccountId,
        registration: &TlaRegistration,
    ) -> Result<(), ContractError> {
        if self.tlas.contains_key(tla_id) {
            return Err(ContractError::TlaAlreadyRegistered);
        }
        if registration.tla_type == TlaType::Business && registration.licensee.is_none() {
            return Err(ContractError::BusinessTlaRequiresLicensee);
        }
        Ok(())
    }

    fn check_open_activation(&self, tla_id: &AccountId) -> Result<(), ContractError> {
        let entry = self.tlas.get(tla_id).ok_or(ContractError::TlaNotFound)?;
        if entry.status != TlaStatus::Registered {
            return Err(ContractError::TlaNotInRegisteredState);
        }
        if entry.tla_type != TlaType::Open {
            return Err(ContractError::WrongActivationEndpoint);
        }
        Ok(())
    }

    fn write_registration(&mut self, tla_id: &AccountId, registration: &TlaRegistration) {
        let entry = TlaEntry {
            tla_type: registration.tla_type.clone(),
            status: TlaStatus::Registered,
            licensee: registration.licensee.clone(),
            premium_category: registration.premium_category.clone(),
            activated_at: 0,
            expires_at: 0,
        };
        self.tlas.insert(tla_id.clone(), entry);
    }

    fn write_open_activation(&mut self, tla_id: &AccountId, now: u64, expires_at: u64) {
        let Some(entry) = self.tlas.get_mut(tla_id) else {
            return;
        };
        entry.status = TlaStatus::Active;
        entry.activated_at = now;
        entry.expires_at = expires_at;
    }

    fn activate_one_open(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        self.check_open_activation(&tla_id)?;
        let now = env::block_timestamp();
        let expires_at = now.saturating_add(ONE_YEAR_NS);
        self.write_open_activation(&tla_id, now, expires_at);
        Event::TlaActivated {
            tla_id,
            expires_at: U64(expires_at),
            paid_yocto: U128(0),
        }
        .emit();
        Ok(())
    }
}

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn register_tla(
        &mut self,
        tla_id: AccountId,
        tla_type: TlaType,
        premium_category: PremiumCategory,
        licensee: Option<AccountId>,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        let registration = TlaRegistration {
            tla_type: tla_type.clone(),
            premium_category: premium_category.clone(),
            licensee: licensee.clone(),
        };
        self.check_registration(&tla_id, &registration)?;
        self.write_registration(&tla_id, &registration);
        Event::TlaRegistered {
            tla_id,
            tla_type,
            premium_category,
            licensee,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn register_tlas(
        &mut self,
        tla_ids: Vec<AccountId>,
        tla_type: TlaType,
        premium_category: PremiumCategory,
        licensee: Option<AccountId>,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        let ids: Vec<&AccountId> = tla_ids.iter().collect();
        assert_batch_fits(&ids)?;
        assert_no_repeats(&ids)?;
        let shared = TlaRegistration {
            tla_type: tla_type.clone(),
            premium_category: premium_category.clone(),
            licensee: licensee.clone(),
        };
        for tla_id in &tla_ids {
            self.check_registration(tla_id, &shared)?;
        }
        for tla_id in &tla_ids {
            self.write_registration(tla_id, &shared);
        }
        Event::TlasRegistered {
            tla_ids,
            tla_type,
            premium_category,
            licensee,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn activate_open_tlas(&mut self, tla_ids: Vec<AccountId>) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        let ids: Vec<&AccountId> = tla_ids.iter().collect();
        assert_batch_fits(&ids)?;
        assert_no_repeats(&ids)?;
        for tla_id in &tla_ids {
            self.check_open_activation(tla_id)?;
        }
        let now = env::block_timestamp();
        let expires_at = now.saturating_add(ONE_YEAR_NS);
        for tla_id in &tla_ids {
            self.write_open_activation(tla_id, now, expires_at);
        }
        Event::TlasActivated {
            tla_ids,
            expires_at: U64(expires_at),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn suspend_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        let entry = self
            .tlas
            .get_mut(&tla_id)
            .ok_or(ContractError::TlaNotFound)?;
        if entry.status == TlaStatus::Registered {
            return Err(ContractError::TlaNotActive);
        }
        entry.status = TlaStatus::Suspended;
        let until = env::block_timestamp().saturating_add(hos_common::MAX_AUTHORITY_HOLD_NS);
        self.suspended_until.insert(tla_id.clone(), until);
        Event::TlaSuspended {
            tla_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn unsuspend_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        let entry = self
            .tlas
            .get_mut(&tla_id)
            .ok_or(ContractError::TlaNotFound)?;
        if entry.status != TlaStatus::Suspended {
            return Err(ContractError::TlaNotSuspended);
        }
        if entry.tla_type == TlaType::Business && entry.licensee.is_none() {
            return Err(ContractError::BusinessTlaMissingLicensee);
        }
        entry.status = TlaStatus::Active;
        self.suspended_until.remove(&tla_id);
        Event::TlaUnsuspended {
            tla_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn add_admin(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if self.admins.len() >= MAX_AUTHORITY_SET {
            return Err(ContractError::AuthoritySetFull);
        }
        if !self.admins.insert(account_id.clone()) {
            return Ok(());
        }
        Event::AdminAdded {
            account: account_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn remove_admin(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if self.admins.len() <= 1 {
            return Err(ContractError::CannotRemoveLastAdmin);
        }
        if !self.admins.remove(&account_id) {
            return Ok(());
        }
        Event::AdminRemoved {
            account: account_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn add_payment_authority(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if self.payment_authorities.len() >= MAX_AUTHORITY_SET {
            return Err(ContractError::AuthoritySetFull);
        }
        if !self.payment_authorities.insert(account_id.clone()) {
            return Ok(());
        }
        Event::PaymentAuthorityAdded {
            account: account_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn bind_payment_authority_tla(
        &mut self,
        account_id: AccountId,
        tla_id: AccountId,
        max_mints: U64,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if !self.payment_authorities.contains(&account_id) {
            return Err(ContractError::OnlyPaymentAuthority);
        }
        if !self.tlas.contains_key(&tla_id) {
            return Err(ContractError::TlaNotFound);
        }
        if max_mints.0 == 0 {
            return Err(ContractError::AuthorityMintAllowanceZero);
        }
        near_sdk::env::storage_write(
            &crate::authority_tla_key(&account_id, &tla_id),
            &max_mints.0.to_le_bytes(),
        );
        Event::PaymentAuthorityBound {
            account: account_id,
            tla_id,
            max_mints,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    pub fn payment_authority_allowance(
        &self,
        account_id: AccountId,
        tla_id: AccountId,
    ) -> Option<U64> {
        crate::authority_allowance(&account_id, &tla_id).map(U64)
    }

    pub fn payment_authority_used(&self, account_id: AccountId, tla_id: AccountId) -> U64 {
        U64(crate::authority_used(&account_id, &tla_id))
    }

    #[handle_result]
    #[payable]
    pub fn unbind_payment_authority_tla(
        &mut self,
        account_id: AccountId,
        tla_id: AccountId,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        near_sdk::env::storage_remove(&crate::authority_tla_key(&account_id, &tla_id));
        near_sdk::env::storage_remove(&crate::authority_used_key(&account_id, &tla_id));
        Event::PaymentAuthorityUnbound {
            account: account_id,
            tla_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    pub fn is_payment_authority_bound(&self, account_id: AccountId, tla_id: AccountId) -> bool {
        near_sdk::env::storage_has_key(&crate::authority_tla_key(&account_id, &tla_id))
    }

    #[handle_result]
    #[payable]
    pub fn remove_payment_authority(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if !self.payment_authorities.remove(&account_id) {
            return Ok(());
        }
        Event::PaymentAuthorityRemoved {
            account: account_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn add_venue(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if account_id == env::current_account_id() {
            return Err(ContractError::VenueIsRegistry);
        }
        if self.venues.len() >= MAX_VENUES {
            return Err(ContractError::AllowlistFull);
        }
        if !self.venues.insert(account_id.clone()) {
            return Ok(());
        }
        Event::VenueAdded {
            account: account_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn remove_venue(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if !self.venues.remove(&account_id) {
            return Ok(());
        }
        Event::VenueRemoved {
            account: account_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn add_recovery_authority(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if self.recovery_authorities.len() >= MAX_AUTHORITY_SET {
            return Err(ContractError::AuthoritySetFull);
        }
        if !self.recovery_authorities.insert(account_id.clone()) {
            return Ok(());
        }
        Event::RecoveryAuthorityAdded {
            account: account_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn remove_recovery_authority(
        &mut self,
        account_id: AccountId,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if !self.recovery_authorities.remove(&account_id) {
            return Ok(());
        }
        Event::RecoveryAuthorityRemoved {
            account: account_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn update_fee_config(&mut self, config: FeeConfig) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if config.rent_tier_5_usd_micro.0 == 0
            && config.rent_tier_8_usd_micro.0 == 0
            && config.rent_tier_10_usd_micro.0 == 0
            && config.rent_tier_12plus_usd_micro.0 == 0
        {
            return Err(ContractError::AllRentTiersZero);
        }
        if config.account_creation_deposit_yocto.0 == 0 {
            return Err(ContractError::CreationDepositZero);
        }
        if config.rent_tier_5_usd_micro.0 < config.rent_tier_8_usd_micro.0
            || config.rent_tier_8_usd_micro.0 < config.rent_tier_10_usd_micro.0
            || config.rent_tier_10_usd_micro.0 < config.rent_tier_12plus_usd_micro.0
        {
            return Err(ContractError::RentTiersNotDescending);
        }
        if config.max_rate_move_bps > 10_000 || config.quote_slippage_bps > 10_000 {
            return Err(ContractError::InvalidRateBounds);
        }
        let min_rate = config.min_near_usd_rate_micro.0;
        let max_rate = config.max_near_usd_rate_micro.0;
        if min_rate == 0
            || min_rate > max_rate
            || max_rate > crate::pricing::MAX_NEAR_USD_RATE_MICRO
        {
            return Err(ContractError::InvalidRateBounds);
        }
        if self.near_usd_rate_micro != 0
            && (self.near_usd_rate_micro < min_rate || self.near_usd_rate_micro > max_rate)
        {
            return Err(ContractError::RateOutOfBounds);
        }
        if config.rate_update_cooldown_ns.0 == 0
            || config.max_rate_age_ns.0 < config.rate_update_cooldown_ns.0
        {
            return Err(ContractError::InvalidRateBounds);
        }
        if config.business_max_subs == 0 {
            return Err(ContractError::InvalidBusinessCap);
        }
        if config.retraction_notice_ns.0 < MIN_RETRACTION_NOTICE_NS {
            return Err(ContractError::RetractionNoticeTooShort);
        }
        if [
            config.tla_allocation_fee_usd_micro.0,
            config.rent_tier_5_usd_micro.0,
            config.rent_tier_8_usd_micro.0,
            config.rent_tier_10_usd_micro.0,
            config.rent_tier_12plus_usd_micro.0,
            config.sub_fee_per_account_usd_micro.0,
        ]
        .iter()
        .any(|fee| *fee > crate::pricing::MAX_USD_MICRO)
        {
            return Err(ContractError::FeeExceedsCap);
        }
        self.fee_config = config;
        Event::FeeConfigUpdated {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn withdraw(&mut self, amount: U128) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        let recipient = self.treasury.clone();
        let amount_yocto = amount.0;
        if amount_yocto == 0 {
            return Err(ContractError::WithdrawalAmountZero);
        }
        if amount_yocto > self.total_revenue {
            return Err(ContractError::InsufficientRevenue);
        }
        if self.total_pending_refunds.saturating_add(amount_yocto) > self.available_balance() {
            return Err(ContractError::InsufficientContractBalance);
        }
        self.total_revenue = self.total_revenue.saturating_sub(amount_yocto);
        self.add_pending_refund(&recipient, amount_yocto);

        Event::WithdrawalQueued {
            amount_yocto: amount,
            recipient,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn add_ft_allowlist(&mut self, token: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        if self.ft_allowlist.contains(&token) {
            return Ok(());
        }
        if self.ft_allowlist.len() >= MAX_ALLOWLIST_SIZE {
            return Err(ContractError::AllowlistFull);
        }
        let widens_sweep = !self.sweepable_tokens.contains(&token);
        if widens_sweep && self.sweepable_tokens.len() >= MAX_SWEEPABLE_SIZE {
            return Err(ContractError::AllowlistFull);
        }
        self.ft_allowlist.insert(token.clone());
        self.sweepable_tokens.insert(token.clone());
        Event::FtAllowlistAdded {
            token,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn remove_ft_allowlist(&mut self, token: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        if !self.ft_allowlist.remove(&token) {
            return Ok(());
        }
        Event::FtAllowlistRemoved {
            token,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn set_lease_term_ns(&mut self, lease_term_ns: U64) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if lease_term_ns.0 < crate::MIN_LEASE_TERM_NS {
            return Err(ContractError::LeaseTermTooShort);
        }
        if lease_term_ns.0 > crate::MAX_LEASE_TERM_NS {
            return Err(ContractError::LeaseTermTooLong);
        }
        self.lease_term_ns = lease_term_ns.0;
        Event::LeaseTermChanged {
            lease_term_ns: U64(lease_term_ns.0),
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn remove_sweepable_token(&mut self, token: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if self.ft_allowlist.contains(&token) {
            return Err(ContractError::TokenStillAllowlisted);
        }
        if !self.sweepable_tokens.remove(&token) {
            return Ok(());
        }
        Event::SweepableTokenRemoved {
            token,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn activate_open_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        self.activate_one_open(tla_id)
    }

    #[handle_result]
    #[payable]
    pub fn admin_clear_reclaim_pending(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        let key = sub_account_key(&tla_id, &name);
        self.reclaim_pending.remove(&key);
        Event::ReclaimPendingCleared {
            full_name: key,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn admin_release_paid_order(&mut self, order_id: String) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        match self.paid_order_ids.get(&order_id) {
            None => Err(ContractError::PaidOrderNotFound),
            Some(PaidOrderState::Settled) => Err(ContractError::PaidOrderAlreadySettled),
            Some(PaidOrderState::InFlight {
                started_at,
                payer,
                tla_id,
                ..
            }) => {
                let held_for = env::block_timestamp().saturating_sub(*started_at);
                if held_for < MANUAL_ORDER_RELEASE_AFTER_NS {
                    return Err(ContractError::PaidOrderStillInFlight);
                }
                let payer = payer.clone();
                let tla_id = tla_id.clone();
                self.paid_order_ids.remove(&order_id);
                self.release_authority_mint(&payer, &tla_id);
                Event::PaidRentalOrderReleased { order_id }.emit();
                Ok(())
            }
        }
    }

    #[handle_result]
    #[payable]
    pub fn admin_release_park(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        let key = sub_account_key(&tla_id, &name);
        if self.sub_accounts.contains_key(&key) {
            return Err(ContractError::SubAccountNameTaken);
        }
        if self.parked_names.remove(&key).is_none() {
            return Err(ContractError::SubAccountNotParked);
        }
        Event::ParkReleased {
            full_name: key,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn admin_force_park(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        validate_name(&name)?;
        if !self.tlas.contains_key(&tla_id) {
            return Err(ContractError::TlaNotFound);
        }
        let key = sub_account_key(&tla_id, &name);
        if self.sub_accounts.contains_key(&key) {
            return Err(ContractError::SubAccountNameTaken);
        }
        if self.parked_names.contains_key(&key) {
            return Err(ContractError::SubAccountNameTaken);
        }
        self.parked_names.insert(
            key.clone(),
            ParkedEntry {
                tla_id,
                parked_at: env::block_timestamp(),
            },
        );
        Event::ParkForced {
            full_name: key,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }
}

pub(crate) const UPGRADE_DELAY_NS: u64 = 48 * 60 * 60 * 1_000_000_000;

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn approve_council_rotation(
        &mut self,
        new_council: AccountId,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
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

    #[handle_result]
    #[payable]
    pub fn cancel_council_rotation(&mut self) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
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

    #[handle_result]
    #[payable]
    pub fn commit_council_rotation(&mut self) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
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
        if env::block_timestamp() < approved_at.saturating_add(self.upgrade_delay_ns) {
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
    #[payable]
    pub fn approve_upgrade(
        &mut self,
        code_hash: near_sdk::json_types::Base58CryptoHash,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
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
    pub fn upgrade(
        &mut self,
        code: near_sdk::json_types::Base64VecU8,
    ) -> Result<near_sdk::Promise, ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
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
        if env::block_timestamp() < approved_at.saturating_add(self.upgrade_delay_ns) {
            return Err(ContractError::ApprovalTooYoung);
        }
        self.approved_code_hash = None;
        self.approved_at = None;
        Ok(hos_common::deploy_and_migrate(code))
    }

    #[payable]
    #[handle_result]
    pub fn seal(
        &mut self,
        public_key: near_sdk::PublicKey,
    ) -> Result<near_sdk::Promise, ContractError> {
        crate::assert_one_yocto()?;
        self.assert_council()?;
        if !self.upgrade_proven {
            return Err(ContractError::UpgradeNotProven);
        }
        let by = env::predecessor_account_id();
        let key: String = (&public_key).into();
        Ok(near_sdk::Promise::new(env::current_account_id())
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

    pub fn approved_upgrade_hash(&self) -> Option<near_sdk::json_types::Base58CryptoHash> {
        self.approved_code_hash
            .map(near_sdk::json_types::Base58CryptoHash::from)
    }

    pub fn approved_upgrade_at(&self) -> Option<near_sdk::json_types::U64> {
        self.approved_at.map(near_sdk::json_types::U64)
    }

    pub fn upgrade_delay_ns(&self) -> near_sdk::json_types::U64 {
        near_sdk::json_types::U64(self.upgrade_delay_ns)
    }
}
