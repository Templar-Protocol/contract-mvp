use clap::Parser;
use templar_gateway_methods_spec::redstone as spec;

use super::{authorized, CREDS};
use crate::cli::{Cli, Command};
use crate::commands::RedstoneNs;

const VERSION_WITH_ADMIN: &str = "templar-redstone-adapter-contract@0.2.0#abc";

#[test]
fn write_prices_decodes_base64_payload() {
    // "hello" encoded as standard base64.
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "redstone",
            "write-prices",
            "--oracle-id",
            "redstone.testnet",
            "--feed-id",
            "BTC",
            "--feed-id",
            "ETH",
            "--payload-base64",
            "aGVsbG8=",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("write-prices should parse");
    let params = match cli.command {
        Command::Redstone {
            command: RedstoneNs::WritePrices(a),
        } => a.try_into_spec().expect("write-prices should decode"),
        _ => panic!("expected redstone write-prices"),
    };

    assert_eq!(params.payload.0, b"hello");
    assert_eq!(params.feed_ids.len(), 2);
    assert_eq!(&*params.feed_ids[0], "BTC");

    let json = serde_json::to_value(&params).unwrap();
    serde_json::from_value::<templar_gateway_methods_spec::redstone::WritePrices>(json)
        .expect("write-prices params should match the gateway spec");
}

#[test]
fn write_prices_rejects_invalid_base64() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "redstone",
            "write-prices",
            "--oracle-id",
            "redstone.testnet",
            "--feed-id",
            "BTC",
            "--payload-base64",
            "not valid base64!!!",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("write-prices should parse at the clap boundary");
    let error = match cli.command {
        Command::Redstone {
            command: RedstoneNs::WritePrices(a),
        } => a
            .try_into_spec()
            .expect_err("invalid base64 should be rejected"),
        _ => panic!("expected redstone write-prices"),
    };
    assert!(error.to_string().contains("base64"));
}

#[test]
fn list_role_maps_role_arg_to_snake_case() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "redstone",
        "list-role",
        "--oracle-id",
        "redstone.testnet",
        "--role",
        "modify-roles",
    ])
    .expect("list-role should parse");
    let params = match cli.command {
        Command::Redstone {
            command: RedstoneNs::ListRole(a),
        } => a.into_spec(),
        _ => panic!("expected redstone list-role"),
    };

    let json = serde_json::to_value(&params).unwrap();
    assert_eq!(json["role"], "modify_roles");
    serde_json::from_value::<templar_gateway_methods_spec::redstone::ListRole>(json)
        .expect("list-role params should match the gateway spec");
}

/// Parse a `redstone create` invocation carrying the invariant flags plus `extra`.
fn parse(version: &str, extra: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(
        [
            "tmplrmgr",
            "redstone",
            "create",
            "--registry-id",
            "registry.testnet",
            "--name",
            "redstone",
            "--version-key",
            version,
            "--deposit",
            "3.5 NEAR",
        ]
        .into_iter()
        .chain(extra.iter().copied())
        .chain(CREDS),
    )
}

/// Parse a valid `redstone create` invocation and build its gateway spec.
fn create(version: &str, source: &[&str]) -> anyhow::Result<spec::Create> {
    let extra = [&["--admin-id", "signer.testnet"], source].concat();
    let cli = parse(version, &extra).expect("redstone create should parse");

    match cli.command {
        Command::Redstone {
            command: RedstoneNs::Create(a),
        } => {
            let authorization = authorized(&a.signer);
            a.try_into_spec(&authorization)
        }
        _ => panic!("expected redstone create"),
    }
}

fn write_config_fixture(label: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-redstone-config-{}-{}.json",
        std::process::id(),
        label,
    ));
    std::fs::write(&path, bytes).expect("write redstone config fixture");
    path
}

#[rstest::rstest]
#[case::prod("prod", templar_common::oracle::redstone::config::prod())]
#[case::test("test", templar_common::oracle::redstone::config::test())]
fn create_preset_carries_the_builtin_config(
    #[case] preset: &str,
    #[case] expected: templar_common::oracle::redstone::Config,
) {
    let spec = create(VERSION_WITH_ADMIN, &["--preset", preset]).expect("into spec");

    assert_eq!(spec.target.name, "redstone");
    assert_eq!(spec.config, expected);
    assert_eq!(spec.admin_id.as_str(), "signer.testnet");
}

#[test]
fn create_parses_a_config_file_into_the_typed_config() {
    let expected = templar_common::oracle::redstone::config::test();
    let path = write_config_fixture(
        "typed",
        serde_json::to_vec(&expected)
            .expect("config is json")
            .as_ref(),
    );

    let spec = create(
        VERSION_WITH_ADMIN,
        &["--config-file", path.to_str().expect("utf-8 path")],
    )
    .expect("into spec");

    std::fs::remove_file(&path).expect("remove redstone config fixture");

    assert_eq!(spec.config, expected);
}

/// `--config-file` takes a bare `Config`, not the whole init args the retired
/// `--init-args-file` took. Every `Config` field is required, so a file in the old
/// shape is refused rather than read as a partly-default config.
#[test]
fn create_rejects_a_whole_init_args_file_as_a_config() {
    let init_args = serde_json::to_vec(&serde_json::json!({
        "config": templar_common::oracle::redstone::config::test(),
        "admin_id": "governance.testnet",
    }))
    .unwrap();
    let path = write_config_fixture("whole-init-args", &init_args);

    let error = create(
        VERSION_WITH_ADMIN,
        &["--config-file", path.to_str().expect("utf-8 path")],
    )
    .expect_err("a whole-init-args file is not a Config");

    std::fs::remove_file(&path).expect("remove redstone config fixture");

    assert!(error.to_string().contains("parse RedStone config"));
}

#[test]
fn create_requires_an_admin_id() {
    let error =
        parse(VERSION_WITH_ADMIN, &["--preset", "prod"]).expect_err("--admin-id is required");
    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

#[test]
fn create_carries_the_abi_check_opt_out() {
    let spec = create(
        "templar-redstone-adapter-contract@unreadable",
        &["--preset", "prod", "--skip-abi-check"],
    )
    .expect("the explicit opt-out must not parse the version key");

    assert!(spec.target.skip_abi_check);
}

#[rstest::rstest]
#[case::none(&[], clap::error::ErrorKind::MissingRequiredArgument)]
#[case::both(
    &["--preset", "prod", "--config-file", "config.json"],
    clap::error::ErrorKind::ArgumentConflict,
)]
fn create_takes_exactly_one_config_source(
    #[case] source: &[&str],
    #[case] expected: clap::error::ErrorKind,
) {
    let extra = [&["--admin-id", "signer.testnet"], source].concat();
    let error =
        parse(VERSION_WITH_ADMIN, &extra).expect_err("exactly one config source is required");
    assert_eq!(error.kind(), expected);
}

/// Opaque init args are `registry deploy`'s job now, not a mode on this command.
#[rstest::rstest]
#[case::inline("--init-args")]
#[case::file("--init-args-file")]
fn create_no_longer_takes_whole_init_args(#[case] flag: &str) {
    let error = parse(
        VERSION_WITH_ADMIN,
        &["--admin-id", "signer.testnet", flag, "{}"],
    )
    .expect_err("init-args flags are gone");
    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::UnknownArgument,
        "{error}"
    );
}

/// `redstone update-prices` is gone: `oracle update-red-stone` fetches the payload
/// inside the gateway rather than spawning the bridge CLI-side.
#[test]
fn update_prices_is_no_longer_a_redstone_subcommand() {
    let error = Cli::try_parse_from(
        [
            "tmplrmgr",
            "redstone",
            "update-prices",
            "--oracle-id",
            "redstone.testnet",
            "--feed-id",
            "BTC",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect_err("redstone update-prices should no longer parse");

    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
}
