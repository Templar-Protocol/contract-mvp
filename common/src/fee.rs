use near_sdk::{
    json_types::{U128, U64},
    near,
};

use crate::rational::Rational;

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub enum Fee {
    Flat(U128),
    Proportional(Rational<u16>),
}

impl Fee {
    pub fn of(&self, amount: u128) -> Option<u128> {
        match self {
            Fee::Flat(f) => Some(f.0),
            Fee::Proportional(rational) => {
                rational.upcast::<u128>().checked_scalar_mul(amount)?.ceil()
            }
        }
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct TimeBasedFee {
    pub fee: Fee,
    pub duration: U64,
    pub behavior: TimeBasedFeeFunction,
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub enum TimeBasedFeeFunction {
    Fixed,
    Linear,
    Logarithmic,
}

impl TimeBasedFee {
    pub fn of(&self, amount: u128, time: u64) -> Option<u128> {
        let base_fee = self.fee.of(amount)?;

        if self.duration.0 == 0 {
            return Some(0);
        }

        match self.behavior {
            TimeBasedFeeFunction::Fixed => Some(base_fee),
            TimeBasedFeeFunction::Linear => Some(
                base_fee
                    .checked_mul(u128::from(time))?
                    .div_ceil(u128::from(self.duration.0)),
            ),
            TimeBasedFeeFunction::Logarithmic => Some(
                ((base_fee as f64 * f64::log2((1 + time - self.duration.0) as f64))
                    / f64::log2((1 + time) as f64))
                .ceil() as u128,
            ),
        }
    }
}
