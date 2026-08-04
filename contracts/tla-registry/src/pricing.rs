use near_sdk::env;

pub const YOCTO_PER_NEAR: u128 = 1_000_000_000_000_000_000_000_000;
pub const USD_MICRO_PER_DOLLAR: u128 = 1_000_000;
pub const MAX_USD_MICRO: u128 = 100_000_000_000_000;
pub const MAX_NEAR_USD_RATE_MICRO: u128 = 1_000_000_000_000;
pub const BPS_DENOMINATOR: u128 = 10_000;

const RATE_ZERO: &str = "near/usd rate is zero";
const AGGREGATE_CAP: &str = "usd amount exceeds the conversion cap";
const PRICE_OVERFLOW: &str = "price conversion overflow";

pub fn usd_micro_to_near_yocto(usd_micro: u128, near_usd_rate_micro: u128) -> u128 {
    if near_usd_rate_micro == 0 {
        env::panic_str(RATE_ZERO);
    }
    if usd_micro > MAX_USD_MICRO {
        env::panic_str(AGGREGATE_CAP);
    }
    usd_micro
        .checked_mul(YOCTO_PER_NEAR)
        .map(|scaled| scaled / near_usd_rate_micro)
        .unwrap_or_else(|| env::panic_str(PRICE_OVERFLOW))
}

pub fn quote_with_slippage(amount_yocto: u128, slippage_bps: u16) -> u128 {
    let bps = u128::from(slippage_bps);
    if bps > BPS_DENOMINATOR {
        env::panic_str("slippage bps out of range");
    }
    let quotient = amount_yocto / BPS_DENOMINATOR;
    let remainder = amount_yocto % BPS_DENOMINATOR;
    let whole = quotient
        .checked_mul(bps)
        .unwrap_or_else(|| env::panic_str(PRICE_OVERFLOW));
    let partial_numerator = remainder * bps;
    let partial = partial_numerator / BPS_DENOMINATOR
        + u128::from(!partial_numerator.is_multiple_of(BPS_DENOMINATOR));
    let buffer = whole
        .checked_add(partial)
        .unwrap_or_else(|| env::panic_str(PRICE_OVERFLOW));
    amount_yocto
        .checked_add(buffer)
        .unwrap_or_else(|| env::panic_str(PRICE_OVERFLOW))
}

pub fn rate_lower_bound(current: u128, move_bps: u16) -> u128 {
    let numerator = current * (BPS_DENOMINATOR - u128::from(move_bps));
    numerator.div_ceil(BPS_DENOMINATOR)
}

pub fn rate_upper_bound(current: u128, move_bps: u16) -> u128 {
    current * (BPS_DENOMINATOR + u128::from(move_bps)) / BPS_DENOMINATOR
}

pub fn rate_within_bounds(
    current: u128,
    new: u128,
    move_bps: u16,
    floor: u128,
    ceiling: u128,
) -> bool {
    if u128::from(move_bps) > BPS_DENOMINATOR {
        return false;
    }
    if new < floor || new > ceiling {
        return false;
    }
    if current == 0 {
        return true;
    }
    new >= rate_lower_bound(current, move_bps) && new <= rate_upper_bound(current, move_bps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dollars(d: u128) -> u128 {
        d * USD_MICRO_PER_DOLLAR
    }

    fn near(n: u128) -> u128 {
        n * YOCTO_PER_NEAR
    }

    #[test]
    fn fifty_dollars_at_five_dollar_near_is_ten_near() {
        assert_eq!(usd_micro_to_near_yocto(dollars(50), dollars(5)), near(10));
    }

    #[test]
    fn cheaper_near_costs_more_near() {
        let at_5 = usd_micro_to_near_yocto(dollars(50), dollars(5));
        let at_2 = usd_micro_to_near_yocto(dollars(50), dollars(2));
        assert!(at_2 > at_5);
        assert_eq!(at_2, near(25));
    }

    #[test]
    fn fractional_dollar_rate_is_exact() {
        assert_eq!(usd_micro_to_near_yocto(dollars(85), 4_250_000), near(20));
    }

    #[test]
    fn truncation_error_is_below_one_yocto_scale() {
        let rate = 3_333_333;
        assert_eq!(
            usd_micro_to_near_yocto(dollars(10), rate),
            dollars(10) * YOCTO_PER_NEAR / rate
        );
    }

    #[test]
    #[should_panic(expected = "near/usd rate is zero")]
    fn zero_rate_panics() {
        let _ = usd_micro_to_near_yocto(dollars(1), 0);
    }

    #[test]
    #[should_panic(expected = "usd amount exceeds the conversion cap")]
    fn aggregate_over_cap_panics() {
        let _ = usd_micro_to_near_yocto(MAX_USD_MICRO + 1, dollars(5));
    }

    #[test]
    fn slippage_rounds_up_not_down() {
        assert_eq!(quote_with_slippage(near(10), 0), near(10));
        assert_eq!(quote_with_slippage(1_000_000, 500), 1_050_000);
        assert_eq!(quote_with_slippage(9_999, 1), 10_000);
        assert_eq!(quote_with_slippage(1, 1), 2);
    }

    #[test]
    fn slippage_is_overflow_safe_at_extreme_inputs() {
        let amount = MAX_USD_MICRO * YOCTO_PER_NEAR / 10_000;
        let quoted = quote_with_slippage(amount, 2_500);
        assert!(quoted >= amount);
        assert_eq!(quoted, amount + amount.div_ceil(4));
    }

    fn quote_covers_drop(slippage_bps: u16, drop_bps: u128) -> bool {
        let usd = dollars(50);
        let quoted = quote_with_slippage(usd_micro_to_near_yocto(usd, dollars(5)), slippage_bps);
        let rate_at_pay = dollars(5) * (10_000 - drop_bps) / 10_000;
        quoted >= usd_micro_to_near_yocto(usd, rate_at_pay)
    }

    #[test]
    fn slippage_500bps_covers_up_to_476_bps_drop() {
        assert!(quote_covers_drop(500, 476));
        assert!(!quote_covers_drop(500, 477));
        assert!(!quote_covers_drop(500, 500));
    }

    #[test]
    fn slippage_527bps_covers_a_full_500bps_drop() {
        assert!(quote_covers_drop(527, 500));
    }

    #[test]
    fn slippage_2500bps_covers_a_full_2000bps_drop() {
        assert!(quote_covers_drop(2_500, 2_000));
        assert!(!quote_covers_drop(2_000, 2_000));
    }

    #[test]
    fn lower_bound_rounds_up_to_hold_the_percentage() {
        assert_eq!(rate_lower_bound(3, 2_000), 3);
        assert_eq!(rate_lower_bound(dollars(5), 2_000), 4_000_000);
    }

    #[test]
    fn tiny_rate_cannot_move_beyond_band_via_flooring() {
        let floor = 1;
        let ceiling = MAX_NEAR_USD_RATE_MICRO;
        assert!(!rate_within_bounds(3, 2, 2_000, floor, ceiling));
    }

    #[test]
    fn absolute_floor_and_ceiling_enforced() {
        let floor = dollars(1);
        let ceiling = dollars(50);
        assert!(!rate_within_bounds(0, floor - 1, 2_000, floor, ceiling));
        assert!(!rate_within_bounds(0, ceiling + 1, 2_000, floor, ceiling));
        assert!(rate_within_bounds(0, dollars(5), 2_000, floor, ceiling));
    }

    #[test]
    fn move_within_band_accepted_beyond_band_rejected() {
        let (floor, ceiling) = (1, MAX_NEAR_USD_RATE_MICRO);
        let current = dollars(5);
        assert!(rate_within_bounds(
            current,
            dollars(6),
            2_000,
            floor,
            ceiling
        ));
        assert!(rate_within_bounds(
            current,
            dollars(4),
            2_000,
            floor,
            ceiling
        ));
        assert!(!rate_within_bounds(
            current,
            current * 12_001 / 10_000,
            2_000,
            floor,
            ceiling
        ));
        assert!(!rate_within_bounds(
            current,
            current * 7_999 / 10_000,
            2_000,
            floor,
            ceiling
        ));
    }

    #[test]
    fn out_of_range_move_bps_rejected() {
        let (floor, ceiling) = (1, MAX_NEAR_USD_RATE_MICRO);
        assert!(!rate_within_bounds(
            dollars(5),
            dollars(5),
            10_001,
            floor,
            ceiling
        ));
    }
}
