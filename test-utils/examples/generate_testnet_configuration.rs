//! Used by GitHub Actions to generate default market configuration.

use near_sdk::serde_json;
use templar_common::market::YieldWeights;
use test_utils::market_configuration;

pub fn main() {
    println!(
        "{{\"configuration\":{}}}",
        serde_json::to_string(&market_configuration(
            "usdt.fakes.testnet".parse().unwrap(),
            "wrap.testnet".parse().unwrap(),
            YieldWeights::new_with_supply_weight(1),
        ))
        .unwrap(),
    );
}
