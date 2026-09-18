mod activity;
mod admin;
mod asset_gate;
mod business;
mod callbacks;
mod error;
mod events;
mod fees;
mod indexes;
mod interfaces;
mod legacy;
mod lifecycle;
mod marketplace;
mod nft;
mod pricing;
mod reclaim;
mod rental;
#[cfg(test)]
mod tests;
mod types;
mod views;

use crate::error::ContractError;
use crate::events::Event;
use crate::types::*;
use near_sdk::borsh::BorshSerialize;
use near_sdk::json_types::{U128, U64};
use near_sdk::store::{IterableMap, IterableSet, LookupMap, Vector};
use near_sdk::{
    env, is_promise_success, near, require, AccountId, BorshStorageKey, Gas, NearToken,
    PanicOnDefault, Promise,
};

const CONTRACT_VERSION: u8 = 1;
const STATE_VERSION: u16 = 5;

const ALLOWANCE_WARN_REMAINING: u64 = 5;
const AUTHORITY_TLA_PREFIX: &[u8] = b"pa_tla:";
const AUTHORITY_USED_PREFIX: &[u8] = b"pa_use:";

fn authority_scoped_key(prefix: &[u8], authority: &AccountId, tla_id: &AccountId) -> Vec<u8> {
    let mut key = prefix.to_vec();
    key.extend_from_slice(authority.as_str().as_bytes());
    key.push(b'|');
    key.extend_from_slice(tla_id.as_str().as_bytes());
    key
}

pub(crate) fn authority_tla_key(authority: &AccountId, tla_id: &AccountId) -> Vec<u8> {
    authority_scoped_key(AUTHORITY_TLA_PREFIX, authority, tla_id)
}

pub(crate) fn authority_used_key(authority: &AccountId, tla_id: &AccountId) -> Vec<u8> {
    authority_scoped_key(AUTHORITY_USED_PREFIX, authority, tla_id)
}

fn read_u64(key: &[u8]) -> Option<u64> {
    env::storage_read(key)
        .and_then(|raw| <[u8; 8]>::try_from(raw.as_slice()).ok())
        .map(u64::from_le_bytes)
}

pub(crate) fn authority_allowance(authority: &AccountId, tla_id: &AccountId) -> Option<u64> {
    read_u64(&authority_tla_key(authority, tla_id))
}

pub(crate) fn authority_used(authority: &AccountId, tla_id: &AccountId) -> u64 {
    read_u64(&authority_used_key(authority, tla_id)).unwrap_or(0)
}
const MIN_GRACE_PERIOD_NS: u64 = 60 * 1_000_000_000;
pub(crate) const MIN_LEASE_TERM_NS: u64 = 60 * 1_000_000_000;
pub(crate) const MAX_LEASE_TERM_NS: u64 = 10 * ONE_YEAR_NS;
pub(crate) const PRODUCTION_GRACE_PERIOD_NS: u64 = 14 * 24 * 60 * 60 * 1_000_000_000;
pub(crate) const PRODUCTION_LEASE_TERM_NS: u64 = ONE_YEAR_NS;
use hos_common::MAX_AUTHORITY_HOLD_NS;

const GAS_FOR_CLAIM_REFUND_CB: Gas = Gas::from_tgas(10);

const ACTIVITY_CAPACITY: u32 = 256;

/// Discriminants are storage prefixes, so a retired variant keeps its position
/// to preserve the prefixes of every variant after it. Deleting one silently
/// repoints live collections at another key's data.
#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
#[allow(dead_code)]
pub(crate) enum StorageKey {
    Tlas,
    RetiredSubAccounts,
    Admins,
    PendingRefunds,
    FtAllowlist,
    BusinessSubCount,
    BusinessSubCapOverride,
    RetiredListings,
    RetiredAcceptedOffers,
    ParkedNames,
    ReclaimPending,
    PaymentAuthorities,
    RecoveryAuthorities,
    SubAccountsIndexed,
    RetiredListingsIndexed,
    RetiredAcceptedOffersIndexed,
    SubAccountsByOwner,
    SubAccountsByOwnerInner { owner: AccountId },
    SubAccountsByTla,
    SubAccountsByTlaInner { tla: AccountId },
    RecentActivity,
    SweepableTokens,
    SuspendedUntil,
    RetiredTokenMetadataStore,
    Venues,
    PaidOrderIds,
    TlaTerms,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct TlaRegistry {
    pub(crate) state_version: u16,
    pub(crate) tlas: IterableMap<AccountId, TlaEntry>,
    pub(crate) sub_accounts: IterableMap<String, SubAccountEntry>,
    pub(crate) sub_accounts_by_owner: LookupMap<AccountId, IterableSet<String>>,
    pub(crate) sub_accounts_by_tla: LookupMap<AccountId, IterableSet<String>>,
    pub(crate) recent_activity: Vector<ActivityRecord>,
    pub(crate) activity_cursor: u32,
    pub(crate) admins: IterableSet<AccountId>,
    pub(crate) fee_config: FeeConfig,
    pub(crate) total_revenue: u128,
    pub(crate) sub_account_count: u64,
    pub(crate) paused: bool,
    pub(crate) version: u8,
    pub(crate) pending_refunds: LookupMap<AccountId, u128>,
    pub(crate) total_pending_refunds: u128,
    pub(crate) ft_allowlist: IterableSet<AccountId>,
    pub(crate) business_sub_count: LookupMap<AccountId, u32>,
    pub(crate) business_sub_cap_override: LookupMap<AccountId, u32>,
    pub(crate) tla_terms: LookupMap<AccountId, TlaTerms>,
    pub(crate) parked_names: LookupMap<String, ParkedEntry>,
    pub(crate) reclaim_pending: LookupMap<String, bool>,
    pub(crate) payment_authorities: IterableSet<AccountId>,
    pub(crate) recovery_authorities: IterableSet<AccountId>,
    pub(crate) hos_extension: AccountId,
    pub(crate) grace_period_ns: u64,
    pub(crate) lease_term_ns: u64,
    pub(crate) price_oracle: AccountId,
    pub(crate) near_usd_rate_micro: u128,
    pub(crate) rate_updated_at: u64,
    pub(crate) rate_sequence: u64,
    pub(crate) treasury: AccountId,
    pub(crate) council: AccountId,
    pub(crate) marketplace_paused: bool,
    pub(crate) paused_until_ns: u64,
    pub(crate) unpaused_at: u64,
    pub(crate) sweepable_tokens: IterableSet<AccountId>,
    pub(crate) suspended_until: LookupMap<AccountId, u64>,
    pub(crate) nft_contract_metadata: NftContractMetadata,
    pub(crate) approved_code_hash: Option<[u8; 32]>,
    pub(crate) approved_at: Option<u64>,
    pub(crate) upgrade_delay_ns: u64,
    pub(crate) venues: IterableSet<AccountId>,
    pub(crate) upgrade_proven: bool,
    pub(crate) paid_order_ids: LookupMap<String, PaidOrderState>,
    pub(crate) pending_council: Option<AccountId>,
    pub(crate) pending_council_at: Option<u64>,
    pub(crate) pending_treasury: Option<AccountId>,
    pub(crate) pending_treasury_at: Option<u64>,
}

#[near]
impl TlaRegistry {
    #[init]
    pub fn new(
        admin: AccountId,
        hos_extension: AccountId,
        grace_period_ns: U64,
        treasury: AccountId,
        council: AccountId,
        lease_term_ns: Option<U64>,
    ) -> Self {
        require!(
            grace_period_ns.0 >= MIN_GRACE_PERIOD_NS,
            "grace period too short"
        );
        let lease_term_ns = lease_term_ns.map_or(PRODUCTION_LEASE_TERM_NS, |value| value.0);
        require!(lease_term_ns >= MIN_LEASE_TERM_NS, "lease term too short");
        require!(lease_term_ns <= MAX_LEASE_TERM_NS, "lease term too long");
        let this = env::current_account_id();
        require!(
            hos_extension != this,
            "wiring must not point at the registry"
        );
        require!(
            council != this,
            "council must not be the registry, which ends with no keys"
        );
        let mut admins = IterableSet::new(StorageKey::Admins);
        admins.insert(admin.clone());

        Self {
            state_version: STATE_VERSION,
            tlas: IterableMap::new(StorageKey::Tlas),
            sub_accounts: IterableMap::new(StorageKey::SubAccountsIndexed),
            sub_accounts_by_owner: LookupMap::new(StorageKey::SubAccountsByOwner),
            sub_accounts_by_tla: LookupMap::new(StorageKey::SubAccountsByTla),
            recent_activity: Vector::new(StorageKey::RecentActivity),
            activity_cursor: 0,
            admins,
            fee_config: fees::default_fee_config(),
            total_revenue: 0,
            sub_account_count: 0,
            paused: false,
            version: CONTRACT_VERSION,
            pending_refunds: LookupMap::new(StorageKey::PendingRefunds),
            total_pending_refunds: 0,
            ft_allowlist: IterableSet::new(StorageKey::FtAllowlist),
            business_sub_count: LookupMap::new(StorageKey::BusinessSubCount),
            business_sub_cap_override: LookupMap::new(StorageKey::BusinessSubCapOverride),
            tla_terms: LookupMap::new(StorageKey::TlaTerms),
            parked_names: LookupMap::new(StorageKey::ParkedNames),
            reclaim_pending: LookupMap::new(StorageKey::ReclaimPending),
            payment_authorities: IterableSet::new(StorageKey::PaymentAuthorities),
            recovery_authorities: IterableSet::new(StorageKey::RecoveryAuthorities),
            hos_extension,
            grace_period_ns: grace_period_ns.0,
            lease_term_ns,
            price_oracle: admin,
            near_usd_rate_micro: 0,
            rate_updated_at: 0,
            rate_sequence: 0,
            treasury,
            council,
            marketplace_paused: false,
            paused_until_ns: 0,
            unpaused_at: 0,
            sweepable_tokens: IterableSet::new(StorageKey::SweepableTokens),
            suspended_until: LookupMap::new(StorageKey::SuspendedUntil),
            nft_contract_metadata: nft::initial_metadata(),
            approved_code_hash: None,
            approved_at: None,
            upgrade_delay_ns: admin::UPGRADE_DELAY_NS,
            venues: IterableSet::new(StorageKey::Venues),
            upgrade_proven: false,
            paid_order_ids: LookupMap::new(StorageKey::PaidOrderIds),
            pending_council: None,
            pending_council_at: None,
            pending_treasury: None,
            pending_treasury_at: None,
        }
    }

    #[private]
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        let mut current = match hos_common::state_version() {
            Some(STATE_VERSION) => hos_common::try_state_read::<Self>()
                .unwrap_or_else(|| env::panic_str(error::NO_STATE)),
            Some(4) => hos_common::try_state_read::<legacy::TlaRegistryV4>()
                .map(Self::from)
                .unwrap_or_else(|| env::panic_str(error::NO_STATE)),
            Some(2) => hos_common::try_state_read::<legacy::TlaRegistryV2>()
                .map(Self::from)
                .unwrap_or_else(|| env::panic_str(error::NO_STATE)),
            Some(1) => hos_common::try_state_read::<legacy::TlaRegistryV1>()
                .map(Self::from)
                .unwrap_or_else(|| env::panic_str(error::NO_STATE)),
            Some(_) => env::panic_str(error::STATE_VERSION_UNKNOWN),
            None => env::panic_str(error::NO_STATE),
        };
        current.upgrade_proven = true;
        Event::Upgraded {
            by: env::predecessor_account_id(),
        }
        .emit();
        current
    }

    #[handle_result]
    #[payable]
    pub fn admin_set_initial_rate(&mut self, rate: U128) -> Result<(), ContractError> {
        assert_one_yocto()?;
        self.assert_council()?;
        if self.near_usd_rate_micro != 0 {
            return Err(ContractError::RateAlreadyInitialized);
        }
        if rate.0 < self.fee_config.min_near_usd_rate_micro.0
            || rate.0 > self.fee_config.max_near_usd_rate_micro.0
        {
            return Err(ContractError::RateOutOfBounds);
        }
        self.commit_rate(0, rate.0);
        Ok(())
    }

    #[handle_result]
    pub fn set_near_usd_rate(&mut self, rate: U128) -> Result<(), ContractError> {
        if env::predecessor_account_id() != self.price_oracle {
            return Err(ContractError::OnlyPriceOracle);
        }
        if self.near_usd_rate_micro == 0 {
            return Err(ContractError::RateNotInitialized);
        }
        let now = env::block_timestamp();
        if now
            < self
                .rate_updated_at
                .saturating_add(self.fee_config.rate_update_cooldown_ns.0)
        {
            return Err(ContractError::RateCooldown);
        }
        let floor = self.fee_config.min_near_usd_rate_micro.0;
        let ceiling = self.fee_config.max_near_usd_rate_micro.0;
        let previous = self.near_usd_rate_micro;
        if !pricing::rate_within_bounds(
            previous,
            rate.0,
            self.fee_config.max_rate_move_bps,
            floor,
            ceiling,
        ) {
            return Err(ContractError::RateOutOfBounds);
        }
        if !pricing::rate_is_usable(previous, floor, ceiling) {
            Event::RateRecoveredFromOutOfBand {
                stranded_micro: U128(previous),
                new_micro: rate,
                by: env::predecessor_account_id(),
            }
            .emit();
        }
        self.commit_rate(previous, rate.0);
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn set_price_oracle(&mut self, account: AccountId) -> Result<(), ContractError> {
        assert_one_yocto()?;
        self.assert_council()?;
        self.price_oracle = account.clone();
        Event::PriceOracleUpdated {
            account,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    pub fn get_near_usd_rate(&self) -> U128 {
        U128(self.near_usd_rate_micro)
    }

    pub fn get_rate_meta(&self) -> RateMetaView {
        RateMetaView {
            updated_at: U64(self.rate_updated_at),
            sequence: U64(self.rate_sequence),
        }
    }

    pub fn get_price_oracle(&self) -> AccountId {
        self.price_oracle.clone()
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
        self.end_pause();
        Event::ContractUnpaused {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn claim_refund(&mut self) -> Result<Promise, ContractError> {
        let caller = env::predecessor_account_id();
        let amount = self.pending_refunds.get(&caller).copied().unwrap_or(0);
        if amount == 0 {
            return Err(ContractError::NoPendingRefund);
        }
        if amount > self.available_balance() {
            return Err(ContractError::InsufficientContractBalance);
        }
        self.pending_refunds.remove(&caller);
        self.total_pending_refunds = self.total_pending_refunds.saturating_sub(amount);
        Ok(Promise::new(caller.clone())
            .transfer(NearToken::from_yoctonear(amount))
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_CLAIM_REFUND_CB)
                    .on_claim_refund_settled(caller, U128(amount)),
            ))
    }

    #[private]
    pub fn on_claim_refund_settled(&mut self, caller: AccountId, amount: U128) {
        if is_promise_success() {
            return;
        }
        self.add_pending_refund(&caller, amount.0);
        Event::RefundPending {
            account: caller,
            amount_yocto: amount,
            reason: "transfer_failed".to_string(),
        }
        .emit();
    }

    pub fn get_version(&self) -> u8 {
        self.version
    }

    #[handle_result]
    pub fn pause_marketplace(&mut self) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.marketplace_paused = true;
        Event::MarketplacePaused {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn unpause_marketplace(&mut self) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.marketplace_paused = false;
        Event::MarketplaceUnpaused {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    pub fn is_marketplace_paused(&self) -> bool {
        self.marketplace_paused
    }

    pub(crate) fn assert_marketplace_open(&self) -> Result<(), ContractError> {
        if self.marketplace_paused {
            return Err(ContractError::MarketplacePaused);
        }
        Ok(())
    }

    pub fn is_paused(&self) -> bool {
        self.effective_paused()
    }

    pub fn get_pause_expiry(&self) -> U64 {
        U64(if self.effective_paused() {
            self.paused_until_ns
        } else {
            0
        })
    }

    pub fn get_pending_refund(&self, account_id: AccountId) -> U128 {
        U128(self.pending_refunds.get(&account_id).copied().unwrap_or(0))
    }

    pub fn get_total_pending_refunds(&self) -> U128 {
        U128(self.total_pending_refunds)
    }

    pub fn get_council(&self) -> AccountId {
        self.council.clone()
    }

    pub fn get_treasury(&self) -> AccountId {
        self.treasury.clone()
    }

    pub fn get_suspension_expiry(&self, tla_id: AccountId) -> U64 {
        U64(self.suspension_expiry(&tla_id))
    }
}

impl TlaRegistry {
    pub(crate) fn assert_admin(&self) -> Result<(), ContractError> {
        if !self.admins.contains(&env::predecessor_account_id()) {
            return Err(ContractError::OnlyAdmin);
        }
        Ok(())
    }

    pub(crate) fn assert_council(&self) -> Result<(), ContractError> {
        if env::predecessor_account_id() != self.council {
            return Err(ContractError::OnlyCouncil);
        }
        Ok(())
    }

    pub(crate) fn assert_treasury_or_council(&self) -> Result<(), ContractError> {
        let caller = env::predecessor_account_id();
        if caller != self.treasury && caller != self.council {
            return Err(ContractError::OnlyTreasuryOrCouncil);
        }
        Ok(())
    }

    pub(crate) fn convert_usd_to_near(&self, usd_micro: u128) -> Result<u128, ContractError> {
        let rate = self.near_usd_rate_micro;
        if rate == 0 {
            return Err(ContractError::RateNotInitialized);
        }
        if rate < self.fee_config.min_near_usd_rate_micro.0
            || rate > self.fee_config.max_near_usd_rate_micro.0
        {
            return Err(ContractError::RateOutOfBounds);
        }
        if env::block_timestamp().saturating_sub(self.rate_updated_at)
            > self.fee_config.max_rate_age_ns.0
        {
            return Err(ContractError::RateStale);
        }
        if usd_micro > pricing::MAX_USD_MICRO {
            return Err(ContractError::FeeExceedsCap);
        }
        Ok(pricing::usd_micro_to_near_yocto(usd_micro, rate))
    }

    pub(crate) fn quote_usd_to_near(&self, usd_micro: u128) -> Result<u128, ContractError> {
        let required = self.convert_usd_to_near(usd_micro)?;
        Ok(pricing::quote_with_slippage(
            required,
            self.fee_config.quote_slippage_bps,
        ))
    }

    fn commit_rate(&mut self, previous: u128, new: u128) {
        self.near_usd_rate_micro = new;
        self.rate_updated_at = env::block_timestamp();
        self.rate_sequence = self.rate_sequence.saturating_add(1);
        Event::NearUsdRateUpdated {
            previous_micro: U128(previous),
            new_micro: U128(new),
            sequence: U64(self.rate_sequence),
            by: env::predecessor_account_id(),
        }
        .emit();
    }

    pub(crate) fn assert_payment_authority(&self) -> Result<AccountId, ContractError> {
        let caller = env::predecessor_account_id();
        if !self.payment_authorities.contains(&caller) {
            return Err(ContractError::OnlyPaymentAuthority);
        }
        Ok(caller)
    }

    pub(crate) fn assert_payment_authority_for(
        &self,
        tla_id: &AccountId,
    ) -> Result<AccountId, ContractError> {
        let caller = self.assert_payment_authority()?;
        let Some(allowance) = authority_allowance(&caller, tla_id) else {
            return Err(ContractError::AuthorityNotBoundToTla);
        };
        if allowance != 0 && authority_used(&caller, tla_id) >= allowance {
            return Err(ContractError::AuthorityMintAllowanceSpent);
        }
        Ok(caller)
    }

    pub(crate) fn release_authority_mint(&self, authority: &AccountId, tla_id: &AccountId) {
        let used = authority_used(authority, tla_id);
        if used == 0 {
            return;
        }
        let key = authority_used_key(authority, tla_id);
        env::storage_write(&key, &used.saturating_sub(1).to_le_bytes());
    }

    pub(crate) fn record_authority_mint(&self, authority: &AccountId, tla_id: &AccountId) {
        let used = authority_used(authority, tla_id).saturating_add(1);
        env::storage_write(&authority_used_key(authority, tla_id), &used.to_le_bytes());
        let Some(allowance) = authority_allowance(authority, tla_id) else {
            return;
        };
        let remaining = allowance.saturating_sub(used);
        if allowance != 0 && remaining <= ALLOWANCE_WARN_REMAINING {
            Event::PaymentAuthorityAllowanceLow {
                account: authority.clone(),
                tla_id: tla_id.clone(),
                remaining: U64(remaining),
            }
            .emit();
        }
    }

    pub(crate) fn assert_recovery_authority(&self) -> Result<AccountId, ContractError> {
        let caller = env::predecessor_account_id();
        if !self.recovery_authorities.contains(&caller) {
            return Err(ContractError::OnlyRecoveryAuthority);
        }
        Ok(caller)
    }

    pub(crate) fn assert_not_paused(&self) -> Result<(), ContractError> {
        if self.effective_paused() {
            return Err(ContractError::Paused);
        }
        Ok(())
    }

    pub(crate) fn effective_paused(&self) -> bool {
        self.paused && env::block_timestamp() < self.paused_until_ns
    }

    fn reclaim_floor_ns(&self) -> u64 {
        if self.effective_paused() {
            return u64::MAX;
        }
        let ended_at = if self.paused {
            self.paused_until_ns
        } else {
            self.unpaused_at
        };
        ended_at.saturating_add(self.grace_period_ns)
    }

    fn end_pause(&mut self) {
        self.paused = false;
        self.paused_until_ns = 0;
        self.unpaused_at = env::block_timestamp();
    }

    pub(crate) fn suspension_expiry(&self, tla_id: &AccountId) -> u64 {
        self.suspended_until.get(tla_id).copied().unwrap_or(0)
    }

    pub(crate) fn terms_for(&self, tla_id: &AccountId) -> TlaTerms {
        self.tla_terms.get(tla_id).cloned().unwrap_or_default()
    }

    pub(crate) fn allocation_fee_usd_micro(&self, tla_id: &AccountId) -> u128 {
        self.terms_for(tla_id)
            .allocation_fee_usd_micro
            .map_or(self.fee_config.tla_allocation_fee_usd_micro.0, |fee| fee.0)
    }

    pub(crate) fn tla_rent_usd_micro(&self, tla_id: &AccountId) -> u128 {
        self.terms_for(tla_id).tla_rent_usd_micro.map_or_else(
            || fees::base_rent(tla_id.as_str().len() as u8, &self.fee_config),
            |rent| rent.0,
        )
    }

    pub(crate) fn clock(&self) -> LifecycleClock {
        LifecycleClock {
            grace_period_ns: self.grace_period_ns,
            reclaim_floor_ns: self.reclaim_floor_ns(),
        }
    }

    pub(crate) fn add_pending_refund(&mut self, account: &AccountId, amount: u128) {
        let existing = self.pending_refunds.get(account).copied().unwrap_or(0);
        self.pending_refunds
            .insert(account.clone(), existing.saturating_add(amount));
        self.total_pending_refunds = self.total_pending_refunds.saturating_add(amount);
    }

    pub(crate) fn refund_excess(&mut self, payer: &AccountId, attached: u128, charged: u128) {
        let excess = attached.saturating_sub(charged);
        if excess > 0 {
            self.add_pending_refund(payer, excess);
        }
    }

    pub(crate) fn available_balance(&self) -> u128 {
        let total = env::account_balance().as_yoctonear();
        let reserve = env::storage_byte_cost()
            .as_yoctonear()
            .saturating_mul(env::storage_usage() as u128);
        total.saturating_sub(reserve)
    }
}

pub(crate) fn assert_one_yocto() -> Result<(), ContractError> {
    if env::attached_deposit() != NearToken::from_yoctonear(1) {
        return Err(ContractError::RequiresOneYocto);
    }
    Ok(())
}
