use std::ops::Deref;

use near_sdk::{near, require};
use templar_primitives::number::Decimal;

pub trait UsageCurve {
    fn at(&self, usage_ratio: Decimal) -> Decimal;
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum InterestRateStrategy {
    Linear(Linear),
    #[cfg_attr(not(target_arch = "wasm32"), schemars(with = "PiecewiseParams"))]
    Piecewise(Piecewise),
    #[cfg_attr(not(target_arch = "wasm32"), schemars(with = "Exponential2Params"))]
    Exponential2(Exponential2),
}

impl InterestRateStrategy {
    pub const fn zero() -> Self {
        Self::Linear(Linear {
            base: Decimal::ZERO,
            top: Decimal::ZERO,
        })
    }

    #[must_use]
    pub fn linear(base: Decimal, top: Decimal) -> Option<Self> {
        Some(Self::Linear(Linear::new(base, top)?))
    }

    #[must_use]
    pub fn piecewise(
        base: Decimal,
        optimal: Decimal,
        rate_1: Decimal,
        rate_2: Decimal,
    ) -> Option<Self> {
        Some(Self::Piecewise(Piecewise::new(
            base, optimal, rate_1, rate_2,
        )?))
    }

    #[must_use]
    pub fn exponential2(base: Decimal, top: Decimal, eccentricity: Decimal) -> Option<Self> {
        Some(Self::Exponential2(Exponential2::new(
            base,
            top,
            eccentricity,
        )?))
    }
}

impl Deref for InterestRateStrategy {
    type Target = dyn UsageCurve;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Linear(linear) => linear as &dyn UsageCurve,
            Self::Piecewise(piecewise) => piecewise as &dyn UsageCurve,
            Self::Exponential2(exponential2) => exponential2 as &dyn UsageCurve,
        }
    }
}

/// ```text,no_run
/// r(u) = u * (t - b) + b
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct Linear {
    base: Decimal,
    top: Decimal,
}

impl Linear {
    pub fn new(base: Decimal, top: Decimal) -> Option<Self> {
        (base <= top).then_some(Self { base, top })
    }
}

impl UsageCurve for Linear {
    fn at(&self, usage_ratio: Decimal) -> Decimal {
        usage_ratio * (self.top - self.base) + self.base
    }
}

/// ```text,no_run
/// r(u) = {
///     if u < o : r_1 * u + b,
///     else     : r_2 * u + o * (r_1 - r_2) + b
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
#[serde(try_from = "PiecewiseParams", into = "PiecewiseParams")]
pub struct Piecewise {
    params: PiecewiseParams,
    i_negative_rate_2_b: Decimal,
}

impl Piecewise {
    pub fn new(base: Decimal, optimal: Decimal, rate_1: Decimal, rate_2: Decimal) -> Option<Self> {
        if optimal > 1u32 {
            return None;
        }

        if rate_1 > rate_2 {
            return None;
        }

        Some(Self {
            i_negative_rate_2_b: optimal * (rate_2 - rate_1) - base,
            params: PiecewiseParams {
                base,
                optimal,
                rate_1,
                rate_2,
            },
        })
    }
}

impl UsageCurve for Piecewise {
    fn at(&self, usage_ratio: Decimal) -> Decimal {
        require!(
            usage_ratio <= Decimal::ONE,
            "Invariant violation: Usage ratio cannot be over 100%.",
        );

        if usage_ratio < self.params.optimal {
            self.params.rate_1 * usage_ratio + self.params.base
        } else {
            self.params.rate_2 * usage_ratio - self.i_negative_rate_2_b
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct PiecewiseParams {
    base: Decimal,
    optimal: Decimal,
    rate_1: Decimal,
    rate_2: Decimal,
}

impl TryFrom<PiecewiseParams> for Piecewise {
    type Error = &'static str;

    fn try_from(
        PiecewiseParams {
            base,
            optimal,
            rate_1,
            rate_2,
        }: PiecewiseParams,
    ) -> Result<Self, Self::Error> {
        Self::new(base, optimal, rate_1, rate_2).ok_or("Invalid Piecewise parameters")
    }
}

impl From<Piecewise> for PiecewiseParams {
    fn from(value: Piecewise) -> Self {
        value.params
    }
}

/// ```text,no_run
/// r(u) = b + (t - b) * (2^ku - 1) / (2^k - 1)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
#[serde(try_from = "Exponential2Params", into = "Exponential2Params")]
pub struct Exponential2 {
    params: Exponential2Params,
    i_factor: Decimal,
}

impl Exponential2 {
    /// # Panics
    /// - If 2^eccentricity overflows `Decimal`.
    pub fn new(base: Decimal, top: Decimal, eccentricity: Decimal) -> Option<Self> {
        if base > top {
            return None;
        }

        if eccentricity > 24u32 || eccentricity.is_zero() {
            return None;
        }

        #[allow(clippy::unwrap_used, reason = "Invariant checked above")]
        Some(Self {
            i_factor: (top - base) / (eccentricity.pow2().unwrap() - 1u32),
            params: Exponential2Params {
                base,
                top,
                eccentricity,
            },
        })
    }
}

impl UsageCurve for Exponential2 {
    fn at(&self, usage_ratio: Decimal) -> Decimal {
        require!(
            usage_ratio <= Decimal::ONE,
            "Invariant violation: Usage ratio cannot be over 100%.",
        );

        #[allow(clippy::unwrap_used, reason = "Invariant checked above")]
        (self.params.base
            + self.i_factor * ((self.params.eccentricity * usage_ratio).pow2().unwrap() - 1u32))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct Exponential2Params {
    base: Decimal,
    top: Decimal,
    eccentricity: Decimal,
}

impl TryFrom<Exponential2Params> for Exponential2 {
    type Error = &'static str;

    fn try_from(
        Exponential2Params {
            base,
            top,
            eccentricity,
        }: Exponential2Params,
    ) -> Result<Self, Self::Error> {
        Self::new(base, top, eccentricity).ok_or("Invalid Exponential2 parameters")
    }
}

impl From<Exponential2> for Exponential2Params {
    fn from(value: Exponential2) -> Self {
        value.params
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Div;

    use templar_primitives::dec;

    #[test]
    fn schema_matches_serde_wire_forms() {
        let schema = near_sdk::serde_json::to_value(schemars::schema_for!(InterestRateStrategy))
            .expect("strategy schema serializes");
        let validator = jsonschema::draft7::new(&schema).expect("strategy schema is Draft 7");
        let strategies = [
            (
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ONE)
                    .expect("valid linear strategy"),
                near_sdk::serde_json::json!({"Linear":{"base":"0","top":"1"}}),
            ),
            (
                InterestRateStrategy::piecewise(
                    Decimal::ZERO,
                    dec!("0.5"),
                    dec!("0.125"),
                    dec!("0.5"),
                )
                .expect("valid piecewise strategy"),
                near_sdk::serde_json::json!({"Piecewise":{"base":"0","optimal":"0.5","rate_1":"0.125","rate_2":"0.5"}}),
            ),
            (
                InterestRateStrategy::exponential2(Decimal::ZERO, Decimal::ONE, Decimal::ONE)
                    .expect("valid exponential strategy"),
                near_sdk::serde_json::json!({"Exponential2":{"base":"0","top":"1","eccentricity":"1"}}),
            ),
        ];

        for (strategy, expected_json) in strategies {
            let serialized =
                near_sdk::serde_json::to_value(&strategy).expect("strategy serialization succeeds");
            assert_eq!(serialized, expected_json);
            validator
                .validate(&serialized)
                .expect("serialized strategy is valid under its schema");
            assert_eq!(
                near_sdk::serde_json::from_value::<InterestRateStrategy>(serialized)
                    .expect("strategy deserialization succeeds"),
                strategy
            );
        }

        for internal_json in [
            near_sdk::serde_json::json!({"Piecewise":{"params":{"base":"0","optimal":"0.5","rate_1":"0.125","rate_2":"0.5"},"i_negative_rate_2_b":"0.1875"}}),
            near_sdk::serde_json::json!({"Exponential2":{"params":{"base":"0","top":"1","eccentricity":"1"},"i_factor":"1"}}),
        ] {
            validator
                .validate(&internal_json)
                .expect_err("internal runtime representation is not schema-valid");
            near_sdk::serde_json::from_value::<InterestRateStrategy>(internal_json)
                .expect_err("internal runtime representation is not serde-valid");
        }
    }

    use super::*;

    #[test]
    fn piecewise() {
        let s = Piecewise::new(Decimal::ZERO, dec!("0.9"), dec!("0.035"), dec!("0.6")).unwrap();

        assert!(s.at(Decimal::ZERO).near_equal(Decimal::ZERO));
        assert!(s.at(dec!("0.1")).near_equal(dec!("0.0035")));
        assert!(s.at(dec!("0.5")).near_equal(dec!("0.0175")));
        assert!(s.at(dec!("0.6")).near_equal(dec!("0.021")));
        assert!(s.at(dec!("0.9")).near_equal(dec!("0.0315")));
        assert!(s.at(dec!("0.95")).near_equal(dec!("0.0615")));
        assert!(s.at(Decimal::ONE).near_equal(dec!("0.0915")));
    }

    // Backstop for `fuzz_decimals` (ENG-341): the harness predicts and skips
    // the `base > optimal*(rate_2 - rate_1)` region because `Piecewise::new`
    // computes `optimal*(rate_2 - rate_1) - base` as an unsigned `Decimal` and
    // underflows there — and libfuzzer-sys can't tell that abort from a real
    // crash. The abort, the symptom of the tracked bug, is asserted here so the
    // suppressed region stays pinned. (U512 underflow panics with "arithmetic
    // operation overflow".)
    #[test]
    // Match only the stable "overflow" substring, not the full toolchain- /
    // backend-specific panic text (U512 emits "arithmetic operation overflow").
    #[should_panic(expected = "overflow")]
    fn piecewise_new_underflows_when_base_exceeds_cross_term() {
        // optimal*(rate_2 - rate_1) = 0.9*(0.1 - 0.0) = 0.09; base = 0.5 > 0.09.
        let _ = Piecewise::new(dec!("0.5"), dec!("0.9"), dec!("0.0"), dec!("0.1"));
    }

    #[test]
    fn exponential2() {
        let s = Exponential2::new(dec!("0.005"), dec!("0.08"), dec!("6")).unwrap();
        assert!(s.at(Decimal::ZERO).near_equal(dec!("0.005")));
        assert!(s
            .at(dec!("0.25"))
            .near_equal(dec!("0.00717669895803117868762306839097547161")));
        assert!(s.at(Decimal::ONE_HALF).near_equal(Decimal::ONE.div(75u32)));
    }
}
