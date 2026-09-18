use crate::pricing::USD_MICRO_PER_DOLLAR;
use crate::types::{FeeConfig, PremiumCategory, TlaEntry, TlaTerms, TlaType, ONE_NEAR};
use near_sdk::json_types::{U128, U64};
use near_sdk::AccountId;

pub fn base_rent(label_len: u8, config: &FeeConfig) -> u128 {
    match label_len {
        0..=5 => config.rent_tier_5_usd_micro.0,
        6..=8 => config.rent_tier_8_usd_micro.0,
        9..=10 => config.rent_tier_10_usd_micro.0,
        _ => config.rent_tier_12plus_usd_micro.0,
    }
}

pub fn sub_account_rent(label_len: u8, premium: &PremiumCategory, config: &FeeConfig) -> u128 {
    let base = base_rent(label_len, config);
    let (num, den) = premium.multiplier();
    base.saturating_mul(num) / den
}

pub fn calculate_rent(
    tla: &TlaEntry,
    _tla_id: &AccountId,
    name: &str,
    config: &FeeConfig,
    terms: &TlaTerms,
) -> u128 {
    match tla.tla_type {
        TlaType::Business => terms
            .sub_fee_usd_micro
            .map_or(config.sub_fee_per_account_usd_micro.0, |fee| fee.0),
        TlaType::Open => {
            let label_len = u8::try_from(name.len()).unwrap_or(0);
            sub_account_rent(label_len, &tla.premium_category, config)
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
    use crate::types::TlaStatus;

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

    #[test]
    fn the_same_label_costs_the_same_under_any_namespace() {
        let config = default_fee_config();
        let terms = TlaTerms {
            allocation_fee_usd_micro: None,
            tla_rent_usd_micro: None,
            sub_fee_usd_micro: None,
        };
        let tla = TlaEntry {
            tla_type: TlaType::Open,
            status: TlaStatus::Active,
            licensee: None,
            premium_category: PremiumCategory::Standard,
            activated_at: 0,
            expires_at: u64::MAX,
        };
        let short: AccountId = "hos".parse().unwrap();
        let long: AccountId = "a-much-longer-namespace".parse().unwrap();
        assert_eq!(
            calculate_rent(&tla, &short, "alice", &config, &terms),
            calculate_rent(&tla, &long, "alice", &config, &terms)
        );
        assert!(
            calculate_rent(&tla, &short, "ab", &config, &terms)
                > calculate_rent(&tla, &short, "abcdefghijk", &config, &terms)
        );
    }
}
