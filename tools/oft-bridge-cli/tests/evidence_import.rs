// Tests fail by panicking; Result handling in assertions is noise.
#![allow(clippy::expect_used)]

mod common;

use std::{collections::BTreeMap, fs, path::Path, path::PathBuf};

use templar_oft_bridge_cli::{
    domain::{
        AssetKind, AssetPolicyV1, ChainIdentityV1, DesiredRouteV1, Direction, Environment,
        MessageRecordV1,
    },
    state::RouteStore,
};

fn desired() -> DesiredRouteV1 {
    DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: 1,
        route_id: "route-evidence".into(),
        identity: ChainIdentityV1 {
            environment: Environment::StellarTestnetSepolia,
            stellar_passphrase: "Test SDF Network ; September 2015".into(),
            stellar_eid: 40_600,
            stellar_endpoint: "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV".into(),
            stellar_endpoint_code_hash: "1".repeat(64),
            evm_chain_id: 11_155_111,
            evm_eid: 40_161,
            evm_endpoint: "0x6EDCE65403992e310A62460808c4b910D972f10f".into(),
            evm_endpoint_code_hash: "2".repeat(64),
        },
        asset: AssetPolicyV1 {
            kind: AssetKind::NativeSac,
            asset_id: "native".into(),
            local_decimals: 7,
            issuer_custodian_evidence_sha256: None,
            destination_acceptance_evidence_sha256: None,
            custody_risk_acceptance_sha256: None,
            forbidden_classic_issuer: None,
            evidence: BTreeMap::default(),
        },
        stellar_owner: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
        stellar_delegate: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
        evm_owner: "0x0000000000000000000000000000000000000001".into(),
        evm_delegate: "0x0000000000000000000000000000000000000001".into(),
        config: BTreeMap::default(),
    }
}

fn record(guid: &str, nonce: &str) -> MessageRecordV1 {
    common::message_record(Direction::StellarToEvm, nonce.parse().expect("nonce"), guid)
}

/// Byte snapshots of every durable route file, in a stable order.
fn route_bytes(root: &Path) -> Vec<Vec<u8>> {
    ["route.json", "messages.jsonl", "operations.jsonl"]
        .iter()
        .map(|name| fs::read(root.join(name)).expect("durable route file"))
        .collect()
}

fn write_bundle(directory: &Path, messages: &[MessageRecordV1], desired_sha256: &str) -> PathBuf {
    let bundle = serde_json::json!({
        "schema_name": "evidence_bundle",
        "schema_version": 1,
        "route_id": "route-evidence",
        "desired_sha256": desired_sha256,
        "observed_lockbox_raw": "1000000000",
        "normalized_evm_supply_raw": "950000000",
        "messages": messages,
    });
    let path = directory.join("bundle.json");
    fs::write(&path, serde_json::to_vec(&bundle).expect("bundle json")).expect("write bundle");
    path
}

#[test]
fn failed_multi_record_import_leaves_durable_files_byte_identical() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, state) = RouteStore::create(&root, desired()).expect("create route");
    common::seed_ofts(&store);
    // The second record duplicates the first identity. A sequential import
    // would have durably committed record one before rejecting record two.
    let messages = [record("guid-1", "1"), record("guid-1", "1")];
    let bundle = write_bundle(directory.path(), &messages, &state.desired_sha256);
    let before = route_bytes(&root);

    let error = templar_oft_bridge_cli::canary::import_evidence(&root, &bundle, true)
        .expect_err("duplicate identity must fail the import");
    assert!(
        error.to_string().contains("already recorded"),
        "unexpected error: {error}"
    );

    assert!(
        store.load_messages().expect("load messages").is_empty(),
        "failed import must not durably import any message"
    );
    let after = route_bytes(&root);
    assert_eq!(
        before, after,
        "failed import must leave every durable route file byte-identical"
    );
}

#[test]
fn valid_import_commits_batch_and_custody_keys() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, state) = RouteStore::create(&root, desired()).expect("create route");
    common::seed_ofts(&store);
    let messages = [record("guid-1", "1"), record("guid-2", "2")];
    let bundle = write_bundle(directory.path(), &messages, &state.desired_sha256);

    let data =
        templar_oft_bridge_cli::canary::import_evidence(&root, &bundle, true).expect("import");
    assert_eq!(data.result["written"], serde_json::json!(true));
    assert_eq!(data.result["verified"], serde_json::json!(true));
    assert_eq!(data.result["message_count"], serde_json::json!(2));

    let loaded = store.load_messages().expect("load messages");
    assert_eq!(loaded.len(), 2, "all batch records must be imported");
    assert_eq!(loaded[0].guid, record("guid-1", "1").guid);
    assert_eq!(loaded[1].guid, record("guid-2", "2").guid);

    let state = store.load_state().expect("load state");
    assert_eq!(
        state.effective_config["custody:observed_lockbox_raw"],
        serde_json::json!("1000000000")
    );
    assert_eq!(
        state.effective_config["custody:normalized_evm_supply_raw"],
        serde_json::json!("950000000")
    );
    assert!(state.effective_config["custody:evidence_bundle_sha256"]
        .as_str()
        .is_some_and(|digest| !digest.is_empty()));
}

#[test]
fn duplicate_import_is_rejected_without_durable_change() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, state) = RouteStore::create(&root, desired()).expect("create route");
    common::seed_ofts(&store);
    let messages = [record("guid-1", "1"), record("guid-2", "2")];
    let bundle = write_bundle(directory.path(), &messages, &state.desired_sha256);
    templar_oft_bridge_cli::canary::import_evidence(&root, &bundle, true).expect("import");
    let before = route_bytes(&root);

    let error = templar_oft_bridge_cli::canary::import_evidence(&root, &bundle, true)
        .expect_err("re-import of an occupied ledger must conflict");
    assert!(
        error.to_string().contains("empty message ledger"),
        "unexpected error: {error}"
    );
    let after = route_bytes(&root);
    assert_eq!(
        before, after,
        "a rejected duplicate import must change nothing"
    );
    assert_eq!(store.load_messages().expect("load messages").len(), 2);
}

#[test]
fn read_only_import_validates_without_writing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, state) = RouteStore::create(&root, desired()).expect("create route");
    common::seed_ofts(&store);
    let messages = [record("guid-1", "1"), record("guid-2", "2")];
    let bundle = write_bundle(directory.path(), &messages, &state.desired_sha256);
    let before = route_bytes(&root);

    let data =
        templar_oft_bridge_cli::canary::import_evidence(&root, &bundle, false).expect("validate");
    assert_eq!(data.result["written"], serde_json::json!(false));
    assert_eq!(data.result["verified"], serde_json::json!(true));
    let after = route_bytes(&root);
    assert_eq!(before, after, "read-only import must write nothing");
}

#[test]
fn retry_completes_import_after_messages_commit_but_before_state_commit() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, state) = RouteStore::create(&root, desired()).expect("create route");
    common::seed_ofts(&store);
    let messages = [record("guid-1", "1"), record("guid-2", "2")];
    let bundle = write_bundle(directory.path(), &messages, &state.desired_sha256);
    let evidence: serde_json::Value = serde_json::from_slice(&fs::read(&bundle).unwrap()).unwrap();
    let digest = templar_oft_bridge_cli::canonical_sha256(&evidence).unwrap();
    templar_oft_bridge_cli::state::write_create_new_json(
        &root.join(".evidence-import.json"),
        &serde_json::json!({
            "schema_name": "evidence_import_marker",
            "schema_version": 1,
            "bundle_sha256": digest,
            "evidence": evidence,
        }),
    )
    .unwrap();
    store.append_messages_batch(messages.to_vec()).unwrap();

    let result =
        templar_oft_bridge_cli::canary::import_evidence(&root, &bundle, true).expect("resume");
    assert_eq!(result.result["written"], true);
    assert!(!root.join(".evidence-import.json").exists());
    let state = store.load_state().unwrap();
    assert_eq!(
        state.effective_config["custody:evidence_bundle_sha256"],
        serde_json::json!(digest)
    );
    assert_eq!(store.load_messages().unwrap(), messages);
}
