#![allow(clippy::unwrap_used)]
//! Used by GitHub Actions to generate default market configuration.

use near_sdk::serde_json;
use templar_common::{
    asset::{FungibleAsset, FungibleAssetAmount},
    fee::{Fee, TimeBasedFee},
    market::{MarketConfiguration, YieldWeights},
    rational::{Fraction, Rational},
};

pub fn main() {
    println!(
        "{{\"configuration\":{}}}",
        serde_json::to_string(&MarketConfiguration {
            borrow_asset: FungibleAsset::nep141("usdt.fakes.testnet".parse().unwrap()),
            collateral_asset: FungibleAsset::nep141("wrap.testnet".parse().unwrap()),
            balance_oracle_account_id: "balance_oracle".parse().unwrap(),
            minimum_collateral_ratio_per_borrow: Rational::new(120, 100),
            maximum_borrow_asset_usage_ratio: Fraction::new(99, 100).unwrap(),
            borrow_origination_fee: Fee::zero(),
            borrow_annual_maintenance_fee: Fee::zero(),
            maximum_borrow_duration_ms: None,
            minimum_borrow_amount: FungibleAssetAmount::new(1),
            maximum_borrow_amount: FungibleAssetAmount::new(u128::MAX),
            supply_withdrawal_fee: TimeBasedFee::zero(),
            yield_weights: YieldWeights::new_with_supply_weight(1),
            maximum_liquidator_spread: Fraction::new(5, 100).unwrap(),
        })
        .unwrap(),
    );
}
