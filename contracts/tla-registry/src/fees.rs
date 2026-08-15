use crate::pricing::USD_MICRO_PER_DOLLAR;
use crate::types::{total_name_length, FeeConfig, PremiumCategory, TlaEntry, TlaType, ONE_NEAR};
use near_sdk::json_types::{U128, U64};
use near_sdk::AccountId;

pub fn base_rent(total_len: u8, config: &FeeConfig) -> u128 {
    match total_len {
        0..=5 => config.rent_tier_5_usd_micro.0,
        6..=8 => config.rent_tier_8_usd_micro.0,
        9..=10 => config.rent_tier_10_usd_micro.0,
        _ => config.rent_tier_12plus_usd_micro.0,
    }
}

pub fn sub_account_rent(total_len: u8, premium: &PremiumCategory, config: &FeeConfig) -> u128 {
    let base = base_rent(total_len, config);
    let (num, den) = premium.multiplier();
    base.saturating_mul(num) / den
}

pub fn calculate_rent(tla: &TlaEntry, tla_id: &AccountId, name: &str, config: &FeeConfig) -> u128 {
    match tla.tla_type {
        TlaType::Business => config.sub_fee_per_account_usd_micro.0,
        TlaType::Open => {
            let total_len = total_name_length(tla_id, name);
            sub_account_rent(total_len, &tla.premium_category, config)
        }
    }
}

pub fn default_fee_config() -> FeeConfig {
    FeeConfig {
        tla_allocation_fee_usd_micro: U128(1000 * USD_MICRO_PER_DOLLAR),
        rent_tier_5_usd_micro: U128(50 * USD_MICRO_PER_DOLLAR),
        rent_tier_8_usd_micro: U128(20 * USD_MICRO_PER_DOLLAR),
        rent_tier_10_usd_micro: U128(10 * USD_MICRO_PER_DOLLAR),
        rent_tier_12plus_usd_micro: U128(5 * USD_MICRO_PER_DOLLAR),
        sub_fee_per_account_usd_micro: U128(USD_MICRO_PER_DOLLAR / 2),
        account_creation_deposit_yocto: U128(ONE_NEAR / 100),
        business_max_subs: 1000,
        retraction_notice_ns: U64(7 * 24 * 60 * 60 * 1_000_000_000),
        resale_commission_bps: 250,
        max_rate_move_bps: 2_000,
        quote_slippage_bps: 527,
        min_near_usd_rate_micro: U128(100_000),
        max_near_usd_rate_micro: U128(100_000_000),
        rate_update_cooldown_ns: U64(300 * 1_000_000_000),
        max_rate_age_ns: U64(6 * 60 * 60 * 1_000_000_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_rent_non_increasing_and_boundaries() {
        let config = default_fee_config();
        let mut prev = u128::MAX;
        for len in 0u8..=64 {
            let rent = base_rent(len, &config);
            assert!(
                rent <= prev,
                "base_rent must not increase with name length (len={len})"
            );
            prev = rent;
        }
        assert_eq!(base_rent(0, &config), config.rent_tier_5_usd_micro.0);
        assert_eq!(base_rent(5, &config), config.rent_tier_5_usd_micro.0);
        assert_eq!(base_rent(6, &config), config.rent_tier_8_usd_micro.0);
        assert_eq!(base_rent(8, &config), config.rent_tier_8_usd_micro.0);
        assert_eq!(base_rent(9, &config), config.rent_tier_10_usd_micro.0);
        assert_eq!(base_rent(10, &config), config.rent_tier_10_usd_micro.0);
        assert_eq!(base_rent(11, &config), config.rent_tier_12plus_usd_micro.0);
        assert_eq!(base_rent(64, &config), config.rent_tier_12plus_usd_micro.0);
    }
}
