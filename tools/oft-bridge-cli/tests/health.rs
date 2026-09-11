use std::collections::BTreeMap;

use templar_oft_bridge_cli::{
    canonical_sha256,
    domain::{
        AssetKind, AssetPolicyV1, ChainIdentityV1, DesiredRouteV1, Environment, OpeningCustodyV1,
        SCHEMA_VERSION,
    },
    error::Error,
    health,
    state::RouteStore,
    ttl,
};

fn desired() -> DesiredRouteV1 {
    DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: SCHEMA_VERSION,
        route_id: "health-route".into(),
        identity: ChainIdentityV1 {
            environment: Environment::StellarTestnetSepolia,
            stellar_passphrase: templar_oft_bridge_cli::environment::STELLAR_TESTNET_PASSPHRASE
                .into(),
            stellar_eid: templar_oft_bridge_cli::environment::STELLAR_TESTNET_EID,
            stellar_endpoint: templar_oft_bridge_cli::environment::STELLAR_TESTNET_ENDPOINT.into(),
            stellar_endpoint_code_hash: "a".repeat(64),
            evm_chain_id: 11_155_111,
            evm_eid: templar_oft_bridge_cli::environment::SEPOLIA_EID,
            evm_endpoint: templar_oft_bridge_cli::environment::SEPOLIA_ENDPOINT.into(),
            evm_endpoint_code_hash: "b".repeat(64),
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
        stellar_owner: "GCLQ3APIE5AS4XJUTRP5AF7ZMQAXDNGIRMIF3MIWQPF6ZPFJVNJDCN5E".into(),
        stellar_delegate: "GCLQ3APIE5AS4XJUTRP5AF7ZMQAXDNGIRMIF3MIWQPF6ZPFJVNJDCN5E".into(),
        evm_owner: "0xc61B17BF20b4B16bb70C1942CD8D9eBDe6726386".into(),
        evm_delegate: "0xc61B17BF20b4B16bb70C1942CD8D9eBDe6726386".into(),
        config: BTreeMap::new(),
    }
}

fn healthy_store() -> (tempfile::TempDir, RouteStore) {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("state");
    let (store, mut state) = RouteStore::create(&path, desired()).unwrap();
    let stellar_oft = "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV";
    let evm_oft = "0x1111111111111111111111111111111111111111";
    state
        .contracts
        .insert("stellar_oft".into(), stellar_oft.into());
    state.contracts.insert("evm_oft".into(), evm_oft.into());
    state.contracts.insert(
        format!("peer:{}", state.identity.evm_eid),
        format!(
            "0x{}",
            hex::encode(templar_oft_bridge_cli::codec::evm_address_to_bytes32(evm_oft).unwrap())
        ),
    );
    state.contracts.insert(
        format!("peer:{}", state.identity.stellar_eid),
        format!(
            "0x{}",
            hex::encode(templar_oft_bridge_cli::codec::strkey_to_bytes32(stellar_oft).unwrap())
        ),
    );
    store.save_state(&state).unwrap();
    store
        .record_opening_custody(OpeningCustodyV1 {
            schema_name: "opening_custody".into(),
            schema_version: SCHEMA_VERSION,
            stellar_ledger: 1,
            stellar_ledger_hash: "a".repeat(64),
            stellar_ledger_time_unix: 1,
            lockbox_raw: 2_000_000,
            evm_block: 1,
            evm_block_hash: format!("0x{}", "b".repeat(64)),
            evm_supply_raw: 2_000_000,
            artifact_lock_sha256: templar_oft_bridge_cli::artifacts::lock_sha256().unwrap(),
            effective_config_sha256:
                canonical_sha256(&BTreeMap::<String, serde_json::Value>::new()).unwrap(),
            zero_packet_history_proven: true,
            history_evidence_sha256: None,
        })
        .unwrap();
    (temporary, store)
}

#[test]
fn healthy_route_has_empty_findings() {
    let (_temporary, store) = healthy_store();
    assert!(health::check(store.root()).unwrap().is_empty());
    assert_eq!(
        health::command(store.root()).unwrap().result["healthy"],
        true
    );
}

#[test]
fn health_reports_config_and_ttl_risk_with_structured_codes() {
    let (_temporary, store) = healthy_store();
    let mut state = store.load_state().unwrap();
    state
        .requested_config
        .insert("send_library:stellar:40161".into(), serde_json::json!("C1"));
    state
        .requested_config
        .insert("ttl:min_instance_margin".into(), serde_json::json!("50"));
    state
        .effective_config
        .insert("ttl:current_ledger".into(), serde_json::json!("100"));
    state
        .effective_config
        .insert("ttl:instance_live_until".into(), serde_json::json!("120"));
    store.save_state(&state).unwrap();
    let Error::Health(findings) = health::command(store.root()).unwrap_err() else {
        panic!("expected structured health failure");
    };
    let codes: Vec<_> = findings
        .iter()
        .map(|finding| finding["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"config_drift"));
    assert!(codes.contains(&"ttl_instance_risk"));
}

#[test]
fn ttl_freeze_requires_exact_irreversible_acknowledgement() {
    let (_temporary, store) = healthy_store();
    let state = store.load_state().unwrap();
    assert!(matches!(
        ttl::freeze(&state, "yes"),
        Err(Error::InvalidInput(_))
    ));
    assert!(ttl::freeze(&state, ttl::FREEZE_ACKNOWLEDGEMENT).is_ok());
    assert!(ttl::set_config(&state, 10, 9, 10, 20).is_err());
    assert!(ttl::set_config(&state, 10, 10, 10, 20).is_ok());
}

#[test]
fn health_reports_artifact_lock_drift() {
    let (_temporary, store) = healthy_store();
    let mut state = store.load_state().unwrap();
    state.opening_custody.as_mut().unwrap().artifact_lock_sha256 = "0".repeat(64);
    store.save_state(&state).unwrap();
    assert!(health::check(store.root())
        .unwrap()
        .iter()
        .any(|finding| finding["code"] == "artifact_drift"));
}

#[test]
fn health_compares_typed_route_configs_to_effective_hashes() {
    let (_temporary, store) = healthy_store();
    let mut state = store.load_state().unwrap();
    let config = serde_json::json!({
        "required_dvns": ["CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB"],
        "optional_dvns": [],
        "optional_threshold": 0,
        "confirmations": 1
    });
    let typed: templar_oft_bridge_cli::layerzero::UlnConfigType3V1 =
        serde_json::from_value(config.clone()).unwrap();
    state
        .requested_config
        .insert("uln_receive_config:stellar:40161".into(), config);
    state.effective_config.insert(
        "uln_receive_config:stellar:40161".into(),
        serde_json::json!(typed.config_sha256().unwrap()),
    );
    store.save_state(&state).unwrap();
    assert!(!health::check(store.root())
        .unwrap()
        .iter()
        .any(|finding| finding["code"] == "config_drift"));
}

#[test]
fn health_reports_missing_required_role_with_specific_code() {
    let (_temporary, store) = healthy_store();
    let mut state = store.load_state().unwrap();
    state.requested_config.insert(
        "authority:stellar:role:PAUSER_ROLE".into(),
        serde_json::json!("GCLQ3APIE5AS4XJUTRP5AF7ZMQAXDNGIRMIF3MIWQPF6ZPFJVNJDCN5E"),
    );
    store.save_state(&state).unwrap();

    let findings = health::check(store.root()).unwrap();
    assert!(findings
        .iter()
        .any(|finding| finding["code"] == "authority_role_drift"));
}

#[test]
fn health_accepts_complete_containment_observation_but_rejects_incomplete_state() {
    let (_temporary, store) = healthy_store();
    let mut state = store.load_state().unwrap();
    state.effective_config.insert(
        "containment:stellar".into(),
        serde_json::json!({"snapshot_sha256": "a".repeat(64), "status": "confirmed"}),
    );
    store.save_state(&state).unwrap();
    assert!(health::check(store.root())
        .unwrap()
        .iter()
        .any(|finding| finding["code"] == "containment_state_incomplete"));

    let mut state = store.load_state().unwrap();
    state.effective_config.insert(
        format!("containment:snapshot:{}", "a".repeat(64)),
        serde_json::json!({"direction": "stellar_to_evm"}),
    );
    store.save_state(&state).unwrap();
    assert!(!health::check(store.root())
        .unwrap()
        .iter()
        .any(|finding| finding["code"] == "containment_state_incomplete"));
}
