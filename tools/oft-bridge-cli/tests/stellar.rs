use std::collections::BTreeMap;

use templar_oft_bridge_cli::domain::{
    AssetKind, AssetPolicyV1, ChainIdentityV1, Environment, RouteStateV1, SCHEMA_VERSION,
};

use stellar_baselib::{
    account::{Account, AccountBehavior as _},
    transaction::TransactionBehavior as _,
    transaction_builder::{TransactionBuilder, TransactionBuilderBehavior as _},
    xdr::{Limits, ReadXdr as _, TransactionEnvelope, WriteXdr as _},
};
use templar_oft_bridge_cli::stellar::{sign_envelope, StellarSecretProviderV1};

#[test]
fn assembled_envelope_is_signed_without_exposing_seed() {
    const SECRET: &str = "SD7X7LEHBNMUIKQGKPARG5TDJNBHKC346OUARHGZL5ITC6IJPXHILY36";
    const PUBLIC: &str = "GDFQVQCYYB7GKCGSCUSIQYXTPLV5YJ3XWDMWGQMDNM4EAXAL7LITIBQ7";
    const PASSPHRASE: &str = "Test SDF Network ; September 2015";
    const ENV_NAME: &str = "TMPLR_OFT_TEST_STELLAR_SECRET";

    std::env::set_var(ENV_NAME, SECRET);
    let provider = StellarSecretProviderV1::from_named_env(ENV_NAME).expect("secret provider");
    std::env::remove_var(ENV_NAME);
    assert_eq!(provider.public_key(), PUBLIC);

    let mut account = Account::new(PUBLIC, "1").expect("account");
    let envelope = TransactionBuilder::new(&mut account, PASSPHRASE, None)
        .fee(100u32)
        .build_for_simulation()
        .to_envelope()
        .expect("envelope")
        .to_xdr_base64(Limits::none())
        .expect("XDR");
    let signed = sign_envelope(&envelope, PASSPHRASE, &provider).expect("sign");
    let signed = TransactionEnvelope::from_xdr_base64(&signed, Limits::none()).expect("signed XDR");
    let signatures = match signed {
        TransactionEnvelope::Tx(envelope) => envelope.signatures.len(),
        other => panic!("unexpected envelope: {other:?}"),
    };
    assert_eq!(signatures, 1);
}

fn route_state() -> RouteStateV1 {
    const OWNER: &str = "GDFQVQCYYB7GKCGSCUSIQYXTPLV5YJ3XWDMWGQMDNM4EAXAL7LITIBQ7";
    const OFT: &str = "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV";
    let contracts = [
        ("stellar_owner", OWNER),
        ("stellar_oft", OFT),
        ("stellar_role:FEE_CONFIG_MANAGER_ROLE", OWNER),
        ("stellar_role:RATE_LIMITER_MANAGER_ROLE", OWNER),
        ("stellar_role:PAUSER_ROLE", OWNER),
        ("stellar_role:UNPAUSER_ROLE", OWNER),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    RouteStateV1 {
        schema_name: "route_state".into(),
        schema_version: SCHEMA_VERSION,
        route_id: "stellar-codec".into(),
        desired_sha256: "1".repeat(64),
        identity: ChainIdentityV1 {
            environment: Environment::StellarTestnetSepolia,
            stellar_passphrase: "Test SDF Network ; September 2015".into(),
            stellar_eid: 40_600,
            stellar_endpoint: OFT.into(),
            stellar_endpoint_code_hash: "2".repeat(64),
            evm_chain_id: 11_155_111,
            evm_eid: 40_161,
            evm_endpoint: "0x1111111111111111111111111111111111111111".into(),
            evm_endpoint_code_hash: "3".repeat(64),
        },
        asset: AssetPolicyV1 {
            kind: AssetKind::NativeSac,
            asset_id: "native".into(),
            local_decimals: 7,
            issuer_custodian_evidence_sha256: None,
            destination_acceptance_evidence_sha256: None,
            custody_risk_acceptance_sha256: None,
            forbidden_classic_issuer: None,
            evidence: BTreeMap::new(),
        },
        opening_custody: None,
        operations_log: "operations.jsonl".into(),
        messages_log: "messages.jsonl".into(),
        lock_file: ".lock".into(),
        contracts,
        requested_config: BTreeMap::new(),
        effective_config: BTreeMap::new(),
    }
}

#[test]
fn native_stellar_operation_binds_exact_arguments_and_typed_config() {
    let operation = templar_oft_bridge_cli::domain::OperationV1::SetStellarPeer {
        remote_eid: 40_161,
        peer: "0x1111111111111111111111111111111111111111".into(),
    };
    let invocation = templar_oft_bridge_cli::layerzero::build_stellar_operation_for_route(
        &route_state(),
        &operation,
    )
    .expect("operation");
    assert_eq!(invocation.function, "set_peer");
    assert_eq!(invocation.args_xdr_hex.len(), 3);
    let first = hex::decode(&invocation.args_xdr_hex[0]).expect("hex");
    assert_eq!(
        stellar_baselib::xdr::ScVal::from_xdr(first, Limits::none()).expect("XDR"),
        stellar_baselib::xdr::ScVal::U32(40_161)
    );

    let config = templar_oft_bridge_cli::layerzero::UlnConfigType3V1 {
        required_dvns: vec!["GDFQVQCYYB7GKCGSCUSIQYXTPLV5YJ3XWDMWGQMDNM4EAXAL7LITIBQ7".into()],
        optional_dvns: vec![],
        optional_threshold: 0,
        confirmations: 1,
        use_default_confirmations: false,
        use_default_required_dvns: false,
        use_default_optional_dvns: false,
    };
    let operation = templar_oft_bridge_cli::layerzero::set_uln_operation(
        templar_oft_bridge_cli::domain::Vm::Stellar,
        40_161,
        "send",
        "GDFQVQCYYB7GKCGSCUSIQYXTPLV5YJ3XWDMWGQMDNM4EAXAL7LITIBQ7",
        "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV",
        "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV",
        &config,
    )
    .unwrap();
    let invocation =
        templar_oft_bridge_cli::layerzero::build_stellar_operation(&operation).unwrap();
    assert_eq!(invocation.function, "set_config");
    assert_eq!(invocation.args_xdr_hex.len(), 4);
}

#[test]
fn official_oft_management_signatures_include_authorizers_and_structs() {
    use templar_oft_bridge_cli::{
        domain::OperationV1, layerzero::build_stellar_operation_for_route,
    };

    let state = route_state();
    for (operation, function, argument_count) in [
        (
            OperationV1::SetDefaultFee { bps: 25 },
            "set_default_fee_bps",
            2,
        ),
        (
            OperationV1::SetDestinationFee {
                remote_eid: 40_161,
                bps: 20,
            },
            "set_fee_bps",
            3,
        ),
        (
            OperationV1::SetOutboundRateLimit {
                remote_eid: 40_161,
                limit_raw: 1_000_000,
                window_seconds: 60,
                mode: "net".into(),
            },
            "set_rate_limit",
            4,
        ),
        (OperationV1::PauseEmergency, "pause", 1),
        (
            OperationV1::SetTtlConfig {
                instance_threshold: 10,
                instance_extend_to: 20,
                persistent_threshold: 30,
                persistent_extend_to: 40,
            },
            "set_ttl_configs",
            2,
        ),
        (
            OperationV1::GrantRole {
                role: "PAUSER_ROLE".into(),
                address: "GDFQVQCYYB7GKCGSCUSIQYXTPLV5YJ3XWDMWGQMDNM4EAXAL7LITIBQ7".into(),
            },
            "grant_role",
            3,
        ),
    ] {
        let invocation = build_stellar_operation_for_route(&state, &operation).unwrap();
        assert_eq!(invocation.function, function);
        assert_eq!(invocation.args_xdr_hex.len(), argument_count);
    }
}
