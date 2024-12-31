use near_sdk::json_types::U128;
use near_sdk::near;

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
#[near]
pub struct BorrowAssetMetrics {
    pub used: U128,
    /// Available to be borrowed right now.
    pub available: U128,
    pub deposited: U128,
}

impl BorrowAssetMetrics {
    pub fn calculate(deposited: u128, balance: u128, maximum_usage_ratio: Rational<u128>) -> Self {
        assert!(deposited >= balance);

        let used = deposited - balance;

        let available = maximum_usage_ratio
            .checked_scalar_mul(deposited)
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
                self.deposited,
                self.balance,
                self.maximum_usage_ratio,
            );

            assert_eq!(metrics.available.0, self.expected_available);
            assert_eq!(metrics.used.0, self.expected_used);
            assert_eq!(metrics.deposited.0, self.deposited);
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
#[near]
pub struct LiquidationSpread {
    pub supply_position: U128,
    pub liquidator: U128,
    pub protocol: U128,
    // pub insurance: U128,
}

#[near(serializers = [borsh, json])]
pub enum Nep141MarketDepositMessage {
    Supply,
    Collateralize,
    Repay,
    Liquidate,
}
