#![allow(clippy::unwrap_used)]
//! Used by GitHub Actions to generate default market configuration.

use std::str::FromStr;

use near_sdk::serde_json;
use templar_common::{
    asset::{FungibleAsset, FungibleAssetAmount},
    fee::{Fee, TimeBasedFee},
    market::{MarketConfiguration, YieldWeights},
    number::Decimal,
};

pub fn main() {
    println!(
        "{{\"configuration\":{}}}",
        serde_json::to_string(&MarketConfiguration {
            borrow_asset: FungibleAsset::nep141("usdt.fakes.testnet".parse().unwrap()),
            collateral_asset: FungibleAsset::nep141("wrap.testnet".parse().unwrap()),
            balance_oracle_account_id: "balance_oracle".parse().unwrap(),
            minimum_collateral_ratio_per_borrow: Decimal::from_str("1.2").unwrap(),
            maximum_borrow_asset_usage_ratio: Decimal::from_str("0.99").unwrap(),
            borrow_origination_fee: Fee::zero(),
            borrow_annual_maintenance_fee: Fee::zero(),
            maximum_borrow_duration_ms: None,
            minimum_borrow_amount: FungibleAssetAmount::new(1),
            maximum_borrow_amount: FungibleAssetAmount::new(u128::MAX),
            supply_withdrawal_fee: TimeBasedFee::zero(),
            yield_weights: YieldWeights::new_with_supply_weight(1),
            maximum_liquidator_spread: Decimal::from_str("0.05").unwrap(),
        })
        .unwrap(),
    );
}
