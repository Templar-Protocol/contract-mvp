use std::{
    fmt::{Debug, Display},
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign},
    str::FromStr,
};

use near_sdk::{
    borsh::{BorshDeserialize, BorshSchema, BorshSerialize},
    serde::{self, Deserialize, Serialize},
};
use primitive_types::U512;
use schemars::JsonSchema;

pub const FRACTIONAL_BITS: usize = 128;
const MAX_DECIMAL_PRECISION: usize = 38; // = floor(FRACTIONAL_BITS / log2(10))

#[macro_export]
macro_rules! dec {
    ($s:literal) => {
        Decimal::from_str($s).unwrap()
    };
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Decimal {
    repr: U512,
}

impl Default for Decimal {
    fn default() -> Self {
        Self::zero()
    }
}

impl JsonSchema for Decimal {
    fn schema_name() -> String {
        "Decimal".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        let mut schema = gen.subschema_for::<String>().into_object();
        schema.metadata().description = Some("512-bit fixed-precision decimal".to_string());
        schema.string().pattern = Some("^(0|[1-9][0-9]{0,115})(\\.[0-9]{1,38})?$".to_string());
        schema.into()
    }
}

impl BorshSchema for Decimal {
    fn add_definitions_recursively(
        definitions: &mut std::collections::BTreeMap<
            near_sdk::borsh::schema::Declaration,
            near_sdk::borsh::schema::Definition,
        >,
    ) {
        <[u64; 8] as BorshSchema>::add_definitions_recursively(definitions);
    }

    fn declaration() -> near_sdk::borsh::schema::Declaration {
        String::from("Decimal")
    }
}

impl BorshSerialize for Decimal {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        BorshSerialize::serialize(&self.repr.0, writer)
    }
}

impl BorshDeserialize for Decimal {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        Ok(Self {
            repr: U512(BorshDeserialize::deserialize_reader(reader)?),
        })
    }
}

impl Serialize for Decimal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: near_sdk::serde::Serializer,
    {
        serializer.serialize_str(&self.to_fixed(MAX_DECIMAL_PRECISION))
    }
}

impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Decimal::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl Decimal {
    const REPR_ONE: U512 = U512([0, 0, 1, 0, 0, 0, 0, 0]);
    /// When converting to & from strings, we don't guarantee accurate
    /// representation of bits lower than this.
    const REPR_EPSILON: U512 = U512([0b1000, 0, 0, 0, 0, 0, 0, 0]);

    pub const fn zero() -> Self {
        Self { repr: U512::zero() }
    }

    pub const fn half() -> Self {
        Self {
            repr: U512([0, 0x8000_0000_0000_0000, 0, 0, 0, 0, 0, 0]),
        }
    }

    pub const fn one() -> Self {
        Self {
            repr: Self::REPR_ONE,
        }
    }

    pub const fn two() -> Self {
        Self {
            repr: U512([0, 0, 2, 0, 0, 0, 0, 0]),
        }
    }

    pub fn as_repr(&self) -> &[u64] {
        &self.repr.0
    }

    pub fn near_equal(&self, other: &Decimal) -> bool {
        self.abs_diff(other).repr <= Self::REPR_EPSILON
    }

    #[must_use]
    pub fn abs_diff(&self, other: &Decimal) -> Decimal {
        if self > other {
            self - other
        } else {
            other - self
        }
    }

    pub fn to_u128(&self) -> Option<u128> {
        let truncated = self.repr >> FRACTIONAL_BITS;
        if truncated.bits() <= 128 {
            Some(truncated.as_u128())
        } else {
            None
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap
    )]
    pub fn to_f64_lossy(&self) -> f64 {
        let frac = self.repr.low_u128() as f64 / 2f64.powi(FRACTIONAL_BITS as i32);
        let low = (self.repr >> FRACTIONAL_BITS).low_u128() as f64;
        let high = (self.repr >> (FRACTIONAL_BITS * 2)).low_u128() as f64;

        high + low + frac
    }

    pub fn to_fixed(&self, precision: usize) -> String {
        let precision = precision.min(MAX_DECIMAL_PRECISION);
        format!(
            "{}.{}",
            self.repr >> FRACTIONAL_BITS,
            self.fractional_part_to_dec_string(precision),
        )
    }

    fn fractional_part(&self) -> U512 {
        U512::from(self.repr.low_u128())
    }

    fn epsilon_round(repr: U512) -> U512 {
        (repr + (Self::REPR_EPSILON >> 1)) & !(Self::REPR_EPSILON - 1)
    }

    fn fractional_part_to_dec_string(&self, precision: usize) -> String {
        let mut s = Vec::with_capacity(precision);
        let mut f = self.fractional_part();
        let d = Self::REPR_ONE;

        #[allow(clippy::cast_possible_truncation)]
        for _ in 0..precision {
            if f.is_zero() {
                break;
            }

            f *= 10;

            let digit = (f / d).low_u64();
            s.push(digit as u8 + b'0');

            f %= d;
        }

        unsafe { String::from_utf8_unchecked(s) }
    }
}

pub mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    #[error("Failed to parse decimal")]
    pub struct DecimalParseError;
}

impl FromStr for Decimal {
    type Err = error::DecimalParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (whole, frac) = if let Some((whole, frac)) = s.split_once('.') {
            (whole, Some(frac))
        } else {
            (s, None)
        };

        let whole =
            U512::from_dec_str(whole).map_err(|_| error::DecimalParseError)? << FRACTIONAL_BITS;

        if let Some(frac) = frac {
            let mut f = U512::zero();
            let mut div = 10u128;

            for c in frac.chars().take(MAX_DECIMAL_PRECISION) {
                if let Some(d) = c.to_digit(10) {
                    if d != 0 {
                        let d = (U512::from(d) << (FRACTIONAL_BITS * 2)) / div;
                        f += d;
                    }
                    if let Some(next_div) = div.checked_mul(10) {
                        div = next_div;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }

            Ok(Self {
                repr: (whole + Decimal::epsilon_round(f >> FRACTIONAL_BITS)),
            })
        } else {
            Ok(Self { repr: whole })
        }
    }
}

impl Display for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_f64_lossy())
    }
}

impl Debug for Decimal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.fractional_part().is_zero() {
            write!(f, "{}", self.repr >> FRACTIONAL_BITS)
        } else {
            write!(
                f,
                "{}.{}",
                self.repr >> FRACTIONAL_BITS,
                self.fractional_part_to_dec_string(MAX_DECIMAL_PRECISION),
            )
        }
    }
}

macro_rules! impl_self {
    ($s:ty,$t:ty) => {
        impl Add<$t> for $s {
            type Output = Decimal;

            fn add(self, rhs: $t) -> Self::Output {
                Decimal {
                    repr: self.repr.add(rhs.repr),
                }
            }
        }

        impl Sub<$t> for $s {
            type Output = Decimal;

            fn sub(self, rhs: $t) -> Self::Output {
                Decimal {
                    repr: self.repr.sub(rhs.repr),
                }
            }
        }

        impl Mul<$t> for $s {
            type Output = Decimal;

            fn mul(self, rhs: $t) -> Self::Output {
                Decimal {
                    repr: ((self.repr * rhs.repr) >> FRACTIONAL_BITS),
                }
            }
        }

        impl Div<$t> for $s {
            type Output = Decimal;

            fn div(self, rhs: $t) -> Self::Output {
                Decimal {
                    repr: ((self.repr << FRACTIONAL_BITS) / rhs.repr),
                }
            }
        }
    };
}

impl_self!(Decimal, Decimal);
impl_self!(&Decimal, Decimal);
impl_self!(Decimal, &Decimal);
impl_self!(&Decimal, &Decimal);

macro_rules! impl_self_assign {
    ($s:ty,$t:ty) => {
        impl AddAssign<$t> for $s {
            fn add_assign(&mut self, rhs: $t) {
                self.repr += rhs.repr;
            }
        }

        impl SubAssign<$t> for $s {
            fn sub_assign(&mut self, rhs: $t) {
                self.repr -= rhs.repr;
            }
        }

        impl DivAssign<$t> for $s {
            fn div_assign(&mut self, rhs: $t) {
                self.repr = ((self.repr << FRACTIONAL_BITS) / rhs.repr);
            }
        }

        impl MulAssign<$t> for $s {
            fn mul_assign(&mut self, rhs: $t) {
                self.repr = ((self.repr * rhs.repr) >> FRACTIONAL_BITS);
            }
        }
    };
}

impl_self_assign!(Decimal, Decimal);
impl_self_assign!(Decimal, &Decimal);

macro_rules! impl_int {
    ($t:ty) => {
        impl_int!(@from $t);
        impl_int!(@ops $t, Decimal);
        impl_int!(@ops $t, &Decimal);
    };

    (@from $t:ty) => {
        impl From<$t> for Decimal {
            fn from(value: $t) -> Self {
                Self {
                    repr: U512::from(value) << FRACTIONAL_BITS,
                }
            }
        }
    };

    (@ops $t:ty,$s:ty) => {
        impl Mul<$t> for $s {
            type Output = Decimal;

            fn mul(self, rhs: $t) -> Self::Output {
                self * Decimal::from(rhs)
            }
        }

        impl Mul<$s> for $t {
            type Output = Decimal;

            fn mul(self, rhs: $s) -> Self::Output {
                Decimal::from(self) * rhs
            }
        }

        impl Div<$t> for $s {
            type Output = Decimal;

            fn div(self, rhs: $t) -> Self::Output {
                self / Decimal::from(rhs)
            }
        }

        impl Div<$s> for $t {
            type Output = Decimal;

            fn div(self, rhs: $s) -> Self::Output {
                Decimal::from(self) / rhs
            }
        }

        impl Add<$t> for $s {
            type Output = Decimal;

            fn add(self, rhs: $t) -> Self::Output {
                self + Decimal::from(rhs)
            }
        }

        impl Add<$s> for $t {
            type Output = Decimal;

            fn add(self, rhs: $s) -> Self::Output {
                Decimal::from(self) + rhs
            }
        }

        impl Sub<$t> for $s {
            type Output = Decimal;

            fn sub(self, rhs: $t) -> Self::Output {
                self - Decimal::from(rhs)
            }
        }

        impl Sub<$s> for $t {
            type Output = Decimal;

            fn sub(self, rhs: $s) -> Self::Output {
                Decimal::from(self) - rhs
            }
        }

        impl PartialEq<$t> for $s {
            fn eq(&self, other: &$t) -> bool {
                self.repr == Decimal::from(*other).repr
            }
        }

        impl PartialOrd<$t> for $s {
            fn partial_cmp(&self, other: &$t) -> Option<std::cmp::Ordering> {
                self.repr.partial_cmp(&Decimal::from(*other).repr)
            }
        }
    };
}

impl_int!(u8);
impl_int!(u16);
impl_int!(u32);
impl_int!(u64);
impl_int!(u128);

#[cfg(test)]
mod tests {
    use near_sdk::serde_json;
    use rand::Rng;
    use rstest::rstest;

    use super::*;

    // These functions are intentionally implemented using mathematical
    // operations instead of bitwise operations, so as to test the
    // correctness of the mathematical operators.

    fn with_upper_u128(n: u128) -> Decimal {
        let mut d = Decimal::from(n);
        d *= Decimal::from(u128::pow(2, 64));
        d *= Decimal::from(u128::pow(2, 64));
        d
    }

    fn get_upper_u128(mut d: Decimal) -> u128 {
        d /= Decimal::from(u128::pow(2, 64));
        d /= Decimal::from(u128::pow(2, 64));
        d.to_u128().unwrap()
    }

    #[rstest]
    #[case(0, 0)]
    #[case(0, 1)]
    #[case(1, 0)]
    #[case(1, 1)]
    #[case(2_934_570_000_008_u128, 9_595_959_283_u128)]
    #[case(u128::MAX, 0)]
    #[case(0, u128::MAX)]
    #[test]
    fn addition(#[case] a: u128, #[case] b: u128) {
        assert_eq!(Decimal::from(a) + Decimal::from(b), a + b);
        assert_eq!(
            get_upper_u128(with_upper_u128(a) + with_upper_u128(b)),
            a + b,
        );
    }

    #[rstest]
    #[case(0, 0)]
    #[case(1, 0)]
    #[case(1, 1)]
    #[case(2_934_570_000_008_u128, 9_595_959_283_u128)]
    #[case(u128::MAX, 0)]
    #[case(u128::MAX, 1)]
    #[case(u128::MAX, u128::MAX / 2)]
    #[case(u128::MAX, u128::MAX)]
    #[test]
    fn subtraction(#[case] a: u128, #[case] b: u128) {
        assert_eq!(Decimal::from(a) - Decimal::from(b), a - b);
        assert_eq!(
            get_upper_u128(with_upper_u128(a) - with_upper_u128(b)),
            a - b,
        );
    }

    #[rstest]
    #[case(0, 0)]
    #[case(0, 1)]
    #[case(1, 0)]
    #[case(1, 1)]
    #[case(2, 2)]
    #[case(u128::MAX, 0)]
    #[case(u128::MAX, 1)]
    #[case(0, u128::MAX)]
    #[case(1, u128::MAX)]
    #[test]
    fn multiplication(#[case] a: u128, #[case] b: u128) {
        assert_eq!(Decimal::from(a) * Decimal::from(b), a * b);
        assert_eq!(get_upper_u128(with_upper_u128(a) * b), a * b);
        assert_eq!(get_upper_u128(a * with_upper_u128(b)), a * b);
    }

    #[rstest]
    #[case(0, 1)]
    #[case(1, 1)]
    #[case(1, 2)]
    #[case(u128::MAX, u128::MAX)]
    #[case(u128::MAX, 1)]
    #[case(0, u128::MAX)]
    #[case(1, u128::MAX)]
    #[case(1, 10)]
    #[case(3, 10_000)]
    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn division(#[case] a: u128, #[case] b: u128) {
        let quotient = a as f64 / b as f64;
        let abs_difference_lte = |d: Decimal, f: f64| (d.to_f64_lossy() - f).abs() <= 1e-200;
        assert!(abs_difference_lte(
            Decimal::from(a) / Decimal::from(b),
            quotient,
        ));
        assert!(abs_difference_lte(
            with_upper_u128(a) / with_upper_u128(b),
            quotient,
        ));
    }

    #[test]
    fn constants_are_accurate() {
        assert_eq!(Decimal::zero().to_u128().unwrap(), 0);
        assert!((Decimal::half().to_f64_lossy() - 0.5_f64).abs() < 1e-200);
        assert_eq!(Decimal::one().to_u128().unwrap(), 1);
        assert_eq!(Decimal::two().to_u128().unwrap(), 2);
    }

    #[rstest]
    #[case(Decimal::one())]
    #[case(Decimal::two())]
    #[case(Decimal::zero())]
    #[case(Decimal::half())]
    #[case(Decimal::from(u128::MAX))]
    #[case(Decimal::from(u64::MAX) / Decimal::from(u128::MAX))]
    #[test]
    fn serialization(#[case] value: Decimal) {
        let serialized = serde_json::to_string(&value).unwrap();
        let deserialized: Decimal = serde_json::from_str(&serialized).unwrap();

        assert!(value.near_equal(&deserialized));
    }

    #[test]
    fn from_self_string_serialization_precision() {
        const ITERATIONS: usize = 1_024;
        const TRANSFORMATIONS: usize = 32;

        let mut rng = rand::thread_rng();

        let mut max_error = U512::zero();

        for _ in 0..ITERATIONS {
            let actual = Decimal::from(rng.gen::<u128>()) / Decimal::from(rng.gen::<u128>());

            let mut s = actual.to_fixed(MAX_DECIMAL_PRECISION);
            for _ in 0..(TRANSFORMATIONS - 1) {
                s = Decimal::from_str(&s)
                    .unwrap()
                    .to_fixed(MAX_DECIMAL_PRECISION);
            }
            let parsed = Decimal::from_str(&s).unwrap();

            let e = actual.abs_diff(&parsed).repr;

            if e > max_error {
                max_error = e;
            }

            assert!(
                e <= Decimal::REPR_EPSILON,
                "Stringification error of repr {:?} is repr {:?}",
                actual.repr.0,
                e.0,
            );
        }

        println!("Max error: {:?}", max_error.0);
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn from_f64_string_serialization_precision() {
        const ITERATIONS: usize = 10_000;
        let mut rng = rand::thread_rng();
        let epsilon = Decimal {
            repr: Decimal::REPR_EPSILON,
        }
        .to_f64_lossy();

        let t = |f: f64| {
            let actual = f.abs();
            let string = actual.to_string();
            let parsed = Decimal::from_str(&string).unwrap();

            let e = (parsed.to_f64_lossy() - actual).abs();

            assert!(e <= epsilon, "Stringification error of f64 {actual} is {e}");
        };

        for _ in 0..ITERATIONS {
            t(rng.gen::<f64>() * rng.gen::<u128>() as f64);
        }
    }
}
