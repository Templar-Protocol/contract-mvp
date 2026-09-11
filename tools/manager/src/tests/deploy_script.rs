use clap::Parser;
use serde_json::{json, Value};

use crate::cli::{Cli, Command};
use crate::commands::owner::OwnerNs;
use crate::commands::proxy_oracle::ProxyOracleGovernanceNs;
use crate::commands::registry::RegistryNs;

use super::{authorized, parse_create_proposal, parse_governance, try_parse_governance, CREDS};

const COLLATERAL_PRICE_ID: &str =
    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[test]
fn registry_deploy_requires_init_args() {
    let error = Cli::try_parse_from(
        [
            "tmplrmgr",
            "registry",
            "deploy",
            "--registry-id",
            "registry.testnet",
            "--name",
            "proxy-oracle-market",
            "--version-key",
            "proxy@1",
            "--deposit",
            "3.5 NEAR",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect_err("deploy with no init args should fail to parse");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

#[test]
fn registry_deploy_rejects_both_init_args_sources() {
    let error = Cli::try_parse_from(
        [
            "tmplrmgr",
            "registry",
            "deploy",
            "--registry-id",
            "registry.testnet",
            "--name",
            "proxy-oracle-market",
            "--version-key",
            "proxy@1",
            "--deposit",
            "3.5 NEAR",
            "--init-args",
            "null",
            "--init-args-file",
            "/dev/null",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect_err("deploy with both init-args sources should fail to parse");

    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn registry_deploy_rejects_invalid_inline_init_args() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "registry",
            "deploy",
            "--registry-id",
            "registry.testnet",
            "--name",
            "proxy-oracle-market",
            "--version-key",
            "proxy@1",
            "--deposit",
            "3.5 NEAR",
            "--init-args",
            "{not json",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("registry deploy should parse");

    match cli.command {
        Command::Registry {
            command: RegistryNs::Deploy(cmd),
        } => {
            let authorization = authorized(&cmd.signer);
            cmd.try_into_spec(&authorization)
                .expect_err("invalid inline JSON should be rejected")
        }
        _ => panic!("expected Registry::Deploy"),
    };
}

#[test]
fn registry_deploy_accepts_explicit_inline_null_init_args() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "registry",
            "deploy",
            "--registry-id",
            "registry.testnet",
            "--name",
            "proxy-oracle-market",
            "--version-key",
            "proxy@1",
            "--deposit",
            "3.5 NEAR",
            "--init-args",
            "null",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("registry deploy should parse");

    let params = match cli.command {
        Command::Registry {
            command: RegistryNs::Deploy(cmd),
        } => {
            let authorization = authorized(&cmd.signer);
            cmd.try_into_spec(&authorization)
                .expect("deploy should build")
        }
        _ => panic!("expected Registry::Deploy"),
    };

    let params_json = serde_json::to_value(&params).unwrap();
    assert_eq!(params_json["init_args"], json!("bnVsbA=="));
    serde_json::from_value::<templar_gateway_methods_spec::registry::Deploy>(params_json)
        .expect("typed registry.deploy params should match the gateway spec");
}

#[test]
fn registry_deploy_reads_init_args_file() {
    let init_args = br#"{"owner_id":"operator.testnet"}"#;
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-deploy-init-args-{}-{}.json",
        std::process::id(),
        line!(),
    ));
    std::fs::write(&path, init_args).expect("write init args fixture");

    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "registry",
            "deploy",
            "--registry-id",
            "registry.testnet",
            "--name",
            "proxy-oracle",
            "--version-key",
            "proxy-oracle@1",
            "--init-args-file",
            path.to_str().expect("fixture path is unicode"),
            "--deposit",
            "1 yoctoNEAR",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("registry deploy with init-args-file should parse");

    let params = match cli.command {
        Command::Registry {
            command: RegistryNs::Deploy(cmd),
        } => {
            let authorization = authorized(&cmd.signer);
            cmd.try_into_spec(&authorization)
                .expect("deploy should build")
        }
        _ => panic!("expected Registry::Deploy"),
    };

    std::fs::remove_file(&path).expect("remove init args fixture");

    let params_json = serde_json::to_value(&params).unwrap();
    assert_eq!(params_json["init_args"], json!(base64_of(init_args)));
}

#[test]
fn owner_typed_commands_parse() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "owner",
            "propose",
            "--contract-id",
            "registry.testnet",
            "--account-id",
            "operator.testnet",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("owner propose should parse");

    let params = match cli.command {
        Command::Owner {
            command: OwnerNs::Propose(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected Owner::Propose"),
    };

    let params_json = serde_json::to_value(&params).unwrap();
    assert_eq!(params_json["contract_id"], json!("registry.testnet"));
    serde_json::from_value::<templar_gateway_methods_spec::owner::ProposeOwner>(params_json)
        .expect("typed owner.propose params should match the gateway spec");

    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "owner",
            "accept",
            "--contract-id",
            "registry.testnet",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("owner accept should parse");

    let params = match cli.command {
        Command::Owner {
            command: OwnerNs::Accept(cmd),
        } => cmd.into_spec(),
        _ => panic!("expected Owner::Accept"),
    };

    serde_json::from_value::<templar_gateway_methods_spec::owner::AcceptOwner>(
        serde_json::to_value(&params).unwrap(),
    )
    .expect("typed owner.accept params should match the gateway spec");
}

#[test]
fn governance_create_proposal_reshapes_legacy_proxy_file() {
    let legacy_entries = json!([{
        "source": {"Request": {"Pyth": {
            "oracle_id": "pyth-oracle.testnet",
            "price_id": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
        }}},
        "weight": 1
    }]);
    let proxy_file = write_legacy_proxy_fixture(&legacy_entries);

    // The helper injects credentials at the `create-proposal` level, before the
    // `set-proxy` operation subcommand.
    let cmd = parse_create_proposal([
        "--governance-id",
        "proxy.registry.testnet",
        "--id",
        "0",
        "oracle",
        "set-proxy",
        "--price-id",
        COLLATERAL_PRICE_ID,
        "--proxy-file",
        proxy_file.to_str().expect("fixture path is unicode"),
    ]);
    let params = cmd
        .try_into_spec(
            "proxy.registry.testnet".parse().expect("valid account id"),
            0,
        )
        .expect("create-proposal should build");

    std::fs::remove_file(&proxy_file).expect("remove proxy fixture");

    // The set-proxy subcommand now builds a generic admin_set_proxy target call; the reshaped proxy
    // lives in the (base64) call args.
    let proxy = match &params.operation {
        templar_proxy_oracle_near_governance_common::Operation::TargetFunctionCall(call) => {
            assert_eq!(call.method_name, "admin_set_proxy");
            let args: Value = serde_json::from_slice(&call.args.0).expect("valid json args");
            args["proxy"].clone()
        }
        reflexive @ templar_proxy_oracle_near_governance_common::Operation::Reflexive(_) => {
            panic!("expected admin_set_proxy target call, got {reflexive:?}")
        }
    };
    assert_eq!(proxy["aggregator"]["MedianLow"]["sources"], legacy_entries);
    assert_eq!(proxy["aggregator"]["MedianLow"]["min_sources"], json!(1));

    let params_json = serde_json::to_value(&params).unwrap();
    serde_json::from_value::<templar_gateway_methods_spec::proxy_oracle_governance::CreateProposal>(
        params_json,
    )
    .expect("typed createProposal params should match the gateway spec");
}

#[test]
fn governance_execute_proposal_typed_args_parse() {
    let params = match parse_governance(
        [
            "execute-proposal",
            "--governance-id",
            "proxy.registry.testnet",
            "--id",
            "0",
        ]
        .into_iter()
        .chain(CREDS),
    ) {
        ProxyOracleGovernanceNs::ExecuteProposal(cmd) => {
            cmd.into_spec("proxy.registry.testnet".parse().expect("valid account id"))
        }
        _ => panic!("expected execute-proposal"),
    };

    serde_json::from_value::<templar_gateway_methods_spec::proxy_oracle_governance::ExecuteProposal>(
        serde_json::to_value(&params).unwrap(),
    )
    .expect("typed executeProposal params should match the gateway spec");
}

#[test]
fn create_proposal_set_proxy_requires_price_id() {
    let error = try_parse_governance(
        [
            "create-proposal",
            "--governance-id",
            "proxy.registry.testnet",
            "--id",
            "0",
        ]
        .into_iter()
        .chain(CREDS)
        .chain(["oracle", "set-proxy"]),
    )
    .expect_err("set-proxy should require --price-id");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

#[test]
fn json_fallback_still_works_for_create_proposal() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "write",
            "proxyOracleGovernance.createProposal",
            "--json",
            r#"{"governance_id":"proxy.registry.testnet","id":0,"operation":{"TargetFunctionCall":{"method_name":"admin_set_proxy","args":"e30=","attached_deposit":"0","gas":"30000000000000"}},"requested_ttl":"0"}"#,
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("write fallback should parse");

    let params = match cli.command {
        Command::Write(call) => {
            let json_str = call.call.json.expect("json should be present");
            serde_json::from_str(&json_str).expect("json should parse")
        }
        _ => panic!("expected Write variant"),
    };

    serde_json::from_value::<templar_gateway_methods_spec::proxy_oracle_governance::CreateProposal>(
        params,
    )
    .expect("JSON fallback createProposal params should match the gateway spec");
}

fn write_legacy_proxy_fixture(entries: &Value) -> std::path::PathBuf {
    let payload = json!({
        "aggregator": {
            "method": "MedianLow",
            "filter": {
                "min_sources": 1,
                "max_age": "60000000000",
                "max_clock_drift": "10000000000"
            }
        },
        "entries": entries,
    });
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-proxy-fixture-{}-{}.json",
        std::process::id(),
        line!(),
    ));
    std::fs::write(&path, payload.to_string()).expect("write proxy fixture");
    path
}

fn base64_of(bytes: &[u8]) -> String {
    use templar_gateway_types::Base64Bytes;
    serde_json::to_value(Base64Bytes(bytes.to_vec()))
        .expect("Base64Bytes serializes to a string")
        .as_str()
        .expect("base64 value is a string")
        .to_owned()
}
