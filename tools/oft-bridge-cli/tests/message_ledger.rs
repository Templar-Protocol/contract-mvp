// Tests fail by panicking; Result handling in assertions is noise.
#![allow(clippy::expect_used)]

mod common;

use std::{collections::BTreeMap, fs};

use templar_oft_bridge_cli::{
    domain::{
        AssetKind, AssetPolicyV1, ChainIdentityV1, DesiredRouteV1, Direction, Environment,
        MessageRecordV1, MessageStageV1, MessageStatusEventV1,
    },
    state::RouteStore,
};

fn desired() -> DesiredRouteV1 {
    DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: 1,
        route_id: "route-ledger".into(),
        identity: ChainIdentityV1 {
            environment: Environment::StellarTestnetSepolia,
            stellar_passphrase: "Test SDF Network ; September 2015".into(),
            stellar_eid: 40_600,
            stellar_endpoint: "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV".into(),
            stellar_endpoint_code_hash: "01".into(),
            evm_chain_id: 11_155_111,
            evm_eid: 40_161,
            evm_endpoint: "0x6EDCE65403992e310A62460808c4b910D972f10f".into(),
            evm_endpoint_code_hash: "02".into(),
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

fn store() -> (tempfile::TempDir, RouteStore) {
    let directory = tempfile::tempdir().expect("tempdir");
    let (store, _) =
        RouteStore::create(&directory.path().join("route"), desired()).expect("create route");
    common::seed_ofts(&store);
    (directory, store)
}

#[test]
fn appends_and_folds_status_history() {
    let (_directory, store) = store();
    let identity = record("g1", "1").identity();
    store.append_message(record("g1", "1")).expect("append");
    store
        .append_message_event(
            &identity,
            MessageStatusEventV1 {
                stage: MessageStageV1::ForwardLocked,
                observed_at_unix: 2,
                evidence_sha256: "ev-2".into(),
            },
        )
        .expect("event");
    store
        .append_message_event(&identity, reobserved_event())
        .expect("reobserved");
    for (stage, observed_at_unix) in [
        (MessageStageV1::ForwardVerified, 4),
        (MessageStageV1::ForwardCommitted, 5),
        (MessageStageV1::ForwardMinted, 6),
    ] {
        store
            .append_message_event(
                &identity,
                MessageStatusEventV1 {
                    stage,
                    observed_at_unix,
                    evidence_sha256: format!("ev-{observed_at_unix}"),
                },
            )
            .expect("progress after reobservation");
    }
    let messages = store.load_messages().expect("load");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].status_events.len(), 6);
}

fn reobserved_event() -> MessageStatusEventV1 {
    MessageStatusEventV1 {
        stage: MessageStageV1::Reobserved,
        observed_at_unix: 3,
        evidence_sha256: "ev-3".into(),
    }
}

#[test]
fn non_monotonic_transition_is_rejected() {
    let (_directory, store) = store();
    let identity = record("g1", "1").identity();
    store.append_message(record("g1", "1")).expect("append");
    let error = store
        .append_message_event(
            &identity,
            MessageStatusEventV1 {
                stage: MessageStageV1::ForwardMinted,
                observed_at_unix: 2,
                evidence_sha256: "ev-2".into(),
            },
        )
        .expect_err("skipped custody stages");
    assert!(matches!(
        error,
        templar_oft_bridge_cli::error::Error::InvalidInput(_)
    ));
}

#[test]
fn tampered_message_log_is_detected_on_open() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create route");
    common::seed_ofts(&store);
    store.append_message(record("g1", "1")).expect("append");
    drop(store);
    let log = fs::read_to_string(root.join("messages.jsonl")).expect("read");
    let tampered = log.replace("forward_source_accepted", "reverse_unlocked");
    fs::write(root.join("messages.jsonl"), tampered).expect("write");
    let error = RouteStore::open(&root).expect_err("tamper");
    assert!(matches!(
        error,
        templar_oft_bridge_cli::error::Error::Custody(_)
    ));
}

#[test]
fn append_requires_initial_observed_event() {
    let (_directory, store) = store();
    let mut partial = record("g9", "9");
    partial.status_events.clear();
    let error = store.append_message(partial).expect_err("empty history");
    assert!(matches!(
        error,
        templar_oft_bridge_cli::error::Error::InvalidInput(_)
    ));
}

#[test]
fn message_records_carry_no_secret_material() {
    let (_directory, store) = store();
    store.append_message(record("g1", "1")).expect("append");
    let raw = fs::read_to_string(store.root().join("messages.jsonl")).expect("read log");
    assert!(!raw.contains("password"));
    assert!(!raw.contains("secret"));
}

#[test]
fn source_transaction_is_not_recorded_until_authenticated_message_append() {
    let (_directory, store) = store();
    let mut message = record("g1", "1");
    message.source_transaction = "0xAaBb".into();

    assert!(!store
        .has_message_for_source_transaction("0xaabb")
        .expect("empty ledger"));
    store.append_message(message).expect("authenticated append");
    assert!(store
        .has_message_for_source_transaction("0xAABB")
        .expect("recorded source transaction"));
}

#[test]
fn duplicate_source_nonce_or_guid_is_rejected() {
    let (_directory, store1) = store();
    store1.append_message(record("first", "1")).expect("append");
    let nonce_error = store1
        .append_message(record("different-guid", "1"))
        .expect_err("same source nonce");
    assert!(nonce_error.to_string().contains("nonce or GUID"));

    let (_directory2, store2) = store();
    store2
        .append_message(record("same-guid", "1"))
        .expect("append");
    let guid_error = store2
        .append_message(record("same-guid", "2"))
        .expect_err("same GUID");
    assert!(guid_error.to_string().contains("nonce or GUID"));
}
