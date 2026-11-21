use crate::types::Tier;
use crate::BPS_DENOMINATOR;
use soroban_sdk::Vec;

/// Index of the highest tier whose `min_units` is reached, or `None` below MOQ.
pub fn tier_index(tiers: &Vec<Tier>, total_units: u32) -> Option<u32> {
    let mut found = None;
    for i in 0..tiers.len() {
        if let Some(t) = tiers.get(i) {
            if total_units >= t.min_units {
                found = Some(i);
            }
        }
    }
    found
}

/// Unit price a committer pays: the price of the tier the pool is in before
/// their units, i.e. the highest price they could end up paying.
pub fn ceiling_price(tiers: &Vec<Tier>, total_before: u32) -> Option<i128> {
    let idx = tier_index(tiers, total_before).unwrap_or(0);
    tiers.get(idx).map(|t| t.unit_price)
}

/// Final unit price for a pool that reached `total_units` (at least MOQ).
pub fn final_unit_price(tiers: &Vec<Tier>, total_units: u32) -> Option<i128> {
    ceiling_price(tiers, total_units)
}

/// `a * b / c` rounded down, with overflow and zero-divisor checks.
pub fn mul_div_floor(a: i128, b: i128, c: i128) -> Option<i128> {
    if c == 0 {
        return None;
    }
    a.checked_mul(b)?.checked_div(c)
}

pub fn bp_of(amount: i128, bp: u32) -> Option<i128> {
    mul_div_floor(amount, bp as i128, BPS_DENOMINATOR as i128)
}

/// Fees are charged against the supplier payable and always round down.
pub fn round_supplier_payable(amount: i128, bp: u32) -> Option<i128> {
    bp_of(amount, bp)
}

/// A member's proportional refund rounds up so fractional dust is not retained.
pub fn round_member_refund(amount: i128, numerator: u32, denominator: u32) -> Option<i128> {
    if denominator == 0 {
        return None;
    }
    let product = amount.checked_mul(numerator as i128)?;
    let divisor = denominator as i128;
    Some(product / divisor + i128::from(product % divisor != 0))
}

/// Deterministic shortfall allocation. Members are walked in commit order with
/// a running cumulative unit count; each gets
/// `floor(cum_after * received / total) - floor(cum_before * received / total)`,
/// so the allocations always sum to exactly `received`.
pub fn cumulative_alloc(cum_before: u32, units: u32, received: u32, total: u32) -> Option<u32> {
    if total == 0 {
        return None;
    }
    let after = (cum_before as u64).checked_add(units as u64)?;
    let hi = after.checked_mul(received as u64)? / total as u64;
    let lo = (cum_before as u64).checked_mul(received as u64)? / total as u64;
    u32::try_from(hi.checked_sub(lo)?).ok()
}

/// Tiers must be 1..=5, ascending `min_units`, strictly descending price, MOQ first.
pub fn validate_tiers(tiers: &Vec<Tier>, moq: u32, max_units: u32) -> bool {
    let n = tiers.len();
    if n == 0 || n > crate::types::caps::MAX_TIERS {
        return false;
    }
    let mut prev: Option<Tier> = None;
    for i in 0..n {
        let Some(t) = tiers.get(i) else { return false };
        if t.unit_price <= 0 || t.min_units == 0 {
            return false;
        }
        if let Some(p) = prev {
            if t.min_units <= p.min_units || t.unit_price >= p.unit_price {
                return false;
            }
        }
        prev = Some(t);
    }
    match tiers.get(0) {
        Some(first) => first.min_units == moq && max_units >= moq,
        None => false,
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use proptest::prelude::*;
    use soroban_sdk::{vec, Env};

    fn tiers(env: &Env) -> Vec<Tier> {
        vec![
            env,
            Tier {
                min_units: 10,
                unit_price: 100,
            },
            Tier {
                min_units: 50,
                unit_price: 90,
            },
            Tier {
                min_units: 100,
                unit_price: 80,
            },
        ]
    }

    #[test]
    fn tier_lookup() {
        let env = Env::default();
        let t = tiers(&env);
        assert_eq!(tier_index(&t, 9), None);
        assert_eq!(tier_index(&t, 10), Some(0));
        assert_eq!(tier_index(&t, 99), Some(1));
        assert_eq!(tier_index(&t, 100), Some(2));
        assert_eq!(ceiling_price(&t, 0), Some(100));
        assert_eq!(ceiling_price(&t, 60), Some(90));
        assert_eq!(final_unit_price(&t, 500), Some(80));
    }

    #[test]
    fn allocation_sums_to_received() {
        let units = [7u32, 13, 1, 29, 50];
        let total: u32 = units.iter().sum();
        for received in 0..=total {
            let mut cum = 0;
            let mut sum = 0;
            for u in units {
                sum += cumulative_alloc(cum, u, received, total).unwrap();
                cum += u;
            }
            assert_eq!(sum, received);
        }
    }

    #[test]
    fn allocation_preserves_commit_order_for_leftovers() {
        let units = [60u32, 60, 100, 200];
        let total: u32 = units.iter().sum();
        let mut cumulative = 0;
        let mut allocations = Vec::new();

        for units in units {
            allocations.push(cumulative_alloc(cumulative, units, 350, total).unwrap());
            cumulative += units;
        }

        assert_eq!(allocations, vec![50, 50, 83, 167]);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10_000))]
        #[test]
        fn allocation_is_bounded_and_conserves_received_units(
            units in proptest::collection::vec(1u32..=1_000, 1..=32),
            received in 0u32..=32_000,
        ) {
            let total: u32 = units.iter().sum();
            prop_assume!(received <= total);
            let mut cumulative = 0;
            let mut allocated = 0;
            for units in units {
                let share = cumulative_alloc(cumulative, units, received, total).unwrap();
                prop_assert!(share <= units);
                allocated += share;
                cumulative += units;
            }
            prop_assert_eq!(allocated, received);
        }
    }

    #[test]
    fn rounding_policy_is_explicit() {
        assert_eq!(round_supplier_payable(101, 333), Some(3));
        assert_eq!(round_member_refund(101, 1, 3), Some(34));
        assert_eq!(round_member_refund(100, 1, 4), Some(25));
        assert_eq!(round_member_refund(100, 1, 0), None);
    }

    #[test]
    fn tier_validation() {
        let env = Env::default();
        assert!(validate_tiers(&tiers(&env), 10, 200));
        assert!(!validate_tiers(&tiers(&env), 11, 200));
        assert!(!validate_tiers(&tiers(&env), 10, 5));
        let bad = vec![
            &env,
            Tier {
                min_units: 10,
                unit_price: 100,
            },
            Tier {
                min_units: 50,
                unit_price: 100,
            },
        ];
        assert!(!validate_tiers(&bad, 10, 200));
        assert!(!validate_tiers(&Vec::new(&env), 10, 200));
    }

    #[test]
    fn mul_div_checks() {
        assert_eq!(mul_div_floor(10, 3, 4), Some(7));
        assert_eq!(mul_div_floor(1, 1, 0), None);
        assert_eq!(mul_div_floor(i128::MAX, 2, 1), None);
        assert_eq!(bp_of(10_000, 150), Some(150));
    }
}
