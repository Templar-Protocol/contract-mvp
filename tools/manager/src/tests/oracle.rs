use clap::Parser;
use rstest::rstest;
use templar_gateway_core::RedactedString;
use templar_gateway_oracle_updates_dispatch::LazerSourceArgs;
use templar_gateway_types::MethodSpec;

use super::CREDS;
use crate::cli::{Cli, Command};
use crate::commands::{proxy_oracle::parse_price_identifier, OracleNs};
use crate::dispatch::generic::{oracle_route, OracleRoute};

const PRICE_ID: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

/// Every `oracle.*` update must be reachable from generic `write`. `write` selects the
/// context each one needs by hand — one macro callback cannot produce four differently
/// typed contexts — so this expands the canonical list to prove nothing was left behind.
/// Add a method to `for_each_oracle_update_method!` without routing it and this fails.
#[test]
fn every_oracle_update_method_is_routed() {
    macro_rules! assert_routed {
        ($spec:ty) => {
            let method = <$spec as MethodSpec>::RPC_METHOD;
            assert!(
                oracle_route(method).is_some(),
                "{method} is registered by for_each_oracle_update_method! but generic \
                 `write` does not route it",
            );
        };
    }
    templar_gateway_oracle_updates_spec::for_each_oracle_update_method!(assert_routed);
}

#[test]
fn oracle_route_distinguishes_the_four_updates() {
    assert_eq!(oracle_route("oracle.updatePyth"), Some(OracleRoute::Pyth));
    assert_eq!(
        oracle_route("oracle.updateRedStone"),
        Some(OracleRoute::RedStone)
    );
    assert_eq!(oracle_route("oracle.updateLazer"), Some(OracleRoute::Lazer));
    assert_eq!(
        oracle_route("oracle.updatePrices"),
        Some(OracleRoute::Prices)
    );
}

/// A methods-dispatcher write is not an oracle update, so it never builds a source.
#[test]
fn oracle_route_ignores_methods_dispatch_writes() {
    assert_eq!(oracle_route("proxyOracle.updatePrices"), None);
    assert_eq!(oracle_route("redstone.writePrices"), None);
    assert_eq!(oracle_route("market.borrow"), None);
}

#[test]
fn update_red_stone_collects_repeated_feed_ids() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "oracle",
            "update-red-stone",
            "--oracle-id",
            "redstone.testnet",
            "--feed-id",
            "BTC",
            "--feed-id",
            "ETH",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("oracle update-red-stone should parse");

    let Command::Oracle {
        command: OracleNs::RedStone(cmd),
    } = cli.command
    else {
        panic!("expected oracle update-red-stone");
    };
    assert_eq!(
        cmd.sources.redstone_node_path,
        std::path::Path::new("node"),
        "the node path should default"
    );

    let spec = cmd.into_spec();
    assert_eq!(spec.oracle_id.as_str(), "redstone.testnet");
    assert_eq!(spec.feed_ids, vec!["BTC".into(), "ETH".into()]);
}

/// A repeated `--price-id` would otherwise resolve the same dependencies twice.
#[test]
fn update_prices_deduplicates_repeated_price_ids() {
    const OTHER_PRICE_ID: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "oracle",
            "update-prices",
            "--oracle-id",
            "proxy.testnet",
            "--price-id",
            OTHER_PRICE_ID,
            "--price-id",
            PRICE_ID,
            "--price-id",
            OTHER_PRICE_ID,
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("oracle update-prices should parse");

    let Command::Oracle {
        command: OracleNs::Prices(cmd),
    } = cli.command
    else {
        panic!("expected oracle update-prices");
    };

    let spec = cmd.into_spec();
    assert_eq!(
        spec.price_ids,
        vec![
            parse_price_identifier(PRICE_ID).expect("a valid price id"),
            parse_price_identifier(OTHER_PRICE_ID).expect("a valid price id"),
        ],
        "duplicates dropped, and the remainder ordered by identifier"
    );
}

/// A repeated `--price-id` only widens the Hermes query for the same VAA.
#[test]
fn update_pyth_deduplicates_repeated_price_ids() {
    const OTHER_PRICE_ID: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "oracle",
            "update-pyth",
            "--oracle-id",
            "pyth.testnet",
            "--price-id",
            OTHER_PRICE_ID,
            "--price-id",
            PRICE_ID,
            "--price-id",
            OTHER_PRICE_ID,
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("oracle update-pyth should parse");

    let Command::Oracle {
        command: OracleNs::Pyth(cmd),
    } = cli.command
    else {
        panic!("expected oracle update-pyth");
    };
    let spec = cmd.into_spec();
    assert_eq!(spec.oracle_id.as_str(), "pyth.testnet");
    assert_eq!(
        spec.price_ids,
        vec![
            parse_price_identifier(PRICE_ID).expect("a valid price id"),
            parse_price_identifier(OTHER_PRICE_ID).expect("a valid price id"),
        ],
        "duplicates dropped, and the remainder ordered by identifier"
    );
}

/// Empty is a no-op at the gateway, so each update rejects a missing id at parse time.
#[rstest]
#[case::update_pyth(&["update-pyth", "--oracle-id", "pyth.testnet"], "--price-id")]
#[case::update_red_stone(&["update-red-stone", "--oracle-id", "redstone.testnet"], "--feed-id")]
#[case::update_lazer(&["update-lazer", "--oracle-id", "lazer.testnet"], "--feed-id")]
#[case::update_prices(&["update-prices", "--oracle-id", "proxy.testnet"], "--price-id")]
fn oracle_updates_require_an_id(#[case] subcommand: &[&str], #[case] expected_flag: &str) {
    let error = Cli::try_parse_from(
        ["tmplrmgr", "oracle"]
            .into_iter()
            .chain(subcommand.iter().copied())
            .chain(CREDS),
    )
    .expect_err("an oracle update with no ids should fail to parse");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
    assert!(
        error.to_string().contains(expected_flag),
        "the error should name {expected_flag}, so a flag losing `required` cannot pass \
         by way of some other missing argument: {error}"
    );
}

/// Each subcommand flattens only the source args its own method reaches, so the Lazer
/// flags are an unknown argument on the two updates that never build a Lazer source.
/// A parse-succeeds assertion could not catch a regression here: the token is optional
/// at parse time, so flattening it everywhere would still parse.
#[rstest]
#[case::update_pyth(&["update-pyth", "--oracle-id", "pyth.testnet", "--price-id", PRICE_ID])]
#[case::update_red_stone(&["update-red-stone", "--oracle-id", "redstone.testnet", "--feed-id", "BTC"])]
fn pyth_and_redstone_updates_reject_lazer_flags(#[case] subcommand: &[&str]) {
    let error = Cli::try_parse_from(
        ["tmplrmgr", "oracle"]
            .into_iter()
            .chain(subcommand.iter().copied())
            .chain(["--pyth-lazer-api-key", "secret-token"])
            .chain(CREDS),
    )
    .expect_err("a Lazer flag on a source-free update should fail to parse");

    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
}

/// The token is optional at parse time so it burdens no other command, and demanded
/// when the Lazer source is actually built. Built from a literal rather than parsed:
/// clap reads `PYTH_LAZER_API_KEY` from the ambient environment, and clearing it would
/// race every other test that parses a command carrying the flag.
#[test]
fn building_a_lazer_source_without_a_token_names_the_flag() {
    let error = lazer_args(None)
        .build()
        .expect_err("building a Lazer source with no token should fail");

    assert!(
        error.to_string().contains("--pyth-lazer-api-key"),
        "the error should name the flag: {error}"
    );
}

#[test]
fn building_a_lazer_source_with_a_token_succeeds() {
    lazer_args(Some("secret-token".into()))
        .build()
        .expect("a valid Lazer source config");
}

/// `LazerSourceArgs` as clap would parse it, with the API token under test.
fn lazer_args(pyth_lazer_api_key: Option<RedactedString>) -> LazerSourceArgs {
    LazerSourceArgs {
        pyth_lazer_api_key,
        pyth_lazer_ws_url: "wss://example.com/v1/stream"
            .parse()
            .expect("a valid wss url"),
        pyth_lazer_channel: "fixed_rate@200ms".to_owned(),
        pyth_lazer_max_payload_age_ms: 5_000,
    }
}

#[test]
fn update_lazer_collects_repeated_feed_ids() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "oracle",
            "update-lazer",
            "--oracle-id",
            "lazer.testnet",
            "--feed-id",
            "7",
            "--feed-id",
            "8",
            "--pyth-lazer-api-key",
            "secret-token",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("update-lazer should parse");

    let Command::Oracle {
        command: OracleNs::Lazer(cmd),
    } = cli.command
    else {
        panic!("expected oracle update-lazer");
    };

    let spec = cmd.into_spec();
    assert_eq!(spec.oracle_id.as_str(), "lazer.testnet");
    assert_eq!(spec.feed_ids, vec![7, 8]);
}
