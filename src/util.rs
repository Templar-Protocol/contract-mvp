use near_sdk::{
    borsh::{self, BorshDeserialize, BorshSerialize},
    json_types::U128,
    serde::{Deserialize, Serialize},
};

#[derive(Serialize, Deserialize, BorshSerialize, BorshDeserialize, Debug, Clone, Copy)]
#[serde(crate = "near_sdk::serde")]
pub struct Fraction {
    pub numerator: U128,
    pub denominator: U128,
}

impl<T: Into<U128>, U: Into<U128>> From<(T, U)> for Fraction {
    fn from((numerator, denominator): (T, U)) -> Self {
        Self {
            numerator: numerator.into(),
            denominator: denominator.into(),
        }
    }
}

impl Fraction {
    pub fn new(numerator: U128, denominator: U128) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    pub fn to_percentage(self) -> u128 {
        self.numerator.0 * 100 / self.denominator.0
    }
}
