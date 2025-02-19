//! Used by GitHub Actions to generate default market configuration.

use near_sdk::{serde_json, AccountId};
use templar_common::market::YieldWeights;
use test_utils::market_configuration;

pub fn main() {
    let master_account: AccountId = std::env::var("NEAR_CONTRACT_STAGING_ACCOUNT_ID")
        .unwrap()
        .parse()
        .unwrap();

    println!(
        "{{\"configuration\":{}}}",
        serde_json::to_string(&market_configuration(
            "usdt.fakes.testnet".parse().unwrap(),
            "wrap.testnet".parse().unwrap(),
            master_account.clone(),
            YieldWeights::new_with_supply_weight(9).with_static(master_account, 1)
        ))
        .unwrap(),
    );
}
