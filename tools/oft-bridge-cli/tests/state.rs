use std::fs;
use std::process::{Command, Stdio};
use std::time::Duration;

use std::collections::BTreeMap;
use templar_oft_bridge_cli::{
    domain::{
        AssetKind, AssetPolicyV1, ChainIdentityV1, DesiredRouteV1, Environment, OpeningCustodyV1,
        Vm,
    },
    state::{OperationEventV1, OperationState, RouteStore},
};

fn desired() -> DesiredRouteV1 {
    DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: 1,
        route_id: "route-a".into(),
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

const SENDER: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const EVM_SENDER: &str = "0x0000000000000000000000000000000000000001";

fn operations_root(directory: &std::path::Path) -> std::path::PathBuf {
    let root = directory.join("ops");
    assert!(fs::create_dir(&root).is_ok(), "operations root");
    root
}

#[test]
fn creates_route_and_verifies_hash_chain() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create route");
    let _lock = store.lock().expect("lock");
    store
        .append_operation(
            OperationEventV1 {
                operation_id: "op-1".into(),
                state: OperationState::Planned,
                detail: serde_json::json!({"x":1}),
            },
            None,
        )
        .expect("append");
    let records = store
        .verify_log::<OperationEventV1>(std::path::Path::new("operations.jsonl"), "operations")
        .expect("verify");
    assert_eq!(records.len(), 1);
}

#[test]
fn detects_log_tampering() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create route");
    store
        .append_operation(
            OperationEventV1 {
                operation_id: "op-1".into(),
                state: OperationState::Planned,
                detail: serde_json::json!({"x":1}),
            },
            None,
        )
        .expect("append");
    let path = root.join("operations.jsonl");
    let raw = fs::read_to_string(&path)
        .expect("read")
        .replace("op-1", "op-2");
    fs::write(path, raw).expect("tamper");
    let error = store
        .verify_log::<OperationEventV1>(std::path::Path::new("operations.jsonl"), "operations")
        .expect_err("must reject tamper");
    assert!(error.to_string().contains("digest mismatch"));
}

#[test]
fn route_lock_excludes_second_writer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create route");
    let first = store.lock().expect("first lock");
    let error = store.lock().expect_err("second lock must fail");
    assert!(error.to_string().contains("busy"));
    drop(first);
    assert!(root.join(".lock").is_file(), "lock inode remains stable");
    store.lock().expect("lock after release");
}

#[test]
fn domain_lock_excludes_second_route_in_same_domain() {
    let directory = tempfile::tempdir().expect("tempdir");
    let operations_root = operations_root(directory.path());
    let root_a = directory.path().join("routes-a/route");
    let root_b = directory.path().join("routes-b/route");
    fs::create_dir(directory.path().join("routes-a")).expect("route a parent");
    fs::create_dir(directory.path().join("routes-b")).expect("route b parent");
    let (store_a, _) = RouteStore::create(&root_a, desired()).expect("create a");
    let (store_b, _) = RouteStore::create(&root_b, desired()).expect("create b");
    let binding_a = store_a
        .derive_phase_a_binding(Vm::Stellar, SENDER)
        .expect("phase a");
    let binding_b = store_b
        .derive_phase_a_binding(Vm::Stellar, SENDER)
        .expect("phase a b");
    assert_eq!(
        binding_a.domain_sha256(),
        binding_b.domain_sha256(),
        "same environment/vm/sender must share the authority domain"
    );
    let guard = store_a
        .acquire_mutation(&binding_a, &operations_root, "test-operation")
        .expect("acquire a");
    let error = store_b
        .acquire_mutation(&binding_b, &operations_root, "test-operation")
        .expect_err("second route in the same domain must be excluded");
    assert!(error.to_string().contains("busy"));
    drop(guard);
    store_b
        .acquire_mutation(&binding_b, &operations_root, "test-operation")
        .expect("acquire b after release");
}

#[test]
fn evm_authority_domain_canonicalizes_address_case() {
    let directory = tempfile::tempdir().expect("tempdir");
    let operations_root = operations_root(directory.path());
    let root_a = directory.path().join("route-a");
    let root_b = directory.path().join("route-b");
    let (store_a, _) = RouteStore::create(&root_a, desired()).expect("create a");
    let (store_b, _) = RouteStore::create(&root_b, desired()).expect("create b");
    let lower = "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd";
    let mixed = "0xAbCdEfAbCdEfAbCdEfAbCdEfAbCdEfAbCdEfAbCd";
    let binding_a = store_a
        .derive_phase_a_binding(Vm::Evm, lower)
        .expect("lowercase binding");
    let binding_b = store_b
        .derive_phase_a_binding(Vm::Evm, mixed)
        .expect("mixed-case binding");
    assert_eq!(binding_a.sender(), lower);
    assert_eq!(binding_b.sender(), lower);
    assert_eq!(binding_a.domain_sha256(), binding_b.domain_sha256());

    let guard = store_a
        .acquire_mutation(&binding_a, &operations_root, "op-a")
        .expect("first route acquires canonical domain");
    let error = store_b
        .acquire_mutation(&binding_b, &operations_root, "op-b")
        .expect_err("alternate address spelling cannot bypass domain lock");
    assert!(error.to_string().contains("busy"));
    drop(guard);
}

#[test]
fn stale_phase_a_binding_is_rejected_and_locks_released() {
    let directory = tempfile::tempdir().expect("tempdir");
    let operations_root = operations_root(directory.path());
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create");
    let binding = store
        .derive_phase_a_binding(Vm::Stellar, SENDER)
        .expect("phase a");
    let mut state = store.load_state().expect("load state");
    state.identity.environment = Environment::StellarMainnetEthereum;
    store
        .save_state(&state)
        .expect("mutate state between phase a and acquire");
    let error = store
        .acquire_mutation(&binding, &operations_root, "test-operation")
        .expect_err("stale binding must be rejected");
    assert!(error.to_string().contains("stale"));
    assert!(root.join(".lock").is_file(), "route lock inode persists");
    assert!(
        operations_root
            .join(".authority")
            .join(format!("{}.lock", binding.domain_sha256()))
            .is_file(),
        "authority-domain lock inode persists"
    );
    let fresh = store
        .derive_phase_a_binding(Vm::Stellar, SENDER)
        .expect("re-derive after state changed");
    store
        .acquire_mutation(&fresh, &operations_root, "test-operation")
        .expect("restart with a fresh binding succeeds");
}

#[test]
fn advisory_lock_crash_child() {
    let Ok(root) = std::env::var("TMPLR_LOCK_CRASH_ROUTE") else {
        return;
    };
    let operations_root = std::env::var("TMPLR_LOCK_CRASH_OPERATIONS").expect("operations root");
    let marker = std::env::var("TMPLR_LOCK_CRASH_MARKER").expect("marker");
    let store = RouteStore::open(std::path::Path::new(&root)).expect("open child route");
    let binding = store
        .derive_phase_a_binding(Vm::Evm, EVM_SENDER)
        .expect("child binding");
    let _guard = store
        .acquire_mutation(&binding, std::path::Path::new(&operations_root), "crash-op")
        .expect("child guard");
    fs::write(marker, b"locked").expect("ready marker");
    loop {
        std::thread::park();
    }
}

#[test]
fn process_death_releases_persistent_route_and_authority_locks() {
    let directory = tempfile::tempdir().expect("tempdir");
    let operations_root = operations_root(directory.path());
    let root = directory.path().join("route");
    let marker = directory.path().join("child-ready");
    let (store, _) = RouteStore::create(&root, desired()).expect("create route");
    let binding = store
        .derive_phase_a_binding(Vm::Evm, EVM_SENDER)
        .expect("binding");

    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "advisory_lock_crash_child", "--nocapture"])
        .env("TMPLR_LOCK_CRASH_ROUTE", &root)
        .env("TMPLR_LOCK_CRASH_OPERATIONS", &operations_root)
        .env("TMPLR_LOCK_CRASH_MARKER", &marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lock holder");
    for _ in 0..200 {
        if marker.exists() {
            break;
        }
        if child.try_wait().expect("poll child").is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "child acquired both locks");
    child.kill().expect("kill lock holder without running Drop");
    child.wait().expect("reap lock holder");

    assert!(root.join(".lock").is_file(), "route lock file persists");
    assert!(
        operations_root
            .join(".authority")
            .join(format!("{}.lock", binding.domain_sha256()))
            .is_file(),
        "authority lock file persists"
    );
    store
        .acquire_mutation(&binding, &operations_root, "after-crash")
        .expect("kernel releases both advisory locks on process death");
}

#[test]
fn submission_guard_holds_locks_through_checkpoint() {
    let directory = tempfile::tempdir().expect("tempdir");
    let operations_root = operations_root(directory.path());
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create");
    let binding = store
        .derive_phase_a_binding(Vm::Stellar, SENDER)
        .expect("phase a");
    let guard = store
        .acquire_mutation(&binding, &operations_root, "op-send")
        .expect("acquire");
    assert!(
        store
            .acquire_mutation(&binding, &operations_root, "op-send")
            .is_err(),
        "authority-domain lock held while checkpointing"
    );
    assert!(store.lock().is_err(), "route lock held while checkpointing");
    guard
        .store()
        .append_operation(
            OperationEventV1 {
                operation_id: "op-send".into(),
                state: OperationState::Planned,
                detail: serde_json::json!({"unsigned_transaction_sha256": "0xfeed"}),
            },
            None,
        )
        .expect("planned");
    guard
        .store()
        .append_operation(
            OperationEventV1 {
                operation_id: "op-send".into(),
                state: OperationState::Signed,
                detail: serde_json::json!({"signed_transaction_sha256": "0xdeadbeef"}),
            },
            None,
        )
        .expect("signed");
    guard
        .submission_pending("op-send", "0xdeadbeef", "0x1234", "signed-payload")
        .expect("checkpoint appended under both locks");
    let records = guard
        .store()
        .verify_log::<OperationEventV1>(std::path::Path::new("operations.jsonl"), "operations")
        .expect("verify");
    let last = records.last().expect("record");
    assert_eq!(last.payload.state, OperationState::SubmissionPending);
    assert_eq!(last.payload.operation_id, "op-send");
    drop(guard);
    store.lock().expect("route lock released after guard drop");
    store
        .acquire_mutation(&binding, &operations_root, "op-send")
        .expect("domain lock released after guard drop");
}

#[test]
fn unresolved_authority_reservation_survives_guard_drop_and_blocks_competitors() {
    let directory = tempfile::tempdir().expect("tempdir");
    let operations_root = operations_root(directory.path());
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create");
    let binding = store
        .derive_phase_a_binding(Vm::Evm, EVM_SENDER)
        .expect("phase a");
    let guard = store
        .acquire_mutation(&binding, &operations_root, "op-a")
        .expect("acquire");
    guard.reserve_authority("7", &"a".repeat(64)).unwrap();
    drop(guard);

    assert!(store
        .acquire_mutation(&binding, &operations_root, "op-b")
        .is_err());
    let resumed = store
        .acquire_mutation(&binding, &operations_root, "op-a")
        .expect("same operation resumes");
    resumed.reserve_authority("7", &"a".repeat(64)).unwrap();
    for state in [OperationState::Planned, OperationState::Signed] {
        resumed
            .store()
            .append_operation(
                OperationEventV1 {
                    operation_id: "op-a".into(),
                    state,
                    detail: serde_json::json!({"signed_transaction_sha256": "a".repeat(64)}),
                },
                None,
            )
            .unwrap();
    }
    resumed
        .submission_pending("op-a", &"a".repeat(64), "0x1", "payload")
        .unwrap();
    resumed
        .store()
        .append_operation(
            OperationEventV1 {
                operation_id: "op-a".into(),
                state: OperationState::Confirmed,
                detail: serde_json::json!({"transaction_hash": "0x1"}),
            },
            None,
        )
        .unwrap();
    resumed.release_authority_if_terminal().unwrap();
    drop(resumed);
    store
        .acquire_mutation(&binding, &operations_root, "op-b")
        .expect("terminal release allows competitor");
}

fn pending_proposal(
    operation_id: &str,
    relative: &std::path::Path,
    sha256: &str,
) -> OperationEventV1 {
    OperationEventV1 {
        operation_id: operation_id.into(),
        state: OperationState::ProposalPrepared,
        detail: serde_json::json!({"path": relative, "sha256": sha256}),
    }
}

#[test]
fn recovers_prepared_proposal_from_matching_final() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create");
    let relative = std::path::Path::new("proposals/set-peer.json");
    let payload = serde_json::json!({"kind": "set_peer", "remote_eid": 40161});
    let sha256 = templar_oft_bridge_cli::canonical_sha256(&payload).expect("sha256");
    let final_path = root.join(relative);
    let temporary = final_path.with_extension("tmp");
    fs::create_dir_all(final_path.parent().expect("parent")).expect("parent dir");
    templar_oft_bridge_cli::state::write_create_new_json(&final_path, &payload).expect("final");
    templar_oft_bridge_cli::state::write_create_new_json(&temporary, &payload).expect("temp");
    store
        .append_operation(
            pending_proposal("op-recover", relative, &sha256),
            Some(sha256.clone()),
        )
        .expect("prepared checkpoint");
    let _lock = store.lock().expect("route lock");
    store.recover_pending_proposal().expect("recover");
    assert!(!temporary.exists(), "orphan temporary removed");
    store.recover_pending_proposal().expect("recover again");
    let records = store
        .verify_log::<OperationEventV1>(std::path::Path::new("operations.jsonl"), "operations")
        .expect("verify");
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].payload.state, OperationState::ProposalCommitted);
    assert_eq!(records[1].payload.operation_id, "op-recover");
    assert_eq!(
        records[1].companion_artifact_sha256.as_deref(),
        Some(sha256.as_str())
    );
}

#[test]
fn recovers_prepared_proposal_from_matching_temporary() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create");
    let relative = std::path::Path::new("proposals/set-peer.json");
    let payload = serde_json::json!({"kind": "set_peer", "remote_eid": 40161});
    let sha256 = templar_oft_bridge_cli::canonical_sha256(&payload).expect("sha256");
    let final_path = root.join(relative);
    let temporary = final_path.with_extension("tmp");
    fs::create_dir_all(final_path.parent().expect("parent")).expect("parent dir");
    templar_oft_bridge_cli::state::write_create_new_json(&temporary, &payload).expect("temp");
    store
        .append_operation(
            pending_proposal("op-recover", relative, &sha256),
            Some(sha256),
        )
        .expect("prepared checkpoint");
    let _lock = store.lock().expect("route lock");
    store.recover_pending_proposal().expect("recover");
    assert!(!temporary.exists(), "temporary consumed by rename");
    let recovered: serde_json::Value =
        templar_oft_bridge_cli::state::read_json(&final_path).expect("final artifact");
    assert_eq!(recovered, payload);
    store.recover_pending_proposal().expect("recover again");
}

#[test]
fn mismatched_prepared_proposal_fails_closed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create");
    let relative = std::path::Path::new("proposals/set-peer.json");
    let payload = serde_json::json!({"kind": "set_peer", "remote_eid": 40161});
    let other = serde_json::json!({"kind": "set_peer", "remote_eid": 30102});
    let sha256 = templar_oft_bridge_cli::canonical_sha256(&payload).expect("sha256");
    let final_path = root.join(relative);
    let temporary = final_path.with_extension("tmp");
    fs::create_dir_all(final_path.parent().expect("parent")).expect("parent dir");
    templar_oft_bridge_cli::state::write_create_new_json(&final_path, &other).expect("final");
    templar_oft_bridge_cli::state::write_create_new_json(&temporary, &payload)
        .expect("matching temp");
    store
        .append_operation(
            pending_proposal("op-recover", relative, &sha256),
            Some(sha256),
        )
        .expect("prepared checkpoint");
    let _lock = store.lock().expect("route lock");
    let error = store
        .recover_pending_proposal()
        .expect_err("mismatched final must fail closed");
    assert!(error.to_string().contains("proposal_write_failed"));
    let on_disk: serde_json::Value =
        templar_oft_bridge_cli::state::read_json(&final_path).expect("final untouched");
    assert_eq!(on_disk, other);
    assert!(temporary.exists(), "matching temporary preserved");
    let records = store
        .verify_log::<OperationEventV1>(std::path::Path::new("operations.jsonl"), "operations")
        .expect("verify");
    assert_eq!(records.len(), 1, "no committed checkpoint appended");
    assert_eq!(records[0].payload.state, OperationState::ProposalPrepared);
}

#[test]
fn two_process_domain_exclusion() {
    let directory = tempfile::tempdir().expect("tempdir");
    let operations_root = operations_root(directory.path());
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("create");
    let binding = store
        .derive_phase_a_binding(Vm::Stellar, SENDER)
        .expect("phase a");
    let held = directory.path().join("HELD");
    let release = directory.path().join("RELEASE");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .arg("--exact")
        .arg("two_process_domain_exclusion_hold_child")
        .env("TWO_PROCESS_DOMAIN_ROOT", &root)
        .env("TWO_PROCESS_DOMAIN_OPS", &operations_root)
        .env("TWO_PROCESS_DOMAIN_HELD", &held)
        .env("TWO_PROCESS_DOMAIN_RELEASE", &release)
        .spawn()
        .expect("spawn child");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !held.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "child never acquired the domain lock"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let error = store
        .acquire_mutation(&binding, &operations_root, "test-operation")
        .expect_err("parent must observe the child-held domain lock");
    assert!(error.to_string().contains("busy"));
    fs::write(&release, b"release").expect("release marker");
    let status = child.wait().expect("child exit");
    assert!(status.success(), "child harness must pass");
    store
        .acquire_mutation(&binding, &operations_root, "test-operation")
        .expect("parent acquires after child release");
}

#[test]
fn two_process_domain_exclusion_hold_child() {
    let Ok(root) = std::env::var("TWO_PROCESS_DOMAIN_ROOT") else {
        return;
    };
    let ops = std::env::var("TWO_PROCESS_DOMAIN_OPS").expect("ops env");
    let held = std::env::var("TWO_PROCESS_DOMAIN_HELD").expect("held env");
    let release = std::env::var("TWO_PROCESS_DOMAIN_RELEASE").expect("release env");
    let store = RouteStore::open(std::path::Path::new(&root)).expect("child open");
    let binding = store
        .derive_phase_a_binding(Vm::Stellar, SENDER)
        .expect("child phase a");
    let _guard = store
        .acquire_mutation(&binding, std::path::Path::new(&ops), "test-operation")
        .expect("child acquire");
    fs::write(&held, b"held").expect("held marker");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !std::path::Path::new(&release).exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "child timed out waiting for release"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn opening_custody() -> OpeningCustodyV1 {
    OpeningCustodyV1 {
        schema_name: "opening_custody".into(),
        schema_version: 1,
        stellar_ledger: 123,
        stellar_ledger_hash: "stellar-hash".into(),
        stellar_ledger_time_unix: 1_700_000_000,
        lockbox_raw: 0,
        evm_block: 456,
        evm_block_hash: "evm-hash".into(),
        evm_supply_raw: 0,
        artifact_lock_sha256: "artifact-hash".into(),
        effective_config_sha256: "config-hash".into(),
        zero_packet_history_proven: true,
        history_evidence_sha256: None,
    }
}

#[test]
fn opening_custody_is_recorded_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (store, _) =
        RouteStore::create(&directory.path().join("route"), desired()).expect("create");
    store
        .record_opening_custody(opening_custody())
        .expect("record baseline");
    assert_eq!(
        store
            .load_state()
            .expect("state")
            .opening_custody
            .expect("opening"),
        opening_custody()
    );
    assert!(store.record_opening_custody(opening_custody()).is_err());
}

#[test]
fn opening_custody_requires_exactly_one_history_authority() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (store, _) =
        RouteStore::create(&directory.path().join("route"), desired()).expect("create");
    let mut invalid = opening_custody();
    invalid.history_evidence_sha256 = Some("history".into());
    assert!(store.record_opening_custody(invalid).is_err());
}
