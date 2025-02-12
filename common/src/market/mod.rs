use std::collections::HashMap;
use std::num::NonZeroU16;

use near_sdk::{env, near, AccountId};

use crate::asset::BorrowAssetAmount;
use crate::rational::Rational;

mod configuration;
pub use configuration::*;
mod external;
pub use external::*;
mod r#impl;
pub use r#impl::*;

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct BorrowAssetMetrics {
    pub available: BorrowAssetAmount,
    pub deposited: BorrowAssetAmount,
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct YieldWeights {
    pub supply: NonZeroU16,
    pub r#static: HashMap<AccountId, u16>,
}

impl YieldWeights {
    pub fn new_with_supply_weight(supply: u16) -> Self {
        Self {
            supply: supply.try_into().unwrap(),
            r#static: HashMap::new(),
        }
    }

    pub fn with_static(mut self, account_id: AccountId, weight: u16) -> Self {
        self.r#static.insert(account_id, weight);
        self
    }

    pub fn total_weight(&self) -> NonZeroU16 {
        self.r#static
            .values()
            .try_fold(self.supply, |a, b| a.checked_add((*b).into()))
            .unwrap_or_else(|| env::panic_str("Total weight overflow"))
    }

    pub fn static_share(&self, account_id: &AccountId) -> Rational<u16> {
        self.r#static
            .get(account_id)
            .map_or_else(Rational::<u16>::zero, |weight| {
                Rational::new((*weight).into(), self.total_weight().into())
            })
    }
}

#[near(serializers = [json])]
pub enum Nep141MarketDepositMessage {
    Supply,
    Collateralize,
    Repay,
    Liquidate(LiquidateMsg),
}

#[near(serializers = [json])]
pub struct LiquidateMsg {
    pub account_id: AccountId,
    pub oracle_price_proof: OraclePriceProof,
}

/// This represents some sort of proof-of-price from a price oracle, e.g. Pyth.
/// In production, it must be validated, but for now it's just trust me bro.
#[derive(Clone, Copy, Debug)]
#[near(serializers = [json])]
pub struct OraclePriceProof {
    pub collateral_asset_price: Rational<u128>,
    pub borrow_asset_price: Rational<u128>,
}
