use clap::Parser;
use serde_json::json;

use super::{authorized, CREDS};
use crate::cli::{Cli, Command};
use crate::commands::market::MarketNs;

#[test]
fn parses_market_create_typed_args() {
    let init_args = br#"{"configuration":{"time_chunk_configuration":{"duration_ms":"600000"},"borrow_asset":{"Nep141":"usdt.testnet"},"collateral_asset":{"Nep141":"collateral.testnet"},"price_oracle_configuration":{"account_id":"oracle.testnet","collateral_asset_price_id":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","collateral_asset_decimals":24,"borrow_asset_price_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","borrow_asset_decimals":6,"price_maximum_age_s":60},"borrow_mcr_maintenance":"1.4","borrow_mcr_liquidation":"1.33333333333333333333333333333333333333","borrow_asset_maximum_usage_ratio":"0.99","borrow_origination_fee":{"Flat":"0"},"borrow_interest_rate_strategy":{"Piecewise":{"base":"0","optimal":"0.9","rate_1":"0.08888888888888888888888888888888888889","rate_2":"2.4"}},"borrow_maximum_duration_ms":null,"borrow_range":{"minimum":"1","maximum":null},"supply_range":{"minimum":"40000","maximum":null},"supply_withdrawal_range":{"minimum":"40000","maximum":null},"supply_withdrawal_fee":{"fee":{"Flat":"0"},"duration":"0","behavior":"Fixed"},"yield_weights":{"supply":4,"static":{"revenue.testnet":1}},"protocol_account_id":"revenue.testnet","liquidation_maximum_spread":"0.1"}}"#;
    let path = write_init_args_fixture("valid", init_args);

    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "market",
            "create",
            "--registry-id",
            "registry.testnet",
            "--name",
            "market",
            "--version-key",
            "market@1",
            "--init-args-file",
            path.to_str().expect("fixture path is unicode"),
            "--deposit",
            "5.5 NEAR",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("market create should parse");

    let params = match cli.command {
        Command::Market {
            command: MarketNs::Create(cmd),
        } => {
            let authorization = authorized(&cmd.signer);
            cmd.try_into_spec(&authorization)
                .expect("market create should parse init args")
        }
        _ => panic!("expected Market::Create"),
    };

    std::fs::remove_file(&path).expect("remove init args fixture");

    let params_json = serde_json::to_value(&params).unwrap();
    assert_eq!(params_json["registry_id"], json!("registry.testnet"));
    assert_eq!(params_json["name"], json!("market"));
    assert_eq!(params_json["version_key"], json!("market@1"));
    assert_eq!(params_json["deposit"], json!("5500000000000000000000000"));
    assert_eq!(
        params_json["configuration"]["borrow_asset"],
        json!({ "Nep141": "usdt.testnet" })
    );
    serde_json::from_value::<templar_gateway_methods_spec::market::Create>(params_json)
        .expect("typed market.create params should match the gateway spec");
}

#[test]
fn market_create_rejects_missing_init_args_file() {
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-missing-market-init-{}-{}.json",
        std::process::id(),
        line!(),
    ));

    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "market",
            "create",
            "--registry-id",
            "registry.testnet",
            "--name",
            "market",
            "--version-key",
            "market@1",
            "--init-args-file",
            path.to_str().expect("fixture path is unicode"),
            "--deposit",
            "5.5 NEAR",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("market create should parse before file loading");

    let error = match cli.command {
        Command::Market {
            command: MarketNs::Create(cmd),
        } => {
            let authorization = authorized(&cmd.signer);
            cmd.try_into_spec(&authorization)
                .expect_err("missing init args file should be rejected")
        }
        _ => panic!("expected Market::Create"),
    };

    assert!(error.to_string().contains("open market init args"));
}

#[test]
fn market_create_rejects_invalid_init_args() {
    let path = write_init_args_fixture("invalid", br#"{"configuration": null}"#);

    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "market",
            "create",
            "--registry-id",
            "registry.testnet",
            "--name",
            "market",
            "--version-key",
            "market@1",
            "--init-args-file",
            path.to_str().expect("fixture path is unicode"),
            "--deposit",
            "5.5 NEAR",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("market create should parse before init args validation");

    let error = match cli.command {
        Command::Market {
            command: MarketNs::Create(cmd),
        } => {
            let authorization = authorized(&cmd.signer);
            cmd.try_into_spec(&authorization)
                .expect_err("invalid init args should be rejected")
        }
        _ => panic!("expected Market::Create"),
    };

    std::fs::remove_file(&path).expect("remove init args fixture");

    assert!(error.to_string().contains("parse market init args"));
}

fn write_init_args_fixture(label: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-market-init-{}-{}.json",
        std::process::id(),
        label,
    ));
    std::fs::write(&path, bytes).expect("write market init args fixture");
    path
}

#[test]
fn market_remove_parses_beneficiary_and_force() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "market",
            "remove",
            "--beneficiary-id",
            "treasury.testnet",
            "--force",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("market remove should parse");
    let Command::Market {
        command: MarketNs::Remove(cmd),
    } = cli.command
    else {
        panic!("expected Market::Remove");
    };
    assert_eq!(cmd.beneficiary_id().as_str(), "treasury.testnet");
    assert!(cmd.force());
}
