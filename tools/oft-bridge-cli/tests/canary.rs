//! Canary leg quote/send live-observation, watch, recovery, and evidence
//! import tests.
mod common;
use std::collections::BTreeMap;

use templar_oft_bridge_cli::error::{Error, Result};

use std::path::{Path, PathBuf};

use templar_oft_bridge_cli::domain::{
    AssetKind, AssetPolicyV1, ChainIdentityV1, DesiredRouteV1, Direction, Environment, LegIntentV1,
    MessageStageV1, OpeningCustodyV1, OperationV1, Vm, SCHEMA_VERSION,
};
use templar_oft_bridge_cli::state::RouteStore;

const STELLAR_ACCOUNT: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const EVM_ACCOUNT: &str = "0x0000000000000000000000000000000000000001";
const EVM_DESTINATION: &str = "0x0000000000000000000000000000000000000002";
const STELLAR_DESTINATION: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

fn identity() -> ChainIdentityV1 {
    ChainIdentityV1 {
        environment: Environment::StellarTestnetSepolia,
        stellar_passphrase: "Test SDF Network ; September 2015".into(),
        stellar_eid: 40_600,
        stellar_endpoint: templar_oft_bridge_cli::environment::STELLAR_TESTNET_ENDPOINT.into(),
        stellar_endpoint_code_hash: "1".repeat(64),
        evm_chain_id: 11_155_111,
        evm_eid: 40_161,
        evm_endpoint: templar_oft_bridge_cli::environment::SEPOLIA_ENDPOINT.into(),
        evm_endpoint_code_hash: "2".repeat(64),
    }
}

fn desired(config: BTreeMap<String, serde_json::Value>) -> DesiredRouteV1 {
    DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: 1,
        route_id: "route-canary".into(),
        identity: identity(),
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
        stellar_owner: STELLAR_ACCOUNT.into(),
        stellar_delegate: STELLAR_ACCOUNT.into(),
        evm_owner: EVM_ACCOUNT.into(),
        evm_delegate: EVM_ACCOUNT.into(),
        config,
    }
}

/// Recorded canary fee/rate/option evidence for both legs. The forward leg
/// originates on Stellar, the reverse on EVM; each source sender is bound to
/// its own VM.
fn canary_config() -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        ("canary:max_amount_raw".into(), serde_json::json!("1000000")),
        ("canary:quote_ttl_seconds".into(), serde_json::json!("3600")),
        (
            "canary:sender:stellar_to_evm".into(),
            serde_json::json!(STELLAR_ACCOUNT),
        ),
        (
            "canary:sender:evm_to_stellar".into(),
            serde_json::json!(EVM_ACCOUNT),
        ),
        (
            "canary:max_native_fee_raw:stellar_to_evm".into(),
            serde_json::json!("1000"),
        ),
        (
            "canary:quoted_native_fee_raw:stellar_to_evm".into(),
            serde_json::json!("500"),
        ),
        (
            "canary:max_native_fee_raw:evm_to_stellar".into(),
            serde_json::json!("1000"),
        ),
        (
            "canary:quoted_native_fee_raw:evm_to_stellar".into(),
            serde_json::json!("500"),
        ),
        (
            "canary:extra_options:stellar_to_evm".into(),
            serde_json::json!("0003"),
        ),
        (
            "canary:extra_options:evm_to_stellar".into(),
            serde_json::json!("0003"),
        ),
        ("fee_bps:stellar_to_evm".into(), serde_json::json!("25")),
        ("fee_bps:evm_to_stellar".into(), serde_json::json!("10")),
        (
            "canary:finality_policy".into(),
            serde_json::json!("confirmed"),
        ),
        (
            "canary:max_outstanding_obligation_raw".into(),
            serde_json::json!("10000000"),
        ),
        (
            "canary:stellar_resource_fee_ceiling_raw:stellar_to_evm".into(),
            serde_json::json!("100000"),
        ),
        (
            "canary:evm_gas_limit:evm_to_stellar".into(),
            serde_json::json!("300000"),
        ),
        (
            "canary:evm_max_fee_per_gas_wei:evm_to_stellar".into(),
            serde_json::json!("50000000000"),
        ),
        (
            "canary:evm_max_priority_fee_per_gas_wei:evm_to_stellar".into(),
            serde_json::json!("1000000000"),
        ),
    ])
}

fn opening_custody() -> OpeningCustodyV1 {
    OpeningCustodyV1 {
        schema_name: "opening_custody".into(),
        schema_version: SCHEMA_VERSION,
        stellar_ledger: 100,
        stellar_ledger_hash: "aa".repeat(32),
        stellar_ledger_time_unix: 1_700_000_000,
        lockbox_raw: 2_000_000,
        evm_block: 200,
        evm_block_hash: "bb".repeat(32),
        evm_supply_raw: 0,
        artifact_lock_sha256: "cc".repeat(32),
        effective_config_sha256: "dd".repeat(32),
        zero_packet_history_proven: true,
        history_evidence_sha256: None,
    }
}

/// Adopted, fully converged testnet route with recorded canary evidence and
/// a finalized opening-custody baseline. No packet history unless a test
/// appends one.
fn route() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired(canary_config())).expect("route");
    let mut state = store.load_state().expect("state");
    state.effective_config = state.requested_config.clone();
    store.save_state(&state).expect("save");
    common::seed_ofts(&store);
    store
        .record_opening_custody(opening_custody())
        .expect("custody");
    (directory, root)
}

fn quote_intent(
    root: &Path,
    direction: Direction,
    amount: u128,
    to: &str,
) -> (tempfile::TempDir, PathBuf) {
    let out_dir = tempfile::tempdir().expect("out dir");
    let out = out_dir.path().join("intent.json");
    templar_oft_bridge_cli::canary::quote(root, direction, amount, to, &out).expect("quote");
    (out_dir, out)
}

fn rewritten(intent_dir: &Path, name: &str, intent: &LegIntentV1) -> PathBuf {
    let path = intent_dir.join(name);
    templar_oft_bridge_cli::state::write_create_new_json(&path, intent).expect("rewrite");
    path
}

#[test]
fn quote_forward_is_canonical_and_bound_to_stellar_sender() {
    let (_directory, root) = route();
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let intent: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).unwrap();
    assert_eq!(intent.direction, Direction::StellarToEvm);
    assert_eq!(intent.sender, STELLAR_ACCOUNT);
    assert_eq!(intent.refund_address, STELLAR_ACCOUNT);
    assert_eq!(intent.destination_eid, 40_161);
    assert_eq!(intent.amount_raw, "100000");
    assert_eq!(intent.minimum_received_raw, "99750");
    assert_eq!(intent.native_fee_raw, "500");
    assert_eq!(intent.maximum_native_fee_raw, "1000");
    assert_eq!(intent.extra_options, "0003");
    assert_eq!(intent.config_snapshot_sha256.len(), 64);
    assert_eq!(intent.custody_snapshot_sha256.len(), 64);
    assert!(intent.expires_at_unix > 1_700_000_000);
}

#[test]
fn quote_reverse_is_canonical_and_bound_to_evm_sender() {
    let (_directory, root) = route();
    let (_out_dir, out) =
        quote_intent(&root, Direction::EvmToStellar, 200_000, STELLAR_DESTINATION);
    let intent: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).unwrap();
    assert_eq!(intent.direction, Direction::EvmToStellar);
    assert_eq!(intent.sender, EVM_ACCOUNT);
    assert_eq!(intent.refund_address, EVM_ACCOUNT);
    assert_eq!(intent.destination_eid, 40_600);
    assert_eq!(intent.minimum_received_raw, "199800");
}

#[test]
fn quote_refuses_unconverged_route() {
    let (_directory, root) = route();
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    state.effective_config.remove("canary:max_amount_raw");
    store.save_state(&state).expect("save");
    let out_dir = tempfile::tempdir().expect("out dir");
    let out = out_dir.path().join("intent.json");
    let error = templar_oft_bridge_cli::canary::quote(
        &root,
        Direction::StellarToEvm,
        100,
        EVM_DESTINATION,
        &out,
    )
    .unwrap_err();
    assert!(matches!(error, Error::Conflict(_)));
    assert!(error.to_string().contains("not fully converged"));
}

#[test]
fn quote_refuses_missing_opening_custody_baseline() {
    let (_directory, root) = route();
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    state.opening_custody = None;
    store.save_state(&state).expect("save");
    let out_dir = tempfile::tempdir().expect("out dir");
    let out = out_dir.path().join("intent.json");
    let error = templar_oft_bridge_cli::canary::quote(
        &root,
        Direction::StellarToEvm,
        100,
        EVM_DESTINATION,
        &out,
    )
    .unwrap_err();
    assert!(
        matches!(error, Error::Custody(_))
            && error
                .to_string()
                .contains("opening custody is not finalized")
    );
}

#[test]
fn quote_refuses_zero_or_uncapped_amounts() {
    let (_directory, root) = route();
    let out_dir = tempfile::tempdir().expect("out dir");
    let zero = out_dir.path().join("zero.json");
    let zero_error = templar_oft_bridge_cli::canary::quote(
        &root,
        Direction::StellarToEvm,
        0,
        EVM_DESTINATION,
        &zero,
    )
    .unwrap_err();
    assert!(matches!(zero_error, Error::Policy(_)));
    let over = out_dir.path().join("over.json");
    let over_error = templar_oft_bridge_cli::canary::quote(
        &root,
        Direction::StellarToEvm,
        3_000_000,
        EVM_DESTINATION,
        &over,
    )
    .unwrap_err();
    assert!(matches!(over_error, Error::Policy(_)));
}

#[test]
fn quote_refuses_missing_fee_and_fee_cap_evidence() {
    let (_directory, root) = route();
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    state.requested_config.remove("fee_bps:stellar_to_evm");
    state.effective_config.remove("fee_bps:stellar_to_evm");
    store.save_state(&state).expect("save");
    let out_dir = tempfile::tempdir().expect("out dir");
    let out = out_dir.path().join("intent.json");
    let missing_fee = templar_oft_bridge_cli::canary::quote(
        &root,
        Direction::StellarToEvm,
        100,
        EVM_DESTINATION,
        &out,
    )
    .unwrap_err();
    assert!(
        matches!(missing_fee, Error::Custody(_))
            && missing_fee.to_string().contains("fee_bps:stellar_to_evm")
    );

    state
        .requested_config
        .insert("fee_bps:stellar_to_evm".into(), serde_json::json!("25"));
    state
        .effective_config
        .insert("fee_bps:stellar_to_evm".into(), serde_json::json!("25"));
    state
        .requested_config
        .remove("canary:max_native_fee_raw:stellar_to_evm");
    state
        .effective_config
        .remove("canary:max_native_fee_raw:stellar_to_evm");
    store.save_state(&state).expect("save");
    let missing_cap = templar_oft_bridge_cli::canary::quote(
        &root,
        Direction::StellarToEvm,
        100,
        EVM_DESTINATION,
        &out,
    )
    .unwrap_err();
    assert!(
        matches!(missing_cap, Error::Custody(_))
            && missing_cap
                .to_string()
                .contains("canary:max_native_fee_raw:stellar_to_evm")
    );
}

#[test]
fn quote_refuses_recorded_fee_above_ceiling_and_bad_sender_slot() {
    let (_directory, root) = route();
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    state.requested_config.insert(
        "canary:quoted_native_fee_raw:stellar_to_evm".into(),
        serde_json::json!("5000"),
    );
    state.effective_config.insert(
        "canary:quoted_native_fee_raw:stellar_to_evm".into(),
        serde_json::json!("5000"),
    );
    store.save_state(&state).expect("save");
    let out_dir = tempfile::tempdir().expect("out dir");
    let out = out_dir.path().join("intent.json");
    let over_ceiling = templar_oft_bridge_cli::canary::quote(
        &root,
        Direction::StellarToEvm,
        100,
        EVM_DESTINATION,
        &out,
    )
    .unwrap_err();
    assert!(matches!(over_ceiling, Error::Custody(_)));

    state.requested_config.insert(
        "canary:quoted_native_fee_raw:stellar_to_evm".into(),
        serde_json::json!("500"),
    );
    state.effective_config.insert(
        "canary:quoted_native_fee_raw:stellar_to_evm".into(),
        serde_json::json!("500"),
    );
    state.requested_config.insert(
        "canary:sender:stellar_to_evm".into(),
        serde_json::json!(EVM_ACCOUNT),
    );
    state.effective_config.insert(
        "canary:sender:stellar_to_evm".into(),
        serde_json::json!(EVM_ACCOUNT),
    );
    store.save_state(&state).expect("save");
    let wrong_vm_sender = templar_oft_bridge_cli::canary::quote(
        &root,
        Direction::StellarToEvm,
        100,
        EVM_DESTINATION,
        &out,
    )
    .unwrap_err();
    assert!(matches!(wrong_vm_sender, Error::InvalidInput(_)));
}

#[test]
fn quote_binds_recorded_refund_address() {
    let (_directory, root) = route();
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    state.requested_config.insert(
        "canary:refund_address:stellar_to_evm".into(),
        serde_json::json!(common::STELLAR_OFT),
    );
    state.effective_config.insert(
        "canary:refund_address:stellar_to_evm".into(),
        serde_json::json!(common::STELLAR_OFT),
    );
    store.save_state(&state).expect("save");
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let intent: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).unwrap();
    assert_eq!(intent.sender, STELLAR_ACCOUNT);
    assert_eq!(intent.refund_address, common::STELLAR_OFT);
}

#[test]
fn send_selects_stellar_source_for_forward_and_accepts_without_flag_when_clear() {
    let (_directory, root) = route();
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let operation = templar_oft_bridge_cli::canary::send_operation(&root, &out, false)
        .expect("send without obligations");
    match operation {
        OperationV1::SendLeg { vm, intent } => {
            assert_eq!(vm, Vm::Stellar);
            assert_eq!(intent.sender, STELLAR_ACCOUNT);
            assert_eq!(intent.destination_eid, 40_161);
        }
        other => panic!("unexpected operation {other:?}"),
    }
}

#[test]
fn send_selects_evm_source_for_reverse() {
    let (_directory, root) = route();
    let (_out_dir, out) =
        quote_intent(&root, Direction::EvmToStellar, 100_000, STELLAR_DESTINATION);
    let operation =
        templar_oft_bridge_cli::canary::send_operation(&root, &out, false).expect("reverse send");
    match operation {
        OperationV1::SendLeg { vm, intent } => {
            assert_eq!(vm, Vm::Evm);
            assert_eq!(intent.sender, EVM_ACCOUNT);
            assert_eq!(intent.destination_eid, 40_600);
        }
        other => panic!("unexpected operation {other:?}"),
    }
}

#[test]
fn send_rejects_stale_config_snapshot_and_fee_drift() {
    let (_directory, root) = route();
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    state
        .requested_config
        .insert("fee_bps:stellar_to_evm".into(), serde_json::json!("30"));
    state
        .effective_config
        .insert("fee_bps:stellar_to_evm".into(), serde_json::json!("30"));
    store.save_state(&state).expect("save");
    let error = templar_oft_bridge_cli::canary::send_operation(&root, &out, false)
        .expect_err("fee drift must reject");
    assert!(matches!(error, Error::Conflict(_)));
    assert!(error.to_string().contains("minimum_received_raw"));
}

#[test]
fn send_rejects_stale_custody_baseline() {
    let (_directory, root) = route();
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    let mut opening = state.opening_custody.clone().expect("opening");
    opening.lockbox_raw += 1;
    state.opening_custody = Some(opening);
    store.save_state(&state).expect("save");
    let error = templar_oft_bridge_cli::canary::send_operation(&root, &out, false)
        .expect_err("custody drift must reject");
    assert!(matches!(error, Error::Conflict(_)));
    assert!(error.to_string().contains("custody_snapshot_sha256"));
}

#[test]
fn send_rejects_expired_and_ttl_shrunk_intents() {
    let (_directory, root) = route();
    let (out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let mut expired_intent: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).unwrap();
    expired_intent.expires_at_unix = 1;
    let expired = rewritten(out_dir.path(), "expired.json", &expired_intent);
    let expired_error = templar_oft_bridge_cli::canary::send_operation(&root, &expired, false)
        .expect_err("expired intent must reject");
    assert!(
        matches!(expired_error, Error::Conflict(_))
            && expired_error.to_string().contains("expired")
    );

    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    state
        .requested_config
        .insert("canary:quote_ttl_seconds".into(), serde_json::json!("60"));
    state
        .effective_config
        .insert("canary:quote_ttl_seconds".into(), serde_json::json!("60"));
    store.save_state(&state).expect("save");
    let ttl_error = templar_oft_bridge_cli::canary::send_operation(&root, &out, false)
        .expect_err("TTL shrink must reject");
    assert!(
        matches!(ttl_error, Error::Conflict(_)) && ttl_error.to_string().contains("TTL ceiling")
    );
}

#[test]
fn send_rejects_wrong_sender_and_receiver() {
    let (_directory, root) = route();
    let (out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let mut wrong_sender: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).unwrap();
    wrong_sender.sender = common::STELLAR_OFT.to_string();
    let sender_path = rewritten(out_dir.path(), "wrong-sender.json", &wrong_sender);
    let sender_error = templar_oft_bridge_cli::canary::send_operation(&root, &sender_path, false)
        .expect_err("wrong sender must reject");
    assert!(matches!(sender_error, Error::Conflict(_)));
    assert!(sender_error.to_string().contains("sender"));

    let mut wrong_receiver: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).unwrap();
    wrong_receiver.to = "not-an-evm-address".to_string();
    let receiver_path = rewritten(out_dir.path(), "wrong-receiver.json", &wrong_receiver);
    let receiver_error =
        templar_oft_bridge_cli::canary::send_operation(&root, &receiver_path, false)
            .expect_err("invalid receiver must reject");
    assert!(matches!(receiver_error, Error::InvalidInput(_)));
}

#[test]
fn send_rejects_route_drift_bindings() {
    let (_directory, root) = route();
    let (out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let mut wrong_route: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).unwrap();
    wrong_route.route_id = "other-route".to_string();
    let route_path = rewritten(out_dir.path(), "wrong-route.json", &wrong_route);
    let route_error = templar_oft_bridge_cli::canary::send_operation(&root, &route_path, false)
        .expect_err("route drift must reject");
    assert!(
        matches!(route_error, Error::Conflict(_))
            && route_error.to_string().contains("does not bind")
    );

    let mut wrong_desired: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).unwrap();
    wrong_desired.desired_sha256 = "f".repeat(64);
    let desired_path = rewritten(out_dir.path(), "wrong-desired.json", &wrong_desired);
    let desired_error = templar_oft_bridge_cli::canary::send_operation(&root, &desired_path, false)
        .expect_err("desired digest drift must reject");
    assert!(
        matches!(desired_error, Error::Conflict(_))
            && desired_error.to_string().contains("does not bind")
    );
}

#[test]
fn send_rejects_uncapped_amount_after_quote() {
    let (_directory, root) = route();
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    state
        .requested_config
        .insert("canary:max_amount_raw".into(), serde_json::json!("50000"));
    state
        .effective_config
        .insert("canary:max_amount_raw".into(), serde_json::json!("50000"));
    store.save_state(&state).expect("save");
    let error = templar_oft_bridge_cli::canary::send_operation(&root, &out, false)
        .expect_err("cap breach must reject");
    assert!(matches!(error, Error::Policy(_)));
}

fn append_stuck(root: &Path, direction: Direction, seed: &str, outstanding_raw: &str) {
    let mut record = common::message_record(direction, 1, seed);
    record.status_events[0].stage = match direction {
        Direction::StellarToEvm => MessageStageV1::ForwardLocked,
        Direction::EvmToStellar => MessageStageV1::ReverseBurned,
    };
    match direction {
        Direction::StellarToEvm => record.net_locked_raw = outstanding_raw.into(),
        Direction::EvmToStellar => record.burned_raw = outstanding_raw.into(),
    }
    let store = RouteStore::open(root).expect("open");
    store.append_message(record).expect("append stuck");
}

#[test]
fn forward_send_refused_without_flag_while_obligation_outstanding() {
    let (_directory, root) = route();
    append_stuck(&root, Direction::StellarToEvm, "stuck-fwd", "100000");
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let error = templar_oft_bridge_cli::canary::send_operation(&root, &out, false)
        .expect_err("stuck obligation must gate the send");
    assert!(
        matches!(error, Error::Policy(_))
            && error.to_string().contains("--allow-additional-obligation")
    );
}

#[test]
fn reverse_send_refused_without_flag_while_obligation_outstanding() {
    let (_directory, root) = route();
    append_stuck(&root, Direction::EvmToStellar, "stuck-rev", "100000");
    let (_out_dir, out) =
        quote_intent(&root, Direction::EvmToStellar, 100_000, STELLAR_DESTINATION);
    let error = templar_oft_bridge_cli::canary::send_operation(&root, &out, false)
        .expect_err("stuck reverse obligation must gate the send");
    assert!(
        matches!(error, Error::Policy(_))
            && error.to_string().contains("--allow-additional-obligation")
    );
}

#[test]
fn additional_obligation_allowed_only_within_recorded_cap() {
    let (_directory, root) = route();
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    for config in [&mut state.requested_config, &mut state.effective_config] {
        config.insert(
            "canary:max_outstanding_obligation_raw".into(),
            serde_json::json!("1000000"),
        );
    }
    store.save_state(&state).expect("save");
    append_stuck(&root, Direction::StellarToEvm, "stuck-capped", "100000");
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let operation = templar_oft_bridge_cli::canary::send_operation(&root, &out, true)
        .expect("within cap must send");
    assert!(matches!(
        operation,
        OperationV1::SendLeg {
            vm: Vm::Stellar,
            ..
        }
    ));

    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    for config in [&mut state.requested_config, &mut state.effective_config] {
        config.insert(
            "canary:max_outstanding_obligation_raw".into(),
            serde_json::json!("150000"),
        );
    }
    store.save_state(&state).expect("save");
    let (_over_dir, over_intent) =
        quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let over = templar_oft_bridge_cli::canary::send_operation(&root, &over_intent, true)
        .expect_err("resulting obligation over cap must reject");
    assert!(
        matches!(over, Error::Policy(_)) && over.to_string().contains("exceeds the recorded cap")
    );
}

#[test]
fn first_obligation_must_fit_recorded_cap() {
    let (_directory, root) = route();
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    for config in [&mut state.requested_config, &mut state.effective_config] {
        config.insert(
            "canary:max_outstanding_obligation_raw".into(),
            serde_json::json!("50000"),
        );
    }
    store.save_state(&state).expect("save");
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let error = templar_oft_bridge_cli::canary::send_operation(&root, &out, false)
        .expect_err("first obligation above cap must reject");
    assert!(matches!(error, Error::Policy(_)));
    assert!(error.to_string().contains("exceeds the recorded cap"));
}

#[test]
fn quote_refuses_missing_obligation_cap_evidence() {
    let (_directory, root) = route();
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    for config in [&mut state.requested_config, &mut state.effective_config] {
        config.remove("canary:max_outstanding_obligation_raw");
    }
    store.save_state(&state).expect("save");
    let out_dir = tempfile::tempdir().expect("out dir");
    let out = out_dir.path().join("intent.json");
    let error = templar_oft_bridge_cli::canary::quote(
        &root,
        Direction::StellarToEvm,
        100_000,
        EVM_DESTINATION,
        &out,
    )
    .expect_err("missing cap evidence must be refused at quote, not fabricated");
    assert!(
        matches!(error, Error::Custody(_))
            && error
                .to_string()
                .contains("canary:max_outstanding_obligation_raw")
    );
}

#[test]
fn settled_ledger_history_does_not_gate_a_new_send() {
    let (_directory, root) = route();
    let mut settled = common::message_record(Direction::StellarToEvm, 1, "settled");
    settled.status_events[0].stage = MessageStageV1::ForwardMinted;
    let store = RouteStore::open(&root).expect("open");
    store.append_message(settled).expect("append settled");
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    for config in [&mut state.requested_config, &mut state.effective_config] {
        config.insert(
            "canary:max_outstanding_obligation_raw".into(),
            serde_json::json!("1000000"),
        );
    }
    store.save_state(&state).expect("save");
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    templar_oft_bridge_cli::canary::send_operation(&root, &out, false)
        .expect("settled history must not gate");
}

// =====================================================================
// Step 8 live observation contract. A quote binds read-only observations
// (source height, source sequence/nonce, pre-send balance/lockbox/supply)
// without reserving a sequence/nonce or constructing a signable
// transaction; send re-reads the same observations and refuses any drift
// per VM before signing.
// =====================================================================

const FAKE_STELLAR_TOKEN: &str = "CDLZJTQFJXODQZOEBKKN5Y3L4V5YZ5CPUYDQ3EDZWJMLC3LRLR7K4JMC";
const FAKE_EVM_TOKEN: &str = "0x2222222222222222222222222222222222222222";

fn live_route() -> (tempfile::TempDir, PathBuf) {
    let (_directory, root) = route();
    let store = RouteStore::open(&root).expect("open");
    let mut state = store.load_state().expect("state");
    state
        .contracts
        .insert("stellar_token".into(), FAKE_STELLAR_TOKEN.into());
    state
        .contracts
        .insert("evm_token".into(), FAKE_EVM_TOKEN.into());
    state
        .contracts
        .insert("stellar_owner".into(), STELLAR_ACCOUNT.into());
    store.save_state(&state).expect("save");
    (_directory, root)
}

#[derive(Clone)]
struct FakeObservedStellar {
    ledger: u32,
    sequence: String,
    balances: BTreeMap<(String, String), String>,
}

impl templar_oft_bridge_cli::stellar::StellarChain for FakeObservedStellar {
    fn network_passphrase(&self) -> Result<String> {
        Ok("Test SDF Network ; September 2015".into())
    }
    fn endpoint_eid(&self, _endpoint: &str, _source: &str) -> Result<u32> {
        Ok(40_600)
    }
    fn account_sequence(&self, _account: &str) -> Result<String> {
        Ok(self.sequence.clone())
    }
    fn invoke_view(
        &self,
        _contract: &str,
        _function: &str,
        _args_xdr_hex: &[String],
        _source: &str,
    ) -> Result<stellar_baselib::xdr::ScVal> {
        Ok(stellar_baselib::xdr::ScVal::U32(0))
    }
    fn token_balance(&self, token: &str, address: &str, _source: &str) -> Result<String> {
        self.balances
            .get(&(token.to_string(), address.to_string()))
            .cloned()
            .ok_or_else(|| Error::Custody(format!("fake stellar balance for {token} {address}")))
    }
    fn account_signers(&self, _account: &str) -> Result<BTreeMap<String, u32>> {
        Ok(BTreeMap::from([(STELLAR_ACCOUNT.to_string(), 1)]))
    }
    fn account_threshold(&self, _account: &str, _level: &str) -> Result<u32> {
        Ok(1)
    }
    fn latest_ledger(&self) -> Result<u32> {
        Ok(self.ledger)
    }
    fn simulate_transaction(
        &self,
        _state: &templar_oft_bridge_cli::domain::RouteStateV1,
        _operation: &OperationV1,
        _source: &str,
        _sequence: &str,
        _min_ledger: u32,
        _max_ledger: u32,
    ) -> Result<templar_oft_bridge_cli::stellar::StellarSimulationV1> {
        panic!("quote/send re-read must not construct a signable Stellar transaction")
    }
    fn submit_transaction(&self, _signed_envelope_xdr: &str) -> Result<String> {
        Ok("tx".into())
    }
    fn transaction_status(
        &self,
        _transaction_hash: &str,
    ) -> Result<templar_oft_bridge_cli::stellar::StellarTransactionStatusV1> {
        Ok(
            templar_oft_bridge_cli::stellar::StellarTransactionStatusV1 {
                status: "in_flight".into(),
                ledger: None,
                envelope_xdr: None,
                contract_events: Vec::new(),
            },
        )
    }
}

fn u256_word(value: &str) -> Vec<u8> {
    alloy::primitives::U256::from_str_radix(value, 10)
        .expect("decimal word")
        .to_be_bytes::<32>()
        .to_vec()
}

fn selector(signature: &str) -> Vec<u8> {
    alloy::primitives::keccak256(signature.as_bytes())[..4].to_vec()
}

struct FakeObservedEvm {
    block: u64,
    nonce: u64,
    words: BTreeMap<Vec<u8>, Vec<u8>>,
}

#[async_trait::async_trait]
impl templar_oft_bridge_cli::evm::EvmChain for FakeObservedEvm {
    async fn chain_id(&self) -> Result<u64> {
        Ok(11_155_111)
    }
    async fn code(&self, _address: alloy::primitives::Address) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }
    async fn call(&self, _to: alloy::primitives::Address, calldata: Vec<u8>) -> Result<Vec<u8>> {
        let key = calldata[..4].to_vec();
        self.words
            .get(&key)
            .cloned()
            .ok_or_else(|| Error::Custody("fake evm call word".into()))
    }
    async fn endpoint_eid(&self, _endpoint: &str) -> Result<u32> {
        Ok(40_161)
    }
    async fn account_nonce(&self, _address: alloy::primitives::Address) -> Result<u64> {
        Ok(self.nonce)
    }
    async fn safe_state(&self, _safe: alloy::primitives::Address) -> Result<Option<(u32, String)>> {
        Ok(None)
    }
    async fn latest_block(&self) -> Result<u64> {
        Ok(self.block)
    }
    async fn estimate_transaction(
        &self,
        _from: alloy::primitives::Address,
        _to: alloy::primitives::Address,
        _value: alloy::primitives::U256,
        _calldata: Vec<u8>,
    ) -> Result<templar_oft_bridge_cli::evm::EvmSimulationV1> {
        panic!("quote/send re-read must not estimate a signable EVM transaction")
    }
    async fn send_raw_transaction(&self, _encoded: &[u8]) -> Result<String> {
        Ok("0xtx".into())
    }
    async fn transaction_receipt(
        &self,
        _transaction_hash: &str,
    ) -> Result<Option<templar_oft_bridge_cli::evm::EvmReceiptV1>> {
        Ok(None)
    }
    async fn transaction_by_hash(
        &self,
        _transaction_hash: &str,
    ) -> Result<Option<serde_json::Value>> {
        Ok(None)
    }
}

fn obs_stellar(ledger: u32, sequence: &str, sender: &str, lockbox: &str) -> FakeObservedStellar {
    FakeObservedStellar {
        ledger,
        sequence: sequence.into(),
        balances: BTreeMap::from([
            (
                (FAKE_STELLAR_TOKEN.to_string(), STELLAR_ACCOUNT.to_string()),
                sender.to_string(),
            ),
            (
                (
                    FAKE_STELLAR_TOKEN.to_string(),
                    common::STELLAR_OFT.to_string(),
                ),
                lockbox.to_string(),
            ),
        ]),
    }
}

fn obs_evm(block: u64, nonce: u64, supply: &str) -> FakeObservedEvm {
    FakeObservedEvm {
        block,
        nonce,
        words: BTreeMap::from([(selector("totalSupply()"), u256_word(supply))]),
    }
}

fn quote_live_intent(
    root: &Path,
    direction: Direction,
    amount: u128,
    to: &str,
    stellar: &FakeObservedStellar,
    evm: &FakeObservedEvm,
) -> (tempfile::TempDir, PathBuf) {
    let out_dir = tempfile::tempdir().expect("out dir");
    let out = out_dir.path().join("intent.json");
    templar_oft_bridge_cli::canary::quote_live(root, direction, amount, to, &out, stellar, evm)
        .expect("live quote");
    (out_dir, out)
}

#[test]
fn quote_live_records_observations_without_plan_or_nonce_reservation() {
    let (_directory, root) = live_route();
    let stellar = obs_stellar(4_310, "41", "900000", "2000000");
    let evm = obs_evm(420, 7, "2000000");
    let (_out_dir, out) = quote_live_intent(
        &root,
        Direction::StellarToEvm,
        100_000,
        EVM_DESTINATION,
        &stellar,
        &evm,
    );
    let intent: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).expect("intent");
    assert_eq!(intent.quote_source_ledger, Some(4_310));
    assert_eq!(intent.quote_source_block, None);
    assert_eq!(intent.observed_sequence_nonce.as_deref(), Some("41"));
    let snapshot = intent.pre_send_snapshot.expect("pre-send snapshot");
    assert_eq!(snapshot.source_balance_raw, "900000");
    assert_eq!(snapshot.lockbox_raw, "2000000");
    assert_eq!(snapshot.evm_supply_raw, "2000000");
    assert!(matches!(
        intent.fee_ceiling,
        Some(templar_oft_bridge_cli::domain::LegFeeCeilingV1::Stellar { .. })
    ));
    assert_eq!(
        intent.finality_policy,
        Some(templar_oft_bridge_cli::domain::LegFinalityPolicyV1::Confirmed)
    );
    assert_eq!(intent.peer_snapshot_sha256.len(), 64);
    let obligation = intent
        .additional_obligation
        .expect("additional-obligation policy");
    assert_eq!(obligation.outstanding_raw, "0");
}

#[test]
fn evm_send_requires_successor_block_and_canonical_receipt() {
    let (_directory, root) = live_route();
    let (_out_dir, out) =
        quote_intent(&root, Direction::EvmToStellar, 100_000, STELLAR_DESTINATION);
    let intent: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).expect("intent");
    let receipt = templar_oft_bridge_cli::evm::EvmReceiptV1 {
        transaction_hash: "0x1234".into(),
        block_number: Some(420),
        succeeded: Some(true),
        logs: Vec::new(),
        raw: serde_json::json!({"blockHash": "0xaaaa"}),
    };

    assert!(!templar_oft_bridge_cli::canary::evm_source_finalized(
        &intent,
        &receipt,
        420,
        Some(&receipt),
    )
    .expect("same-block receipt"));
    assert!(templar_oft_bridge_cli::canary::evm_source_finalized(
        &intent,
        &receipt,
        421,
        Some(&receipt),
    )
    .expect("canonical successor-block receipt"));

    let mut reorged = receipt.clone();
    reorged.raw = serde_json::json!({"blockHash": "0xbbbb"});
    assert!(!templar_oft_bridge_cli::canary::evm_source_finalized(
        &intent,
        &receipt,
        421,
        Some(&reorged),
    )
    .expect("reorged receipt"));
}

#[test]
fn stellar_send_requires_successor_ledger_and_stable_inclusion() {
    let (_directory, root) = live_route();
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let intent: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).expect("intent");
    let status = templar_oft_bridge_cli::stellar::StellarTransactionStatusV1 {
        status: "success".into(),
        ledger: Some(4_310),
        envelope_xdr: None,
        contract_events: Vec::new(),
    };

    assert!(!templar_oft_bridge_cli::canary::stellar_source_finalized(
        &intent, &status, 4_310, &status,
    )
    .expect("same-ledger status"));
    assert!(templar_oft_bridge_cli::canary::stellar_source_finalized(
        &intent, &status, 4_311, &status,
    )
    .expect("stable successor-ledger status"));

    let displaced = templar_oft_bridge_cli::stellar::StellarTransactionStatusV1 {
        ledger: Some(4_311),
        ..status.clone()
    };
    assert!(!templar_oft_bridge_cli::canary::stellar_source_finalized(
        &intent, &status, 4_312, &displaced,
    )
    .expect("changed inclusion ledger"));
}

#[test]
fn reverse_live_quote_binds_evm_block_nonce_and_supply() {
    let (_directory, root) = live_route();
    let stellar = obs_stellar(4_310, "41", "0", "2000000");
    let evm = FakeObservedEvm {
        block: 420,
        nonce: 7,
        words: BTreeMap::from([
            (selector("totalSupply()"), u256_word("2000000")),
            (selector("balanceOf(address)"), u256_word("500000")),
        ]),
    };
    let (_out_dir, out) = quote_live_intent(
        &root,
        Direction::EvmToStellar,
        100_000,
        STELLAR_DESTINATION,
        &stellar,
        &evm,
    );
    let intent: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).expect("intent");
    assert_eq!(intent.quote_source_ledger, None);
    assert_eq!(intent.quote_source_block, Some(420));
    assert_eq!(intent.observed_sequence_nonce.as_deref(), Some("7"));
    let snapshot = intent.pre_send_snapshot.expect("pre-send snapshot");
    assert_eq!(snapshot.source_balance_raw, "500000");
    assert_eq!(snapshot.lockbox_raw, "2000000");
    assert_eq!(snapshot.evm_supply_raw, "2000000");
    assert!(matches!(
        intent.fee_ceiling,
        Some(templar_oft_bridge_cli::domain::LegFeeCeilingV1::Evm { .. })
    ));
}

#[test]
fn send_live_accepts_matching_observations() {
    let (_directory, root) = live_route();
    let stellar = obs_stellar(4_310, "41", "900000", "2000000");
    let evm = obs_evm(420, 7, "2000000");
    let (_out_dir, out) = quote_live_intent(
        &root,
        Direction::StellarToEvm,
        100_000,
        EVM_DESTINATION,
        &stellar,
        &evm,
    );
    templar_oft_bridge_cli::canary::send_operation_live(&root, &out, false, &stellar, &evm)
        .expect("identical live observations must not drift");
}

#[test]
fn send_live_rejects_stellar_ledger_drift_before_signing() {
    let (_directory, root) = live_route();
    let stellar = obs_stellar(4_310, "41", "900000", "2000000");
    let evm = obs_evm(420, 7, "2000000");
    let (_out_dir, out) = quote_live_intent(
        &root,
        Direction::StellarToEvm,
        100_000,
        EVM_DESTINATION,
        &stellar,
        &evm,
    );
    let drifted = obs_stellar(4_311, "41", "900000", "2000000");
    let error =
        templar_oft_bridge_cli::canary::send_operation_live(&root, &out, false, &drifted, &evm)
            .expect_err("source ledger drift must refuse before signing");
    assert!(
        matches!(error, Error::Conflict(_)) && error.to_string().contains("quote_source_ledger")
    );
}

#[test]
fn send_live_rejects_stellar_lockbox_drift_before_signing() {
    let (_directory, root) = live_route();
    let stellar = obs_stellar(4_310, "41", "900000", "2000000");
    let evm = obs_evm(420, 7, "2000000");
    let (_out_dir, out) = quote_live_intent(
        &root,
        Direction::StellarToEvm,
        100_000,
        EVM_DESTINATION,
        &stellar,
        &evm,
    );
    let drifted = obs_stellar(4_310, "41", "900000", "2000001");
    let error =
        templar_oft_bridge_cli::canary::send_operation_live(&root, &out, false, &drifted, &evm)
            .expect_err("lockbox reserve drift must refuse before signing");
    assert!(matches!(error, Error::Conflict(_)) && error.to_string().contains("lockbox_raw"));
}

#[test]
fn send_live_rejects_stellar_sequence_drift_before_signing() {
    let (_directory, root) = live_route();
    let stellar = obs_stellar(4_310, "41", "900000", "2000000");
    let evm = obs_evm(420, 7, "2000000");
    let (_out_dir, out) = quote_live_intent(
        &root,
        Direction::StellarToEvm,
        100_000,
        EVM_DESTINATION,
        &stellar,
        &evm,
    );
    let drifted = obs_stellar(4_310, "42", "900000", "2000000");
    let error =
        templar_oft_bridge_cli::canary::send_operation_live(&root, &out, false, &drifted, &evm)
            .expect_err("source sequence drift must refuse before signing");
    assert!(
        matches!(error, Error::Conflict(_))
            && error.to_string().contains("observed_sequence_nonce")
    );
}

#[test]
fn send_live_rejects_evm_block_drift_before_signing() {
    let (_directory, root) = live_route();
    let stellar = obs_stellar(4_310, "41", "0", "2000000");
    let evm = FakeObservedEvm {
        block: 420,
        nonce: 7,
        words: BTreeMap::from([
            (selector("totalSupply()"), u256_word("2000000")),
            (selector("balanceOf(address)"), u256_word("500000")),
        ]),
    };
    let (_out_dir, out) = quote_live_intent(
        &root,
        Direction::EvmToStellar,
        100_000,
        STELLAR_DESTINATION,
        &stellar,
        &evm,
    );
    let drifted = FakeObservedEvm {
        block: 421,
        nonce: 7,
        words: evm.words.clone(),
    };
    let error =
        templar_oft_bridge_cli::canary::send_operation_live(&root, &out, false, &stellar, &drifted)
            .expect_err("source block drift must refuse before signing");
    assert!(
        matches!(error, Error::Conflict(_)) && error.to_string().contains("quote_source_block")
    );
}

#[test]
fn send_live_rejects_evm_supply_drift_before_signing() {
    let (_directory, root) = live_route();
    let stellar = obs_stellar(4_310, "41", "0", "2000000");
    let evm = FakeObservedEvm {
        block: 420,
        nonce: 7,
        words: BTreeMap::from([
            (selector("totalSupply()"), u256_word("2000000")),
            (selector("balanceOf(address)"), u256_word("500000")),
        ]),
    };
    let (_out_dir, out) = quote_live_intent(
        &root,
        Direction::EvmToStellar,
        100_000,
        STELLAR_DESTINATION,
        &stellar,
        &evm,
    );
    let drifted = FakeObservedEvm {
        block: 420,
        nonce: 7,
        words: BTreeMap::from([
            (selector("totalSupply()"), u256_word("2000001")),
            (selector("balanceOf(address)"), u256_word("500000")),
        ]),
    };
    let error =
        templar_oft_bridge_cli::canary::send_operation_live(&root, &out, false, &stellar, &drifted)
            .expect_err("evm supply drift must refuse before signing");
    assert!(matches!(error, Error::Conflict(_)) && error.to_string().contains("evm_supply_raw"));
}

#[test]
fn send_live_refuses_an_offline_preview_intent() {
    let (_directory, root) = live_route();
    let stellar = obs_stellar(4_310, "41", "900000", "2000000");
    let evm = obs_evm(420, 7, "2000000");
    let (_out_dir, out) = quote_intent(&root, Direction::StellarToEvm, 100_000, EVM_DESTINATION);
    let error =
        templar_oft_bridge_cli::canary::send_operation_live(&root, &out, false, &stellar, &evm)
            .expect_err("an offline preview intent has no live observations to bind");
    assert!(
        matches!(error, Error::Conflict(_)) && error.to_string().contains("quote_source_ledger")
    );
}

#[test]
fn stellar_plan_fee_ceiling_refuses_over_budget_envelope() {
    use soroban_client::{
        account::{Account, AccountBehavior as _},
        transaction_builder::{TransactionBuilder, TransactionBuilderBehavior as _},
        xdr::{Limits, WriteXdr as _},
    };
    use stellar_baselib::transaction::TransactionBehavior as _;

    let (_directory, root) = live_route();
    let stellar = obs_stellar(4_310, "41", "900000", "2000000");
    let evm = obs_evm(420, 7, "2000000");
    let (_out_dir, out) = quote_live_intent(
        &root,
        Direction::StellarToEvm,
        100_000,
        EVM_DESTINATION,
        &stellar,
        &evm,
    );
    let intent: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).expect("intent");
    let mut account = Account::new(STELLAR_ACCOUNT, "41").expect("account");
    let operation = stellar_baselib::operation::Operation::new()
        .invoke_contract(common::STELLAR_OFT, "balance", Vec::new(), None)
        .expect("operation");
    let mut builder =
        TransactionBuilder::new(&mut account, "Test SDF Network ; September 2015", None);
    builder.fee(100_001u32).add_operation(operation);
    let transaction = builder.build();
    let envelope_xdr = transaction
        .to_envelope()
        .expect("envelope")
        .to_xdr_base64(Limits::none())
        .expect("envelope xdr");
    let binding = templar_oft_bridge_cli::domain::StellarPlanBindingV1 {
        network_passphrase: "Test SDF Network ; September 2015".into(),
        source_account: STELLAR_ACCOUNT.into(),
        sequence: "41".into(),
        min_ledger: 4_300,
        max_ledger: 4_400,
        auth_entries: Vec::new(),
        envelope_xdr,
        envelope_sha256: "aa".repeat(32),
        simulation_ledger: 4_310,
        signer_weights: BTreeMap::from([(STELLAR_ACCOUNT.to_string(), 1)]),
        required_threshold_weight: 1,
        threshold_level: "low".into(),
    };
    let error = templar_oft_bridge_cli::canary::verify_stellar_plan_fee_ceiling(&intent, &binding)
        .expect_err("over-budget envelope fee must refuse before signing");
    assert!(
        matches!(error, Error::Policy(_)) && error.to_string().contains("resource-fee ceiling")
    );
}

#[test]
fn evm_plan_fee_ceiling_refuses_over_budget_fee_policy() {
    let (_directory, root) = live_route();
    let stellar = obs_stellar(4_310, "41", "0", "2000000");
    let evm = FakeObservedEvm {
        block: 420,
        nonce: 7,
        words: BTreeMap::from([
            (selector("totalSupply()"), u256_word("2000000")),
            (selector("balanceOf(address)"), u256_word("500000")),
        ]),
    };
    let (_out_dir, out) = quote_live_intent(
        &root,
        Direction::EvmToStellar,
        100_000,
        STELLAR_DESTINATION,
        &stellar,
        &evm,
    );
    let intent: LegIntentV1 = templar_oft_bridge_cli::state::read_json(&out).expect("intent");
    let binding = templar_oft_bridge_cli::domain::EvmPlanBindingV1 {
        chain_id: "11155111".into(),
        sender: "0x1111111111111111111111111111111111111111".into(),
        target: FAKE_EVM_TOKEN.into(),
        value: "0".into(),
        nonce: "7".into(),
        calldata: "0x".into(),
        gas_limit: "400000".into(),
        max_fee_per_gas_wei: "50000000000".into(),
        max_priority_fee_per_gas_wei: "1000000000".into(),
        transaction_digest: "digest".into(),
        safe: None,
    };
    let error = templar_oft_bridge_cli::canary::verify_evm_plan_fee_ceiling(&intent, &binding)
        .expect_err("over-budget gas limit must refuse before signing");
    assert!(matches!(error, Error::Policy(_)) && error.to_string().contains("gas_limit"));
}
