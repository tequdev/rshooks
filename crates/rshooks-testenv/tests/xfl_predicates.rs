//! Cross-checks `XFL::is_zero`/`is_strictly_positive`/`is_strictly_negative`
//! (pure local bit tests, no host call) against the mock host's
//! `float_compare` oracle (`COMPARE_EQUAL`/`COMPARE_GREATER`/`COMPARE_LESS`
//! against zero), for values only real host arithmetic can produce
//! (`float_sum`, `float_negate`, `float_multiply`, `float_divide`).

#![allow(clippy::arithmetic_side_effects, clippy::eq_op, missing_docs)]

use rshooks::exit::HookResult;
use rshooks::prelude::*;
use rshooks::*;
use rshooks_testenv::prelude::*;

#[hooks]
pub struct XflPredicateCheck;

#[hooks]
impl XflPredicateCheck {
    /// Computes several XFL values via real host arithmetic (`+`, `-`, `-`
    /// unary, `*`, `/`), then checks that `is_zero`/`is_strictly_positive`/
    /// `is_strictly_negative` agree with `float_compare` against zero for
    /// every one of them. Reports `0` on full agreement, or `100 + index`
    /// for the first case that disagrees, so a failing assertion in the
    /// Rust-side test names the offending case.
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let zero = XFL::from_raw_bits(0);

        let Ok(one) = XFL::new(-15, 1_000_000_000_000_000) else {
            accept!(b"", 1)
        };
        let Ok(two) = one + one else { accept!(b"", 2) };
        let Ok(neg_one) = -one else { accept!(b"", 3) };
        // Arithmetic result that lands exactly on zero.
        let Ok(cancelled) = one - one else {
            accept!(b"", 4)
        };
        let Ok(product) = one * neg_one else {
            accept!(b"", 5)
        };
        let Ok(quotient) = neg_one / two else {
            accept!(b"", 6)
        };

        let cases = [zero, one, two, neg_one, cancelled, product, quotient];
        for (idx, value) in cases.into_iter().enumerate() {
            let oracle_zero = value.compare(zero, COMPARE_EQUAL);
            let oracle_pos = value.compare(zero, COMPARE_GREATER);
            let oracle_neg = value.compare(zero, COMPARE_LESS);

            if Ok(value.is_zero()) != oracle_zero {
                accept!(b"", 100 + idx as i64);
            }
            if value.is_strictly_positive() != oracle_pos {
                accept!(b"", 200 + idx as i64);
            }
            if value.is_strictly_negative() != oracle_neg {
                accept!(b"", 300 + idx as i64);
            }
        }

        accept!(b"", 0)
    }
}

#[test]
fn predicates_agree_with_float_compare_for_real_arithmetic_results() {
    let env = TestEnv::new();
    let exit = env.invoke::<XflPredicateCheck>(0);
    assert_eq!(
        exit.code, 0,
        "predicate/float_compare disagreement (or setup failure) -- see \
         the hook body's code map for what `exit.code` means"
    );
}
