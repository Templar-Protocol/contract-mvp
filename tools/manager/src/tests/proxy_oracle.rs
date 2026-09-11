use std::str::FromStr;

use clap::Parser;
use serde_json::{json, Value};

use near_sdk::json_types::{Base58CryptoHash, Base64VecU8, U128};
use near_sdk::Gas;

use templar_gateway_types::ProposalEncoding;

use super::{authorized, parse_create_proposal, parse_governance, try_parse_governance, CREDS};
use crate::cli::{Cli, Command};
use crate::commands::{
    proxy_oracle::{ProxyOracleGovernanceNs, ProxyOracleNs},
    signer::{Mode, PrintFormat},
};
use templar_common::upgrade::UpgradeSource;
use templar_common::Nanoseconds;
use templar_primitives::Decimal;
use templar_proxy_oracle_kernel::proxy::circuit_breaker::{CircuitBreaker, StepwiseChange};
use templar_proxy_oracle_near_governance_common::{
    target, FunctionCall, MethodPolicy, Operation, ReflexiveKind, ReflexiveOperation, Role,
};

const PRICE_ID: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
struct RemoveFileOnDrop<'a>(&'a std::path::Path);

impl Drop for RemoveFileOnDrop<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// A stand-in resolved governance account for the offline spec-building tests
/// (these pass `--governance-id gov.testnet`, so no resolution is exercised).
fn gov_account() -> near_account_id::AccountId {
    "gov.testnet".parse().expect("valid account id")
}

/// Parse a `create-proposal` invocation and return the built gateway spec.
fn create_proposal(
    args: &[&str],
) -> anyhow::Result<templar_gateway_methods_spec::proxy_oracle_governance::CreateProposal> {
    let cmd = parse_create_proposal(args.iter().copied());
    let id = cmd.id().expect("these tests pass --id explicitly");
    cmd.try_into_spec(gov_account(), id)
}

#[test]
fn governance_and_gov_parse_into_the_nested_command() {
    for alias in ["governance", "gov"] {
        let cli = Cli::try_parse_from([
            "tmplrmgr",
            "proxy-oracle",
            alias,
            "get-proxy-oracle-id",
            "--governance-id",
            "gov.testnet",
        ])
        .expect("governance alias should parse");

        match cli.command {
            Command::ProxyOracle {
                command: ProxyOracleNs::Governance(ProxyOracleGovernanceNs::GetProxyOracleId(_)),
            } => {}
            _ => panic!("expected nested governance command"),
        }
    }
}

#[test]
fn g_alias_is_rejected() {
    let error = Cli::try_parse_from(["tmplrmgr", "proxy-oracle", "g"])
        .expect_err("single-letter governance alias must stay unsupported");
    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
}

/// The typed `operation` a `create-proposal` invocation parses into.
fn operation(args: &[&str]) -> Operation {
    create_proposal(args).expect("into spec").operation
}

/// Parse a `proxy-oracle create` invocation and build its gateway spec.
fn oracle_create(
    version_key: &str,
    owner_id: Option<&str>,
) -> anyhow::Result<templar_gateway_methods_spec::proxy_oracle::Create> {
    let mut args = vec![
        "tmplrmgr",
        "proxy-oracle",
        "create",
        "--registry-id",
        "registry.testnet",
        "--name",
        "proxy-oracle-btc",
        "--version-key",
        version_key,
        "--deposit",
        "5 NEAR",
    ];
    if let Some(owner_id) = owner_id {
        args.extend_from_slice(&["--owner-id", owner_id]);
    }

    match Cli::try_parse_from(args.into_iter().chain(CREDS))
        .expect("proxy-oracle create should parse")
        .command
    {
        Command::ProxyOracle {
            command: ProxyOracleNs::Create(cmd),
        } => {
            let authorization = authorized(&cmd.signer);
            cmd.try_into_spec(&authorization)
        }
        _ => panic!("expected proxy-oracle create"),
    }
}

#[test]
fn upgrade_reads_local_wasm_and_carries_the_migration() {
    let wasm = b"\0asm\x01\0\0\0proxy-oracle";
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-proxy-upgrade-{}-{}.wasm",
        std::process::id(),
        line!()
    ));
    std::fs::write(&path, wasm).expect("write WASM fixture");

    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "proxy-oracle",
            "upgrade",
            "--oracle-id",
            "oracle.testnet",
            "--wasm",
            path.to_str().unwrap(),
            "--migration",
            "v0",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("upgrade command should parse");

    let spec = match cli.command {
        Command::ProxyOracle {
            command: ProxyOracleNs::Upgrade(cmd),
        } => cmd
            .try_into_spec("oracle.testnet".parse().expect("valid account id"))
            .expect("read WASM"),
        _ => panic!("expected proxy-oracle upgrade"),
    };
    std::fs::remove_file(&path).ok();

    assert_eq!(spec.wasm.0, wasm);
    assert_eq!(
        serde_json::to_value(spec).unwrap()["migration"],
        serde_json::json!("v0")
    );
}

const OWNER_AWARE_VERSION_KEY: &str = "owner-aware-test-alias";

/// `--owner-id` and its opaque registry key reach the gateway spec unchanged.
#[test]
fn create_carries_owner_id_without_inferring_constructor_from_version_key() {
    let spec = oracle_create(OWNER_AWARE_VERSION_KEY, Some("gov.testnet")).expect("into spec");

    assert_eq!(spec.target.name, "proxy-oracle-btc");
    assert_eq!(spec.target.version_key, OWNER_AWARE_VERSION_KEY);
    assert_eq!(spec.owner_id, Some("gov.testnet".parse().unwrap()));

    let json = serde_json::to_value(&spec).unwrap();
    serde_json::from_value::<templar_gateway_methods_spec::proxy_oracle::Create>(json)
        .expect("create params should match the gateway spec");
}

#[test]
fn set_role_operation_grants_and_revokes() {
    let granted = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "1",
        "self",
        "set-role",
        "--account-id",
        "op.testnet",
        "--role",
        "admin",
    ]);
    assert_eq!(
        granted,
        Operation::Reflexive(ReflexiveOperation::SetRole {
            account_id: "op.testnet".parse().unwrap(),
            role: Role::Admin,
            set: true,
        })
    );

    let revoked = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "1",
        "self",
        "set-role",
        "--account-id",
        "op.testnet",
        "--role",
        "circuit-breaker-operator",
        "--revoke",
    ]);
    assert_eq!(
        revoked,
        Operation::Reflexive(ReflexiveOperation::SetRole {
            account_id: "op.testnet".parse().unwrap(),
            role: Role::CircuitBreakerOperator,
            set: false,
        })
    );
}

#[test]
fn admin_function_call_operation_defaults_and_encodes_args() {
    let op = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "oracle",
        "call",
        "--method",
        "own_accept_owner",
        "--deposit",
        "1 yoctoNEAR",
    ]);
    // An arbitrary method call maps to the generic target form (identity on method/args/gas).
    assert_eq!(
        op,
        Operation::TargetFunctionCall(FunctionCall {
            method_name: "own_accept_owner".to_string(),
            args: Base64VecU8(b"{}".to_vec()),
            attached_deposit: U128(1),
            gas: Gas::from_tgas(30),
        })
    );
}

#[test]
fn set_reflexive_ttl_operation_builds_kind_and_ttl() {
    let op = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "5",
        "self",
        "set-reflexive-ttl",
        "--kind",
        "self-upgrade",
        "--ttl",
        "1d",
    ]);
    assert_eq!(
        op,
        Operation::Reflexive(ReflexiveOperation::SetReflexiveTtl {
            kind: ReflexiveKind::SelfUpgrade,
            ttl: Nanoseconds::from_secs(24 * 60 * 60),
        })
    );
}

#[test]
fn set_target_default_operation_builds_policy() {
    let op = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "5",
        "self",
        "set-target-default",
        "--ttl",
        "1d",
        "--role",
        "admin",
    ]);
    assert_eq!(
        op,
        Operation::Reflexive(ReflexiveOperation::SetTargetDefault {
            policy: MethodPolicy {
                ttl: Nanoseconds::from_secs(24 * 60 * 60),
                role: Role::Admin,
            },
        })
    );
}

#[test]
fn set_method_policy_operation_sets_and_resets() {
    let set = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "5",
        "self",
        "set-method-policy",
        "--method",
        "admin_set_proxy",
        "--ttl",
        "1h",
        "--role",
        "proxy-configuration-manager",
    ]);
    assert_eq!(
        set,
        Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
            method: "admin_set_proxy".to_string(),
            policy: Some(MethodPolicy {
                ttl: Nanoseconds::from_secs(60 * 60),
                role: Role::ProxyConfigurationManager,
            }),
        })
    );

    let reset = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "5",
        "self",
        "set-method-policy",
        "--method",
        "admin_set_proxy",
        "--reset",
    ]);
    assert_eq!(
        reset,
        Operation::Reflexive(ReflexiveOperation::SetMethodPolicy {
            method: "admin_set_proxy".to_string(),
            policy: None,
        })
    );
}

#[test]
fn set_method_policy_requires_ttl_unless_reset() {
    // Omitting --ttl on a real (non-reset) edit must fail to parse, not silently default to a
    // zero-timelock policy for that method.
    let err = try_parse_governance(["create-proposal"].into_iter().chain(CREDS).chain([
        "--governance-id",
        "gov.testnet",
        "--id",
        "5",
        "self",
        "set-method-policy",
        "--method",
        "admin_set_proxy",
        "--role",
        "admin",
    ]))
    .expect_err("missing --ttl should fail to parse");
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

#[test]
fn configure_circuit_breakers_operation_builds_config() {
    let op = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "2",
        "oracle",
        "configure-circuit-breakers",
        "--price-id",
        PRICE_ID,
        "--sample-interval",
        "1000ns",
        "--history-len",
        "8",
    ]);
    // The typed subcommand builds a generic call to the matching admin_* method.
    match op {
        Operation::TargetFunctionCall(FunctionCall {
            method_name, args, ..
        }) => {
            assert_eq!(method_name, "admin_configure_circuit_breakers");
            let decoded: Value = serde_json::from_slice(&args.0).expect("valid json args");
            assert_eq!(decoded["config"]["history_len"], json!(8));
            assert_eq!(decoded["config"]["sample_interval_ns"], json!("1000"));
        }
        reflexive @ Operation::Reflexive(_) => {
            panic!("expected target function call, got {reflexive:?}")
        }
    }
}

#[test]
fn set_proxy_gas_override_flows_through() {
    let with_gas = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "1",
        "oracle",
        "set-proxy",
        "--price-id",
        PRICE_ID,
        "--gas",
        "120 Tgas",
    ]);
    let Operation::TargetFunctionCall(call) = with_gas else {
        panic!("expected target function call");
    };
    assert_eq!(call.method_name, "admin_set_proxy");
    assert_eq!(call.gas, Gas::from_tgas(120));

    // Omitting --gas falls back to the 30 Tgas default.
    let default = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "1",
        "oracle",
        "set-proxy",
        "--price-id",
        PRICE_ID,
    ]);
    let Operation::TargetFunctionCall(call) = default else {
        panic!("expected target function call");
    };
    assert_eq!(call.gas, Gas::from_tgas(30));
}

#[test]
fn admin_upgrade_operation_reads_code_file() {
    let wasm = b"\0asm\x01\0\0\0upgrade";
    let path = std::env::temp_dir().join(format!(
        "tmplrmgr-upgrade-{}-{}.wasm",
        std::process::id(),
        line!()
    ));
    std::fs::write(&path, wasm).expect("write wasm fixture");

    let op = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "9",
        "oracle",
        "upgrade",
        "--code-file",
        path.to_str().unwrap(),
    ]);
    std::fs::remove_file(&path).ok();

    assert_eq!(
        op,
        Operation::TargetFunctionCall(
            target::admin_upgrade(
                UpgradeSource::Code(Base64VecU8(wasm.to_vec())),
                Base64VecU8(Vec::new()),
                None,
            )
            .unwrap()
        )
    );
}

#[test]
fn admin_upgrade_accepts_a_global_hash() {
    // 32 base58 '1's decode to 32 zero bytes — a valid code hash.
    let by_hash = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "9",
        "oracle",
        "upgrade",
        "--global-hash",
        "11111111111111111111111111111111",
    ]);
    assert_eq!(
        by_hash,
        Operation::TargetFunctionCall(
            target::admin_upgrade(
                UpgradeSource::GlobalHash(Base58CryptoHash::from([0u8; 32])),
                Base64VecU8(Vec::new()),
                None,
            )
            .unwrap()
        )
    );
}

#[test]
fn self_upgrade_operation_targets_the_governance_contract() {
    let op = operation(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "9",
        "self",
        "upgrade",
        "--global-hash",
        "11111111111111111111111111111111",
    ]);
    assert_eq!(
        op,
        Operation::Reflexive(ReflexiveOperation::SelfUpgrade {
            code: UpgradeSource::GlobalHash(Base58CryptoHash::from([0u8; 32])),
            migrate_args: Base64VecU8(Vec::new()),
        })
    );
}

#[test]
fn requested_ttl_defaults_to_zero_and_is_carried() {
    // Explicit --requested-ttl is carried through (in nanoseconds).
    let spec = create_proposal(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "3",
        "--requested-ttl",
        "42ns",
        "oracle",
        "remove-circuit-breaker",
        "--price-id",
        PRICE_ID,
        "--breaker-id",
        "0",
    ])
    .expect("into spec");
    let json = serde_json::to_value(&spec).unwrap();
    assert_eq!(json["requested_ttl"], json!("42"));
    match &spec.operation {
        Operation::TargetFunctionCall(call) => {
            assert_eq!(call.method_name, "admin_remove_circuit_breaker");
            let args: Value = serde_json::from_slice(&call.args.0).expect("valid json args");
            assert_eq!(args["breaker_id"], json!(0));
        }
        reflexive @ Operation::Reflexive(_) => {
            panic!("expected target function call, got {reflexive:?}")
        }
    }

    // Omitting --requested-ttl defaults it to zero.
    let defaulted = create_proposal(&[
        "--governance-id",
        "gov.testnet",
        "--id",
        "3",
        "oracle",
        "remove-circuit-breaker",
        "--price-id",
        PRICE_ID,
        "--breaker-id",
        "0",
    ])
    .expect("into spec");
    assert_eq!(
        serde_json::to_value(&defaulted).unwrap()["requested_ttl"],
        json!("0")
    );
}

#[rstest::rstest]
#[case::defaults_to_json(&[], ProposalEncoding::Json)]
#[case::opts_into_borsh(&["--encoding", "borsh"], ProposalEncoding::Borsh)]
fn create_proposal_carries_the_encoding(
    #[case] extra: &[&str],
    #[case] expected: ProposalEncoding,
) {
    let mut argv = vec!["--governance-id", "gov.testnet", "--id", "0"];
    argv.extend_from_slice(extra);
    argv.extend_from_slice(&[
        "oracle",
        "set-manual-trip",
        "--price-id",
        PRICE_ID,
        "--tripped",
    ]);

    let spec = create_proposal(&argv).expect("into spec");

    assert_eq!(spec.encoding, expected);
}

#[test]
fn create_proposal_id_is_optional_for_auto_fetch() {
    // With --id omitted, the id is left unresolved (the dispatcher fetches the
    // governance contract's next proposal id before writing).
    let cmd = parse_create_proposal([
        "--governance-id",
        "gov.testnet",
        "oracle",
        "set-proxy",
        "--price-id",
        PRICE_ID,
    ]);
    assert_eq!(cmd.id(), None);
    assert!(!cmd.execute_when_ready());
    // A resolved governance account and id flow through to the spec.
    let spec = cmd.try_into_spec(gov_account(), 7).expect("into spec");
    let json = serde_json::to_value(&spec).unwrap();
    assert_eq!(json["id"], json!(7));
    assert_eq!(json["governance_id"], json!("gov.testnet"));
}

#[test]
fn create_proposal_execute_when_ready_flag() {
    let cmd = parse_create_proposal([
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "--execute-when-ready",
        "oracle",
        "call",
        "--method",
        "own_accept_owner",
        "--deposit",
        "1 yoctoNEAR",
    ]);
    assert!(cmd.execute_when_ready());
}

#[test]
fn execute_proposal_when_ready_flag() {
    for (args, expected) in [
        (vec!["--governance-id", "gov.testnet", "--id", "2"], false),
        (
            vec![
                "--governance-id",
                "gov.testnet",
                "--id",
                "2",
                "--when-ready",
            ],
            true,
        ),
    ] {
        let command = parse_governance(["execute-proposal"].into_iter().chain(args).chain(CREDS));
        match command {
            ProxyOracleGovernanceNs::ExecuteProposal(cmd) => {
                assert_eq!(cmd.when_ready(), expected);
                assert_eq!(cmd.id(), 2);
            }
            _ => panic!("expected execute-proposal"),
        }
    }
}

#[test]
fn governance_write_commands_accept_print_mode() {
    let ProxyOracleGovernanceNs::ExecuteProposal(execute) = try_parse_governance([
        "execute-proposal",
        "--governance-id",
        "gov.testnet",
        "--id",
        "2",
        "--signer-id",
        "dao.near",
        "--print",
        "sputnik",
    ])
    .expect("immediate execution should support planning") else {
        panic!("expected execute-proposal");
    };
    assert_eq!(
        authorized(&execute.signer).mode(),
        &Mode::Plan(PrintFormat::Sputnik)
    );

    let ProxyOracleGovernanceNs::CreateProposal(create) = try_parse_governance([
        "create-proposal",
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "--signer-id",
        "dao.near",
        "--print",
        "json",
        "oracle",
        "call",
        "--method",
        "own_accept_owner",
        "--deposit",
        "1 yoctoNEAR",
    ])
    .expect("single-write proposal creation should support planning") else {
        panic!("expected create-proposal");
    };
    assert_eq!(
        authorized(&create.signer).mode(),
        &Mode::Plan(PrintFormat::Json)
    );
}

#[test]
fn proposal_orchestration_flags_conflict_with_print() {
    let execute = try_parse_governance([
        "execute-proposal",
        "--governance-id",
        "gov.testnet",
        "--id",
        "2",
        "--when-ready",
        "--signer-id",
        "dao.near",
        "--print",
        "json",
    ]);
    let create = try_parse_governance([
        "create-proposal",
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "--execute-when-ready",
        "--signer-id",
        "dao.near",
        "--print",
        "json",
        "oracle",
        "call",
        "--method",
        "own_accept_owner",
        "--deposit",
        "1 yoctoNEAR",
    ]);

    assert_eq!(
        execute
            .expect_err("--when-ready must conflict with --print")
            .kind(),
        clap::error::ErrorKind::ArgumentConflict
    );
    assert_eq!(
        create
            .expect_err("--execute-when-ready must conflict with --print")
            .kind(),
        clap::error::ErrorKind::ArgumentConflict
    );
}

#[test]
fn governance_create_builds_typed_init_fields() {
    let spec = match parse_governance(
        [
            "create",
            "--registry-id",
            "registry.testnet",
            "--name",
            "proxy-governance-market",
            "--version-key",
            "gov@1",
            "--proxy-oracle-id",
            "proxy-oracle-market.registry.testnet",
            "--admin-id",
            "operator.testnet",
            "--ttl-default",
            "0ns",
            "--deposit",
            "3.5 NEAR",
        ]
        .into_iter()
        .chain(CREDS),
    ) {
        ProxyOracleGovernanceNs::Create(cmd) => {
            let authorization = authorized(&cmd.signer);
            cmd.try_into_spec(&authorization).expect("into spec")
        }
        _ => panic!("expected governance create"),
    };

    assert_eq!(spec.target.name, "proxy-governance-market");
    assert_eq!(
        spec.proxy_oracle_id.as_str(),
        "proxy-oracle-market.registry.testnet"
    );
    assert_eq!(spec.admin_id.as_str(), "operator.testnet");

    // A uniform policy: every reflexive lock and the target default at the same (zero) TTL, no
    // per-method overrides.
    let policy = serde_json::to_value(&spec.policy).expect("policy is json");
    let reflexive = policy["reflexive_ttls"]
        .as_object()
        .expect("reflexive_ttls");
    for (_, v) in reflexive {
        assert_eq!(v, &json!("0"));
    }
    assert_eq!(policy["default_target"]["ttl"], json!("0"));
    assert_eq!(policy["default_target"]["role"], json!("Admin"));
    assert!(policy["method_policies"]
        .as_object()
        .expect("method_policies")
        .is_empty());
}

#[test]
fn governance_role_reads_parse() {
    let spec = match parse_governance([
        "list-role",
        "--governance-id",
        "gov.testnet",
        "--role",
        "proxy-configuration-manager",
        "--offset",
        "0",
        "--count",
        "10",
    ]) {
        ProxyOracleGovernanceNs::ListRole(cmd) => cmd.into_spec(gov_account()),
        _ => panic!("expected list-role"),
    };
    assert_eq!(
        serde_json::to_value(&spec).unwrap(),
        json!({
            "governance_id": "gov.testnet",
            "role": "ProxyConfigurationManager",
            "offset": 0,
            "count": 10
        })
    );
}

#[test]
fn oracle_update_prices_collects_repeated_ids() {
    let borrow = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "proxy-oracle",
            "update-prices",
            "--oracle-id",
            "proxy.testnet",
            "--price-id",
            PRICE_ID,
            "--price-id",
            borrow,
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("update-prices should parse");
    let spec = match cli.command {
        Command::ProxyOracle {
            command: ProxyOracleNs::UpdatePrices(cmd),
        } => cmd.into_spec("proxy.testnet".parse().expect("valid account id")),
        _ => panic!("expected update-prices"),
    };
    assert_eq!(spec.price_ids.len(), 2);
}

#[test]
fn add_circuit_breaker_breaker_id_is_optional_and_resolvable() {
    let breaker_file = std::env::temp_dir().join(format!(
        "tmplrmgr-circuit-breaker-{}-{}.json",
        std::process::id(),
        line!()
    ));
    let _breaker_file_cleanup = RemoveFileOnDrop(&breaker_file);
    let breaker = CircuitBreaker::StepwiseChange(StepwiseChange {
        max_relative_change: Decimal::from_str("0.1").unwrap(),
    });
    std::fs::write(
        &breaker_file,
        serde_json::to_vec(&breaker).expect("serialize breaker fixture"),
    )
    .expect("write breaker fixture");
    let breaker_file_arg = breaker_file.to_str().expect("fixture path is unicode");

    // Omitting --breaker-id marks the proposal for auto-resolution.
    let parse_unresolved = || {
        parse_create_proposal([
            "--governance-id",
            "gov.testnet",
            "--id",
            "0",
            "oracle",
            "add-circuit-breaker",
            "--price-id",
            PRICE_ID,
            "--breaker-file",
            breaker_file_arg,
        ])
    };
    let unresolved = parse_unresolved();
    let expected = crate::commands::proxy_oracle::parse_price_identifier(PRICE_ID).unwrap();
    assert_eq!(unresolved.unresolved_breaker_price_id(), Some(expected));
    let error = unresolved
        .try_into_spec(gov_account(), 0)
        .expect_err("building a spec must reject an unresolved breaker id");
    assert!(
        error.to_string().contains("breaker id must be resolved"),
        "{error:#}"
    );

    let mut resolved = parse_unresolved();
    resolved.set_breaker_id(4);
    // Once resolved, it is no longer flagged for auto-fetch.
    assert_eq!(resolved.unresolved_breaker_price_id(), None);
    let spec = resolved
        .try_into_spec(gov_account(), 0)
        .expect("resolved breaker id should build");
    match &spec.operation {
        Operation::TargetFunctionCall(call) => {
            assert_eq!(call.method_name, "admin_add_circuit_breaker");
            let args: Value = serde_json::from_slice(&call.args.0).expect("valid json args");
            assert_eq!(args["breaker_id"], json!(4));
        }
        reflexive @ Operation::Reflexive(_) => {
            panic!("expected target function call, got {reflexive:?}")
        }
    }

    // An explicit --breaker-id needs no resolution.
    let explicit = parse_create_proposal([
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "oracle",
        "add-circuit-breaker",
        "--price-id",
        PRICE_ID,
        "--breaker-id",
        "7",
        "--breaker-file",
        "/does/not/need/to/exist.json",
    ]);
    assert_eq!(explicit.unresolved_breaker_price_id(), None);
}

/// A `proxy-oracle` command takes exactly one of `--oracle-id` / `--market-id`.
#[test]
fn oracle_target_requires_exactly_one_selector() {
    let base = [
        "tmplrmgr",
        "proxy-oracle",
        "get-proxy",
        "--price-id",
        PRICE_ID,
    ];

    // Neither selector: the required group is unsatisfied.
    Cli::try_parse_from(base).expect_err("a target selector is required");

    // Both selectors: they are mutually exclusive.
    Cli::try_parse_from(base.into_iter().chain([
        "--oracle-id",
        "oracle.testnet",
        "--market-id",
        "market.testnet",
    ]))
    .expect_err("--oracle-id and --market-id are mutually exclusive");

    // Either alone parses.
    for selector in [
        ["--oracle-id", "oracle.testnet"],
        ["--market-id", "market.testnet"],
    ] {
        Cli::try_parse_from(base.into_iter().chain(selector))
            .expect("exactly one selector should parse");
    }
}

/// A `proxy-oracle gov` command takes exactly one of `--governance-id` /
/// `--oracle-id` / `--market-id`.
#[test]
fn governance_target_requires_exactly_one_selector() {
    let base = ["tmplrmgr", "proxy-oracle", "gov", "get-proxy-oracle-id"];

    // Neither selector: the required group is unsatisfied.
    Cli::try_parse_from(base).expect_err("a target selector is required");

    // Two selectors: mutually exclusive.
    Cli::try_parse_from(base.into_iter().chain([
        "--governance-id",
        "gov.testnet",
        "--oracle-id",
        "oracle.testnet",
    ]))
    .expect_err("governance selectors are mutually exclusive");

    // Each alone parses.
    for selector in [
        ["--governance-id", "gov.testnet"],
        ["--oracle-id", "oracle.testnet"],
        ["--market-id", "market.testnet"],
    ] {
        Cli::try_parse_from(base.into_iter().chain(selector))
            .expect("exactly one selector should parse");
    }
}

/// `proxy-oracle preflight` widens the usual target group with `--registry-id`, which sweeps a
/// whole fleet, but the three remain mutually exclusive.
#[test]
fn preflight_target_requires_exactly_one_selector() {
    let base = ["tmplrmgr", "proxy-oracle", "preflight"];

    Cli::try_parse_from(base).expect_err("a target selector is required");

    Cli::try_parse_from(base.into_iter().chain([
        "--oracle-id",
        "oracle.testnet",
        "--registry-id",
        "registry.testnet",
    ]))
    .expect_err("preflight selectors are mutually exclusive");

    for selector in [
        ["--oracle-id", "oracle.testnet"],
        ["--market-id", "market.testnet"],
        ["--registry-id", "registry.testnet"],
    ] {
        Cli::try_parse_from(base.into_iter().chain(selector))
            .expect("exactly one selector should parse");
    }
}

#[test]
fn oracle_code_upgrade_proposals_require_preflight() {
    let upgrade = parse_create_proposal([
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "oracle",
        "upgrade",
        "--global-hash",
        "11111111111111111111111111111111",
    ]);
    assert!(upgrade.requires_upgrade_preflight());

    let raw_upgrade = parse_create_proposal([
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "oracle",
        "call",
        "--method",
        target::method::UPGRADE,
    ]);
    assert!(raw_upgrade.requires_upgrade_preflight());

    let set_manual_trip = parse_create_proposal([
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "oracle",
        "set-manual-trip",
        "--price-id",
        PRICE_ID,
        "--tripped",
    ]);
    assert!(!set_manual_trip.requires_upgrade_preflight());
}

/// `--skip-preflight` is an opt-out, so its absence must leave the gate armed.
#[test]
fn the_upgrade_preflight_is_on_unless_opted_out() {
    let armed = parse_create_proposal([
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "oracle",
        "upgrade",
        "--global-hash",
        "11111111111111111111111111111111",
    ]);
    assert!(armed.preflight.runs(&Mode::Keychain));
    // A plan builds a payload without submitting, so it stays offline.
    assert!(!armed.preflight.runs(&Mode::Plan(PrintFormat::Json)));

    let opted_out = parse_create_proposal([
        "--governance-id",
        "gov.testnet",
        "--id",
        "0",
        "--skip-preflight",
        "oracle",
        "upgrade",
        "--global-hash",
        "11111111111111111111111111111111",
    ]);
    assert!(!opted_out.preflight.runs(&Mode::Keychain));
}
