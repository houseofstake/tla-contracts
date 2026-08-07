use crate::error::{ContractError, NameInvalidReason};
use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::json_types::{U128, U64};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, AccountId};

pub const ONE_NEAR: u128 = 1_000_000_000_000_000_000_000_000;
pub const ONE_YEAR_NS: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, PartialEq)]
#[borsh(crate = "near_sdk::borsh")]
#[serde(crate = "near_sdk::serde")]
pub enum TlaType {
    Business,
    Open,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, PartialEq)]
#[borsh(crate = "near_sdk::borsh")]
#[serde(crate = "near_sdk::serde")]
pub enum TlaStatus {
    Registered,
    Active,
    Suspended,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, PartialEq)]
#[borsh(crate = "near_sdk::borsh")]
#[serde(crate = "near_sdk::serde")]
pub enum PremiumCategory {
    Legendary,
    Premium,
    Standard,
    Community,
}

impl PremiumCategory {
    pub fn multiplier(&self) -> (u128, u128) {
        match self {
            Self::Legendary => (5, 1),
            Self::Premium => (3, 1),
            Self::Standard => (3, 2),
            Self::Community => (0, 1),
        }
    }
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
pub struct TlaEntry {
    pub tla_type: TlaType,
    pub status: TlaStatus,
    pub licensee: Option<AccountId>,
    pub premium_category: PremiumCategory,
    pub activated_at: u64,
    pub expires_at: u64,
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
pub struct SubAccountEntry {
    pub owner: AccountId,
    pub tla_id: AccountId,
    pub payout_account: AccountId,
    pub rented_at: u64,
    pub expires_at: u64,
    pub retraction_at: Option<u64>,
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
pub struct ActivityRecord {
    pub event: String,
    pub account: String,
    pub block_height: u64,
    pub block_timestamp: u64,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct ActivityView {
    pub event: String,
    pub account: String,
    pub block_height: U64,
    pub block_timestamp: U64,
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
pub struct Listing {
    pub price: u128,
    pub settling: bool,
    pub seller: AccountId,
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
pub struct ParkedEntry {
    pub tla_id: AccountId,
    pub parked_at: u64,
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
pub struct AcceptedOffer {
    pub buyer: AccountId,
    pub price: u128,
    pub settling: bool,
    pub seller: AccountId,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
#[serde(crate = "near_sdk::serde")]
pub struct FeeConfig {
    pub tla_allocation_fee_usd_micro: U128,
    pub rent_tier_5_usd_micro: U128,
    pub rent_tier_8_usd_micro: U128,
    pub rent_tier_10_usd_micro: U128,
    pub rent_tier_12plus_usd_micro: U128,
    pub sub_fee_per_account_usd_micro: U128,
    pub account_creation_deposit_yocto: U128,
    pub business_max_subs: u32,
    pub retraction_notice_ns: U64,
    pub resale_commission_bps: u16,
    pub max_rate_move_bps: u16,
    pub quote_slippage_bps: u16,
    pub min_near_usd_rate_micro: U128,
    pub max_near_usd_rate_micro: U128,
    pub rate_update_cooldown_ns: U64,
    pub max_rate_age_ns: U64,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub enum LifecycleStatus {
    Registered,
    Active,
    Grace,
    Reclaimable,
    Suspended,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct TlaView {
    pub tla_id: AccountId,
    pub tla_type: TlaType,
    pub lifecycle: LifecycleStatus,
    pub licensee: Option<AccountId>,
    pub premium_category: PremiumCategory,
    pub activated_at: U64,
    pub expires_at: U64,
    pub annual_rent: U128,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct SubAccountView {
    pub full_name: String,
    pub owner: AccountId,
    pub tla_id: AccountId,
    pub payout_account: AccountId,
    pub lifecycle: LifecycleStatus,
    pub rented_at: U64,
    pub expires_at: U64,
    pub annual_rent: U128,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct ListingView {
    pub full_name: String,
    pub price_yocto: U128,
    pub settling: bool,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct AcceptedOfferView {
    pub full_name: String,
    pub buyer: AccountId,
    pub price_yocto: U128,
    pub settling: bool,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct SubAccountDetailView {
    pub sub_account: SubAccountView,
    pub listing: Option<ListingView>,
    pub accepted_offer: Option<AcceptedOfferView>,
    pub retraction_at: Option<U64>,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct RentPriceView {
    pub rent_yocto: U128,
    pub creation_deposit_yocto: U128,
    pub total_yocto: U128,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct RegistryStats {
    pub tla_count: u64,
    pub sub_account_count: u64,
    pub total_revenue_yocto: U128,
    pub total_pending_refunds_yocto: U128,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct RateMetaView {
    pub updated_at: U64,
    pub sequence: U64,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct BusinessRenewalCostView {
    pub tla_id: AccountId,
    pub tla_rent_yocto: U128,
    pub per_sub_yocto: U128,
    pub sub_count: u32,
}

pub struct LifecycleClock {
    pub grace_period_ns: u64,
    pub reclaim_floor_ns: u64,
}

impl LifecycleClock {
    pub fn reclaimable_at(&self, from: u64) -> u64 {
        from.saturating_add(self.grace_period_ns)
            .max(self.reclaim_floor_ns)
    }
}

fn time_lifecycle(expires_at: u64, clock: &LifecycleClock) -> LifecycleStatus {
    let now = env::block_timestamp();
    if now < expires_at {
        LifecycleStatus::Active
    } else if now < clock.reclaimable_at(expires_at) {
        LifecycleStatus::Grace
    } else {
        LifecycleStatus::Reclaimable
    }
}

impl TlaEntry {
    pub fn lifecycle(&self, clock: &LifecycleClock) -> LifecycleStatus {
        match self.status {
            TlaStatus::Registered => LifecycleStatus::Registered,
            TlaStatus::Suspended => LifecycleStatus::Suspended,
            TlaStatus::Active => time_lifecycle(self.expires_at, clock),
        }
    }

    pub fn accepting_rentals(&self, suspended_until: u64) -> bool {
        if env::block_timestamp() >= self.expires_at {
            return false;
        }
        match self.status {
            TlaStatus::Active => true,
            TlaStatus::Suspended => env::block_timestamp() >= suspended_until,
            TlaStatus::Registered => false,
        }
    }
}

impl SubAccountEntry {
    pub fn lifecycle(&self, clock: &LifecycleClock) -> LifecycleStatus {
        time_lifecycle(self.expires_at, clock)
    }

    pub fn sweepable(&self) -> bool {
        env::block_timestamp() >= self.expires_at
    }
}

pub fn validate_name(name: &str) -> Result<(), ContractError> {
    if name.is_empty() || name.len() > 60 {
        return Err(ContractError::InvalidName {
            reason: NameInvalidReason::LengthOutOfBounds,
        });
    }
    for c in name.bytes() {
        if !matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_') {
            return Err(ContractError::InvalidName {
                reason: NameInvalidReason::DisallowedCharacter,
            });
        }
    }
    if name.starts_with('-') || name.starts_with('_') || name.ends_with('-') || name.ends_with('_')
    {
        return Err(ContractError::InvalidName {
            reason: NameInvalidReason::EdgeSeparator,
        });
    }
    let bytes = name.as_bytes();
    for pair in bytes.windows(2) {
        if matches!(pair[0], b'-' | b'_') && matches!(pair[1], b'-' | b'_') {
            return Err(ContractError::InvalidName {
                reason: NameInvalidReason::EdgeSeparator,
            });
        }
    }
    Ok(())
}

pub fn sub_account_key(tla_id: &AccountId, name: &str) -> String {
    format!("{}.{}", name, tla_id)
}

pub fn total_name_length(tla_id: &AccountId, name: &str) -> u8 {
    (name.len() + 1 + tla_id.as_str().len()) as u8
}
