use std::collections::HashMap;
use std::num::NonZeroU16;

use near_sdk::{env, near, require, AccountId};

use crate::asset::BorrowAssetAmount;
use crate::rational::Rational;

mod configuration;
pub use configuration::*;
mod external;
pub use external::*;
mod r#impl;
pub use r#impl::*;

/// Borrow asset metrics are related as follows:
///
/// ```
/// available = floor(deposited * maximum_borrow_asset_usage_ratio) - used
/// used = deposited - balance
/// ```
#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct BorrowAssetMetrics {
    pub used: BorrowAssetAmount,
    /// Available to be borrowed right now.
    pub available: BorrowAssetAmount,
    pub deposited: BorrowAssetAmount,
}

impl BorrowAssetMetrics {
    pub fn calculate(
        deposited: BorrowAssetAmount,
        balance: BorrowAssetAmount,
        maximum_usage_ratio: Rational<u128>,
    ) -> Self {
        require!(deposited >= balance);

        let used = deposited.as_u128() - balance.as_u128();

        let available = maximum_usage_ratio
            .checked_scalar_mul(deposited.as_u128())
            .and_then(|x| x.floor())
            .and_then(|x| x.checked_sub(used))
            .unwrap_or(0);

        Self {
            available: available.into(),
            deposited: deposited.into(),
            used: used.into(),
        }
    }
}

#[test]
fn test_available_formula() {
    struct Test {
        maximum_usage_ratio: Rational<u128>,
        deposited: u128,
        balance: u128,
        expected_available: u128,
        expected_used: u128,
    }

    impl Test {
        fn run(&self) {
            let metrics = BorrowAssetMetrics::calculate(
                self.deposited.into(),
                self.balance.into(),
                self.maximum_usage_ratio,
            );

            assert_eq!(metrics.available.as_u128(), self.expected_available);
            assert_eq!(metrics.used.as_u128(), self.expected_used);
            assert_eq!(metrics.deposited.as_u128(), self.deposited);
        }
    }

    let tests = [
        Test {
            maximum_usage_ratio: Rational::new(90, 100),
            deposited: 10000,
            balance: 5000,
            expected_available: 4000,
            expected_used: 5000,
        },
        Test {
            maximum_usage_ratio: Rational::new(0, 100),
            deposited: 10000,
            balance: 5000,
            expected_available: 0,
            expected_used: 5000,
        },
        Test {
            maximum_usage_ratio: Rational::new(100, 100),
            deposited: 10000,
            balance: 5000,
            expected_available: 5000,
            expected_used: 5000,
        },
        Test {
            maximum_usage_ratio: Rational::new(100, 100),
            deposited: 10000,
            balance: 0,
            expected_available: 0,
            expected_used: 10000,
        },
        Test {
            maximum_usage_ratio: Rational::new(100, 100),
            deposited: 0,
            balance: 0,
            expected_available: 0,
            expected_used: 0,
        },
    ];

    for test in tests {
        test.run();
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct YieldWeights {
    pub supply: NonZeroU16,
    pub r#static: HashMap<AccountId, NonZeroU16>,
}

impl YieldWeights {
    pub fn new_with_supply_weight(supply: u16) -> Self {
        Self {
            supply: supply.try_into().unwrap(),
            r#static: HashMap::new(),
        }
    }

    pub fn with_static(mut self, account_id: AccountId, weight: u16) -> Self {
        self.r#static.insert(account_id, weight.try_into().unwrap());
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
