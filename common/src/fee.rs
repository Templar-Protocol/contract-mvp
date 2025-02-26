use near_sdk::{json_types::U64, near};

use crate::{
    asset::{AssetClass, FungibleAssetAmount},
    rational::Rational,
};

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub enum Fee<T: AssetClass> {
    Flat(FungibleAssetAmount<T>),
    Proportional(Rational<u16>),
}

impl<T: AssetClass> Fee<T> {
    pub fn zero() -> Self {
        Self::Flat(FungibleAssetAmount::zero())
    }

    pub fn of(&self, amount: FungibleAssetAmount<T>) -> Option<FungibleAssetAmount<T>> {
        match self {
            Fee::Flat(f) => Some(*f),
            Fee::Proportional(rational) => rational
                .upcast::<u128>()
                .checked_scalar_mul(amount.as_u128())?
                .ceil()
                .map(Into::into),
        }
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct TimeBasedFee<T: AssetClass> {
    pub fee: Fee<T>,
    pub duration: U64,
    pub behavior: TimeBasedFeeFunction,
}

impl<T: AssetClass> TimeBasedFee<T> {
    pub fn zero() -> Self {
        Self {
            fee: Fee::Flat(0.into()),
            duration: 0.into(),
            behavior: TimeBasedFeeFunction::Fixed,
        }
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub enum TimeBasedFeeFunction {
    Fixed,
    Linear,
    Logarithmic,
}

impl<T: AssetClass> TimeBasedFee<T> {
    pub fn of(&self, amount: FungibleAssetAmount<T>, time: u64) -> Option<FungibleAssetAmount<T>> {
        let base_fee = self.fee.of(amount)?;

        if self.duration.0 == 0 {
            return Some(0.into());
        }

        match self.behavior {
            TimeBasedFeeFunction::Fixed => Some(base_fee),
            TimeBasedFeeFunction::Linear => Rational::new(time, self.duration.0)
                .upcast::<u128>()
                .checked_scalar_mul(base_fee.as_u128())?
                .ceil()
                .map(Into::into),
            TimeBasedFeeFunction::Logarithmic => Some(
                // TODO: Seems jank.
                #[allow(
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation,
                    clippy::cast_precision_loss
                )]
                (((base_fee.as_u128() as f64 * f64::log2((1 + time - self.duration.0) as f64))
                    / f64::log2((1 + time) as f64))
                .ceil() as u128)
                    .into(),
            ),
        }
    }
}
