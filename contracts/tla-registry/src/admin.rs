use crate::error::ContractError;
use crate::events::Event;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::{U128, U64};
use near_sdk::{env, near, AccountId};

const MAX_ALLOWLIST_SIZE: u32 = 16;
const MAX_SWEEPABLE_SIZE: u32 = 64;
const MIN_RETRACTION_NOTICE_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

#[near]
impl TlaRegistry {
    #[handle_result]
    pub fn register_tla(
        &mut self,
        tla_id: AccountId,
        tla_type: TlaType,
        premium_category: PremiumCategory,
        licensee: Option<AccountId>,
    ) -> Result<(), ContractError> {
        self.assert_council()?;
        if self.tlas.contains_key(&tla_id) {
            return Err(ContractError::TlaAlreadyRegistered);
        }
        if tla_type == TlaType::Business && licensee.is_none() {
            return Err(ContractError::BusinessTlaRequiresLicensee);
        }

        let entry = TlaEntry {
            tla_type: tla_type.clone(),
            status: TlaStatus::Registered,
            licensee: licensee.clone(),
            premium_category: premium_category.clone(),
            activated_at: 0,
            expires_at: 0,
        };
        self.tlas.insert(tla_id.clone(), entry);

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
    pub fn suspend_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
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
    pub fn unsuspend_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
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
    pub fn add_admin(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        self.assert_council()?;
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
    pub fn remove_admin(&mut self, account_id: AccountId) -> Result<(), ContractError> {
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
    pub fn add_payment_authority(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        self.assert_council()?;
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
    pub fn remove_payment_authority(&mut self, account_id: AccountId) -> Result<(), ContractError> {
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
    pub fn add_recovery_authority(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        self.assert_council()?;
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
    pub fn remove_recovery_authority(
        &mut self,
        account_id: AccountId,
    ) -> Result<(), ContractError> {
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
    pub fn update_fee_config(&mut self, config: FeeConfig) -> Result<(), ContractError> {
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
        if config.resale_commission_bps > 10_000 {
            return Err(ContractError::InvalidCommissionRate);
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
    pub fn withdraw(&mut self, amount: U128) -> Result<(), ContractError> {
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
    pub fn add_ft_allowlist(&mut self, token: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        if self.ft_allowlist.contains(&token) {
            return Ok(());
        }
        if self.ft_allowlist.len() >= MAX_ALLOWLIST_SIZE
            || self.sweepable_tokens.len() >= MAX_SWEEPABLE_SIZE
        {
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
    pub fn remove_ft_allowlist(&mut self, token: AccountId) -> Result<(), ContractError> {
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
    pub fn activate_open_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        let now = env::block_timestamp();
        let new_expires_at = {
            let entry = self
                .tlas
                .get_mut(&tla_id)
                .ok_or(ContractError::TlaNotFound)?;
            if entry.status != TlaStatus::Registered {
                return Err(ContractError::TlaNotInRegisteredState);
            }
            if entry.tla_type != TlaType::Open {
                return Err(ContractError::WrongActivationEndpoint);
            }
            entry.status = TlaStatus::Active;
            entry.activated_at = now;
            entry.expires_at = now.saturating_add(ONE_YEAR_NS);
            entry.expires_at
        };

        Event::TlaActivated {
            tla_id,
            expires_at: U64(new_expires_at),
            paid_yocto: U128(0),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn admin_clear_reclaim_pending(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<(), ContractError> {
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
}

pub(crate) const UPGRADE_DELAY_NS: u64 = 48 * 60 * 60 * 1_000_000_000;

#[near]
impl TlaRegistry {
    #[payable]
    #[handle_result]
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
        Event::Upgraded {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(hos_common::deploy_and_migrate(code))
    }

    pub fn approved_upgrade_hash(&self) -> Option<near_sdk::json_types::Base58CryptoHash> {
        self.approved_code_hash
            .map(near_sdk::json_types::Base58CryptoHash::from)
    }
}
