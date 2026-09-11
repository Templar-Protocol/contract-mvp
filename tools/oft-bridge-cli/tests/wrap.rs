//! Wrap planner tests: the native SAC derivation is pinned to the known
//! testnet XLM SAC (well-known Stellar constant, recorded in the live route
//! evidence); plan shape and fail-closed boundaries are behavioral.

use std::collections::BTreeMap;

use templar_oft_bridge_cli::domain::{
    AssetKind, AssetPolicyV1, ChainIdentityV1, DesiredRouteV1, Environment, OperationV1,
};
use templar_oft_bridge_cli::wrap::{plan_wrap, token_contract};

const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";
/// Well-known native XLM SAC on Stellar testnet; recorded in the live route
/// evidence and derivable from the official XDR preimage rules.
const TESTNET_NATIVE_SAC: &str = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
const OPERATOR: &str = "GCLQ3APIE5AS4XJUTRP5AF7ZMQAXDNGIRMIF3MIWQPF6ZPFJVNJDCN5E";

fn desired(kind: AssetKind, asset_id: &str) -> DesiredRouteV1 {
    DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: 1,
        route_id: "route-wrap-test".into(),
        identity: ChainIdentityV1 {
            environment: Environment::StellarTestnetSepolia,
            stellar_passphrase: TESTNET_PASSPHRASE.into(),
            stellar_eid: 40600,
            stellar_endpoint: "CENDPOINT".into(),
            stellar_endpoint_code_hash: "0".repeat(64),
            evm_chain_id: 11_155_111,
            evm_eid: 40161,
            evm_endpoint: "0x6EDCE65403992e310A62460808c4b910D972f10f".into(),
            evm_endpoint_code_hash: "0".repeat(64),
        },
        asset: AssetPolicyV1 {
            kind,
            asset_id: asset_id.into(),
            local_decimals: 7,
            issuer_custodian_evidence_sha256: None,
            destination_acceptance_evidence_sha256: None,
            custody_risk_acceptance_sha256: None,
            forbidden_classic_issuer: None,
            evidence: BTreeMap::new(),
        },
        stellar_owner: OPERATOR.into(),
        stellar_delegate: OPERATOR.into(),
        evm_owner: "0xc61B17BF20b4B16bb70C1942CD8D9eBDe6726386".into(),
        evm_delegate: "0xc61B17BF20b4B16bb70C1942CD8D9eBDe6726386".into(),
        config: BTreeMap::new(),
    }
}

#[test]
fn native_sac_derivation_matches_known_testnet_sac() {
    let contract = templar_oft_bridge_cli::codec::derive_native_sac_contract(TESTNET_PASSPHRASE)
        .expect("native sac derivation");
    assert_eq!(contract, TESTNET_NATIVE_SAC);
}

#[test]
fn stellar_contract_derivation_is_deterministic_and_input_bound() {
    let salt = templar_oft_bridge_cli::wrap::stellar_salt("route-wrap-test", "native");
    let derived = templar_oft_bridge_cli::codec::derive_stellar_contract_address(
        TESTNET_PASSPHRASE,
        OPERATOR,
        &salt,
    )
    .expect("contract derivation");
    assert!(derived.starts_with('C'));
    let again = templar_oft_bridge_cli::codec::derive_stellar_contract_address(
        TESTNET_PASSPHRASE,
        OPERATOR,
        &salt,
    )
    .expect("repeat derivation");
    assert_eq!(derived, again);
    let other_salt = templar_oft_bridge_cli::wrap::stellar_salt("route-wrap-test", "other");
    let different = templar_oft_bridge_cli::codec::derive_stellar_contract_address(
        TESTNET_PASSPHRASE,
        OPERATOR,
        &other_salt,
    )
    .expect("other derivation");
    assert_ne!(derived, different, "salt must bind the derived address");
}

#[test]
fn native_sac_wrap_plan_is_concrete_and_deterministic() {
    let d = desired(AssetKind::NativeSac, "native");
    let plan = plan_wrap(&d, "desired-hash", "Wrapped XLM", "wXLM", 7, false).expect("wrap plan");
    assert_eq!(plan.stellar_token_contract, TESTNET_NATIVE_SAC);
    assert!(plan.stellar_oft.starts_with('C'));
    assert!(plan.evm_oft.starts_with("0x") && plan.evm_oft.len() == 42);
    assert_eq!(plan.evm_nonce, 7);
    assert_eq!(plan.operations.len(), 5);
    match &plan.operations[0] {
        OperationV1::InstallStellarWasm { wasm_sha256 } => assert_eq!(wasm_sha256.len(), 64),
        other => panic!("node 0 must install the wasm, got {other:?}"),
    }
    match &plan.operations[3] {
        OperationV1::SetStellarPeer { remote_eid, peer } => {
            assert_eq!(*remote_eid, 40161);
            assert!(peer.starts_with("0x") && peer.len() == 66, "peer {peer}");
        }
        other => panic!("node 3 must set the stellar peer, got {other:?}"),
    }
    match &plan.operations[4] {
        OperationV1::SetEvmPeer { remote_eid, peer } => {
            assert_eq!(*remote_eid, 40600);
            assert!(peer.starts_with("0x") && peer.len() == 66, "peer {peer}");
        }
        other => panic!("node 4 must set the evm peer, got {other:?}"),
    }
    let repeat =
        plan_wrap(&d, "desired-hash", "Wrapped XLM", "wXLM", 7, false).expect("repeat plan");
    assert_eq!(plan, repeat, "wrap planning must be deterministic");
}

#[test]
fn usdc_is_refused_before_anything_else() {
    let d = desired(AssetKind::Usdc, "usdc");
    let error = plan_wrap(&d, "h", "n", "s", 0, false).expect_err("usdc refused");
    assert_eq!(error.to_string(), "policy refused: unsupported_use_cctp");
}

#[test]
fn issued_asset_requires_evidence_when_demanded() {
    let mut d = desired(AssetKind::IssuedSep41, TESTNET_NATIVE_SAC);
    let error = plan_wrap(&d, "h", "n", "s", 0, true).expect_err("evidence demanded");
    assert!(error
        .to_string()
        .contains("issuer_custodian_evidence_sha256"));
    // With evidence digests the boundary passes.
    d.asset.issuer_custodian_evidence_sha256 = Some("a".repeat(64));
    d.asset.destination_acceptance_evidence_sha256 = Some("b".repeat(64));
    d.asset.custody_risk_acceptance_sha256 = Some("c".repeat(64));
    plan_wrap(&d, "h", "n", "s", 0, true).expect("evidence satisfied");
}

#[test]
fn issued_asset_requires_contract_identifier() {
    let d = desired(AssetKind::IssuedSep41, "USDCFOO");
    let error = token_contract(TESTNET_PASSPHRASE, &d.asset).expect_err("not a contract");
    assert!(error.to_string().contains("contract identifier"));
}

#[test]
fn forbidden_issuer_cannot_be_operator() {
    let mut d = desired(AssetKind::NativeSac, "native");
    d.asset.forbidden_classic_issuer = Some(OPERATOR.into());
    let error = plan_wrap(&d, "h", "n", "s", 0, false).expect_err("issuer excluded");
    assert!(error.to_string().contains("forbidden classic issuer"));
}

#[test]
fn empty_name_and_symbol_are_rejected() {
    let d = desired(AssetKind::NativeSac, "native");
    assert!(plan_wrap(&d, "h", "", "s", 0, false).is_err());
    assert!(plan_wrap(&d, "h", "n", "  ", 0, false).is_err());
}
