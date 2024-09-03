use std::{
    fmt::Display,
    ops::{Add, AddAssign, Deref, Div, DivAssign, Mul, MulAssign, Rem, RemAssign, Sub, SubAssign},
};

use near_sdk::{
    borsh::{self, BorshDeserialize, BorshSerialize},
    serde::{Deserialize, Serialize},
    AccountId,
};

use crate::{big_decimal::BigDecimal, external::Price};

#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Clone, Debug)]
#[serde(crate = "near_sdk::serde")]
pub struct ContractAsset {
    pub contract_id: AccountId,
    pub oracle_asset_id: String,
    pub last_price: Option<Price>,
}

impl ContractAsset {
    pub fn new(contract_id: AccountId, oracle_asset_id: String) -> Self {
        Self {
            contract_id,
            oracle_asset_id,
            last_price: None,
        }
    }
}

/// Always representative of the native token (NEAR)
#[derive(BorshDeserialize, BorshSerialize, Deserialize, Serialize, Clone, Debug)]
#[serde(crate = "near_sdk::serde")]
pub struct NativeAsset {
    pub oracle_asset_id: String,
    pub last_price: Option<Price>,
}

impl NativeAsset {
    pub fn new(oracle_asset_id: String) -> Self {
        Self {
            oracle_asset_id,
            last_price: None,
        }
    }
}

macro_rules! asset_newtype {
    ($name: ident, $inner: ty) => {
        #[derive(
            BorshSerialize,
            BorshDeserialize,
            Serialize,
            Deserialize,
            Clone,
            Copy,
            Debug,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
        )]
        #[serde(crate = "near_sdk::serde")]
        pub struct $name(pub $inner);

        impl From<$inner> for $name {
            fn from(inner: $inner) -> Self {
                Self(inner)
            }
        }

        impl Deref for $name {
            type Target = $inner;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl Add<Self> for $name {
            type Output = Self;

            fn add(self, rhs: Self) -> Self::Output {
                Self(self.0 + rhs.0)
            }
        }

        impl AddAssign<Self> for $name {
            fn add_assign(&mut self, rhs: Self) {
                self.0 += rhs.0
            }
        }

        impl Sub<Self> for $name {
            type Output = Self;

            fn sub(self, rhs: Self) -> Self::Output {
                Self(self.0 - rhs.0)
            }
        }

        impl SubAssign<Self> for $name {
            fn sub_assign(&mut self, rhs: Self) {
                self.0 -= rhs.0
            }
        }

        impl Mul<$inner> for $name {
            type Output = Self;

            fn mul(self, rhs: $inner) -> Self::Output {
                Self(self.0 * rhs)
            }
        }

        impl Mul<$name> for $inner {
            type Output = $name;

            fn mul(self, rhs: $name) -> Self::Output {
                $name(self * rhs.0)
            }
        }

        impl MulAssign<$inner> for $name {
            fn mul_assign(&mut self, rhs: $inner) {
                self.0 *= rhs
            }
        }

        impl Div<$inner> for $name {
            type Output = Self;

            fn div(self, rhs: $inner) -> Self::Output {
                Self(self.0 / rhs)
            }
        }

        impl Div<$name> for $name {
            type Output = $inner;

            fn div(self, rhs: $name) -> Self::Output {
                self.0 / rhs.0
            }
        }

        impl DivAssign<$inner> for $name {
            fn div_assign(&mut self, rhs: $inner) {
                self.0 /= rhs
            }
        }

        impl Rem<$inner> for $name {
            type Output = Self;

            fn rem(self, rhs: $inner) -> Self::Output {
                Self(self.0 % rhs)
            }
        }

        impl RemAssign<$inner> for $name {
            fn rem_assign(&mut self, rhs: $inner) {
                self.0 %= rhs
            }
        }
    };
}

asset_newtype!(CollateralAssetBalance, u128);
asset_newtype!(LoanAssetBalance, u128);
asset_newtype!(OracleCanonicalValuation, u128);

pub fn valuation(amount: u128, price: &Price) -> OracleCanonicalValuation {
    BigDecimal::round_u128(&BigDecimal::from_balance_price(amount, price, 0)).into()
}
