use std::{
    cmp::Ordering,
    ops::{BitXor, Div, Sub},
};

use near_sdk::near;

/// NOTE: It may be prudent to have two different "ratio" types: one for any
/// positive rational, and one restricted to values [0, 1].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct Rational<T>(T, T);

#[allow(clippy::eq_op)]
fn is_zero<T: BitXor<Output = T> + Copy + Eq>(x: T) -> bool {
    x ^ x == x
}

fn gcd_euclid<T: Ord + Sub<Output = T> + Copy>(mut a: T, mut b: T) -> T {
    loop {
        match a.cmp(&b) {
            Ordering::Equal => return a,
            Ordering::Less => b = b - a,
            Ordering::Greater => a = a - b,
        }
    }
}

impl<T: Div<Output = T> + BitXor<Output = T> + Sub<Output = T> + Copy + Eq + Ord> Rational<T> {
    pub fn new(a: T, b: T) -> Self {
        Self(a, b).simplify()
    }

    pub fn simplify(self) -> Self {
        let Self(mut n, mut d) = self;

        if !is_zero(n) && !is_zero(d) {
            let gcd = gcd_euclid(n, d);
            n = n / gcd;
            d = d / gcd;
        }

        Self(n, d)
    }

    pub fn reciprocal(self) -> Self {
        Self(self.1, self.0)
    }

    pub fn upcast<U: From<T>>(self) -> Rational<U> {
        Rational(U::from(self.0), U::from(self.1))
    }

    pub fn numerator(self) -> T {
        self.0
    }

    pub fn denominator(self) -> T {
        self.1
    }
}

macro_rules! impl_rational {
    ($t:ty) => {
        impl Rational<$t> {
            pub fn floor(self) -> Option<$t> {
                let Self(n, d) = self;

                if d == 0 {
                    return None;
                }

                Some(n / d)
            }

            pub fn ceil(self) -> Option<$t> {
                let Self(n, d) = self;

                if d == 0 {
                    return None;
                }

                Some(n.div_ceil(d))
            }

            pub fn is_zero(&self) -> bool {
                self.0 == 0
            }

            pub fn one() -> Self {
                Self(1, 1)
            }

            pub fn zero() -> Self {
                Self(0, 1)
            }

            pub fn checked_add(self, other: Self) -> Option<Self> {
                if is_zero(self.1) || is_zero(other.1) {
                    return None;
                }

                let gcd = gcd_euclid(self.1, other.1);
                let d = self.1.checked_mul(other.1.checked_div(gcd)?)?;
                let na = self.0.checked_mul(d.checked_div(self.1)?)?;
                let nb = other.0.checked_mul(d.checked_div(other.1)?)?;
                Some(Self(na.checked_add(nb)?, d))
            }

            pub fn checked_mul(self, other: Self) -> Option<Self> {
                let n = self.0.checked_mul(other.0)?;
                let d = self.1.checked_mul(other.1)?;
                Some(Self(n, d))
            }

            pub fn checked_div(self, other: Self) -> Option<Self> {
                self.checked_mul(other.reciprocal())
            }

            pub fn checked_scalar_mul(self, other: $t) -> Option<Self> {
                Some(Self(self.0.checked_mul(other)?, self.1))
            }

            pub fn checked_scalar_div(self, other: $t) -> Option<Self> {
                Some(Self(self.0, self.1.checked_mul(other)?))
            }
        }
    };
}

impl_rational!(u8);
impl_rational!(u16);
impl_rational!(u32);
impl_rational!(u64);
impl_rational!(u128);

#[test]
fn test_gcd() {
    assert_eq!(gcd_euclid(1, 1), 1);
    assert_eq!(gcd_euclid(5, 15), 5);
    assert_eq!(gcd_euclid(27, 6), 3);
    assert_eq!(gcd_euclid(200, 17), 1);
}
