use std::collections::HashMap;
use std::num::NonZeroU16;

use near_sdk::{near, AccountId};
use templar_primitives::number::Decimal;

use crate::asset::{BorrowAssetAmount, CollateralAssetAmount};
mod configuration;
pub use configuration::{AmountRange, MarketConfiguration, ValidAmountRange, APY_LIMIT};
mod external;
pub use external::*;
mod r#impl;
pub use r#impl::*;
mod price_oracle_configuration;
pub use price_oracle_configuration::PriceOracleConfiguration;

pub mod error {
    pub use super::configuration::error::*;
    pub use super::price_oracle_configuration::error::*;
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh, json])]
pub struct BorrowAssetMetrics {
    pub available: BorrowAssetAmount,
    pub deposited_active: BorrowAssetAmount,
    pub deposited_incoming: HashMap<u32, BorrowAssetAmount>,
    pub borrowed: BorrowAssetAmount,
    /// Repaid interest and fees available to pay supplier yield. Caps the
    /// yield leg of every withdrawal and is shared market-wide,
    /// first-come-first-served.
    #[serde(default)]
    pub paid_to_fees: BorrowAssetAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct YieldWeights {
    pub supply: NonZeroU16,
    pub r#static: HashMap<AccountId, u16>,
}

impl YieldWeights {
    /// # Panics
    /// - If `supply` is zero.
    #[allow(clippy::unwrap_used, reason = "Only used during initial construction")]
    pub fn new_with_supply_weight(supply: u16) -> Self {
        Self {
            supply: supply.try_into().unwrap(),
            r#static: HashMap::new(),
        }
    }

    #[must_use]
    pub fn with_static(mut self, account_id: AccountId, weight: u16) -> Self {
        self.r#static.insert(account_id, weight);
        self
    }

    pub fn total_weight(&self) -> NonZeroU16 {
        self.r#static
            .values()
            .try_fold(self.supply, |a, b| a.checked_add(*b))
            .unwrap_or_else(|| crate::panic_with_message("Total weight overflow"))
    }

    pub fn static_share(&self, account_id: &AccountId) -> Decimal {
        self.r#static
            .get(account_id)
            .map_or(Decimal::ZERO, |weight| {
                Decimal::from(*weight) / u16::from(self.total_weight())
            })
    }
}

/// Parsed from the string parameter `msg` passed by `*_transfer_call` to
/// `*_on_transfer` calls.
#[derive(Debug)]
#[near(serializers = [json])]
pub enum DepositMsg {
    /// Add the attached tokens to the sender's supply position's deposit.
    Supply,
    /// Add the attached tokens to the sender's borrow position's collateral
    /// deposit.
    Collateralize,
    /// Use the attached tokens to pay down the sender's borrow position's
    /// liability (sans fees).
    Repay,
    /// Use the attached tokens to pay down a specified borrow position's
    /// liability (sans fees).
    RepayAccount(RepayAccountMsg),
    /// Liquidate an account that is below the configured liquidation
    /// collateralization ratio threshold.
    Liquidate(LiquidateMsg),
}

impl DepositMsg {
    pub fn expects_borrow_asset(&self) -> bool {
        match self {
            Self::Supply | Self::Repay | Self::RepayAccount(..) | Self::Liquidate(..) => true,
            Self::Collateralize => false,
        }
    }
}

/// Indicate an account to repay.
#[derive(Debug)]
#[near(serializers = [json])]
pub struct RepayAccountMsg {
    pub account_id: AccountId,
}

/// Indicate an account to liquidate.
#[derive(Debug)]
#[near(serializers = [json])]
pub struct LiquidateMsg {
    pub account_id: AccountId,
    /// How much collateral to liquidate?
    /// Attempts to liquidate the whole position if `None`.
    pub amount: Option<CollateralAssetAmount>,
}

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct Withdrawal {
    pub account_id: AccountId,
    pub amount_to_account: BorrowAssetAmount,
    pub amount_to_fees: BorrowAssetAmount,
}

#[cfg(test)]
mod tests {
    use near_sdk::{
        json_types::U128,
        serde_json::{self, json, Value},
    };

    use super::*;

    /// Parse the wire `msg` shape, assert re-serializing reproduces it exactly,
    /// and return the parsed message. This is the regression guard for the
    /// `msg` strings passed to `ft_transfer_call`/`mt_transfer_call`: the
    /// contract deserializes `msg` into [`DepositMsg`] through this same path.
    fn roundtrip(wire: &Value) -> DepositMsg {
        let parsed: DepositMsg = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(&serde_json::to_value(&parsed).unwrap(), wire);
        parsed
    }

    fn borrow_asset_metrics_json(paid_to_fees: Option<Value>) -> Value {
        let mut metrics = json!({
            "available": "90",
            "deposited_active": "100",
            "deposited_incoming": { "7": "10" },
            "borrowed": "10",
        });
        if let Some(paid_to_fees) = paid_to_fees {
            metrics["paid_to_fees"] = paid_to_fees;
        }
        metrics
    }

    #[test]
    fn deposit_msg_supply() {
        let msg = roundtrip(&json!("Supply"));
        assert!(matches!(msg, DepositMsg::Supply));
        assert!(msg.expects_borrow_asset());
    }

    #[test]
    fn deposit_msg_collateralize() {
        let msg = roundtrip(&json!("Collateralize"));
        assert!(matches!(msg, DepositMsg::Collateralize));
        assert!(!msg.expects_borrow_asset());
    }

    #[test]
    fn deposit_msg_repay() {
        let msg = roundtrip(&json!("Repay"));
        assert!(matches!(msg, DepositMsg::Repay));
        assert!(msg.expects_borrow_asset());
    }

    #[test]
    fn deposit_msg_repay_account() {
        let msg = roundtrip(&json!({ "RepayAccount": { "account_id": "borrow_user.near" } }));
        let DepositMsg::RepayAccount(RepayAccountMsg { account_id }) = &msg else {
            panic!("expected RepayAccount, got {msg:?}");
        };
        assert_eq!(account_id.as_str(), "borrow_user.near");
        assert!(msg.expects_borrow_asset());
    }

    #[test]
    fn deposit_msg_liquidate() {
        let msg = roundtrip(&json!({
            "Liquidate": { "account_id": "borrow_user.near", "amount": U128(1_000_000) },
        }));
        let DepositMsg::Liquidate(LiquidateMsg { account_id, amount }) = &msg else {
            panic!("expected Liquidate, got {msg:?}");
        };
        assert_eq!(account_id.as_str(), "borrow_user.near");
        assert_eq!(*amount, Some(CollateralAssetAmount::new(1_000_000)));
        assert!(msg.expects_borrow_asset());
    }

    #[test]
    fn deposit_msg_liquidate_whole_position() {
        // Omitting `amount` liquidates the whole position. (A `None` amount
        // re-serializes as `"amount": null`, so this case is parse-only.)
        let msg: DepositMsg =
            serde_json::from_value(json!({ "Liquidate": { "account_id": "borrow_user.near" } }))
                .unwrap();
        let DepositMsg::Liquidate(LiquidateMsg { amount, .. }) = &msg else {
            panic!("expected Liquidate, got {msg:?}");
        };
        assert_eq!(*amount, None);
    }

    #[test]
    fn borrow_asset_metrics_legacy_json() {
        let metrics: BorrowAssetMetrics =
            serde_json::from_value(borrow_asset_metrics_json(None)).unwrap();

        assert_eq!(u128::from(metrics.available), 90);
        assert_eq!(u128::from(metrics.deposited_active), 100);
        assert_eq!(u128::from(metrics.deposited_incoming[&7]), 10);
        assert_eq!(u128::from(metrics.borrowed), 10);
        assert!(metrics.paid_to_fees.is_zero());
    }

    #[test]
    fn borrow_asset_metrics_paid_to_fees_json() {
        for expected_paid_to_fees in [0, u128::MAX] {
            let paid_to_fees = expected_paid_to_fees.to_string();
            let metrics: BorrowAssetMetrics =
                serde_json::from_value(borrow_asset_metrics_json(Some(json!(paid_to_fees))))
                    .unwrap();

            assert_eq!(u128::from(metrics.paid_to_fees), expected_paid_to_fees);
            assert_eq!(
                serde_json::to_value(&metrics).unwrap()["paid_to_fees"],
                paid_to_fees,
            );
        }
    }

    #[test]
    fn borrow_asset_metrics_rejects_malformed_paid_to_fees() {
        for paid_to_fees in [Value::Null, json!("not-an-amount")] {
            assert!(
                serde_json::from_value::<BorrowAssetMetrics>(borrow_asset_metrics_json(Some(
                    paid_to_fees
                ),))
                .is_err()
            );
        }
    }
}
