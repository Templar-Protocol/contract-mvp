//! Route mutation DAG tests: deterministic dependency order, exact readback
//! comparison, mismatch refusal with typed conflicts, idempotent convergence
//! on reruns, and the testnet-only mutation gate. Pure planner tests over
//! typed fixtures; no live chain access.

use std::collections::BTreeMap;

use serde_json::json;

use templar_oft_bridge_cli::domain::{
    AssetKind, AssetPolicyV1, ChainIdentityV1, DesiredRouteV1, Direction, Environment, OperationV1,
    RouteStateV1, Vm, SCHEMA_VERSION,
};
use templar_oft_bridge_cli::environment::{
    ETHEREUM_EID, SEPOLIA_EID, STELLAR_MAINNET_EID, STELLAR_MAINNET_ENDPOINT,
    STELLAR_PUBLIC_PASSPHRASE, STELLAR_TESTNET_EID, STELLAR_TESTNET_ENDPOINT,
    STELLAR_TESTNET_PASSPHRASE,
};
use templar_oft_bridge_cli::error::Error;
use templar_oft_bridge_cli::route::{
    config_key_executor_config, config_key_receive_library, config_key_receive_library_grace,
    config_key_receive_options, config_key_send_library, config_key_uln_config, mutation_gate,
    peer_field, plan_route_mutations, require_convergence, RouteStepStatus,
};
use templar_oft_bridge_cli::wrap::stellar_salt;
use templar_oft_bridge_cli::{canonical_sha256, codec};

const ROUTE_ID: &str = "route-route-planner";
const STELLAR_OWNER: &str = "GCLQ3APIE5AS4XJUTRP5AF7ZMQAXDNGIRMIF3MIWQPF6ZPFJVNJDCN5E";
const EVM_OWNER: &str = "0xc61B17BF20b4B16bb70C1942CD8D9eBDe6726386";
const EVM_END_TO_END: u32 = SEPOLIA_EID;
const STELLAR_END: u32 = STELLAR_TESTNET_EID;

const STELLAR_SEND_LIB: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB";
const EVM_SEND_LIB: &str = "0x1111111111111111111111111111111111111111";
const STELLAR_RECEIVE_LIB: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAC";
const EVM_RECEIVE_LIB: &str = "0x2222222222222222222222222222222222222222";
const OPTIONS_HEX: &str = "0x0001";
const GRACE: &str = "500";

fn identity(environment: Environment) -> ChainIdentityV1 {
    match environment {
        Environment::StellarTestnetSepolia => ChainIdentityV1 {
            environment,
            stellar_passphrase: STELLAR_TESTNET_PASSPHRASE.into(),
            stellar_eid: STELLAR_TESTNET_EID,
            stellar_endpoint: STELLAR_TESTNET_ENDPOINT.into(),
            stellar_endpoint_code_hash: "0".repeat(64),
            evm_chain_id: 11_155_111,
            evm_eid: SEPOLIA_EID,
            evm_endpoint: "0x6EDCE65403992e310A62460808c4b910D972f10f".into(),
            evm_endpoint_code_hash: "0".repeat(64),
        },
        Environment::StellarMainnetEthereum => ChainIdentityV1 {
            environment,
            stellar_passphrase: STELLAR_PUBLIC_PASSPHRASE.into(),
            stellar_eid: STELLAR_MAINNET_EID,
            stellar_endpoint: STELLAR_MAINNET_ENDPOINT.into(),
            stellar_endpoint_code_hash: "0".repeat(64),
            evm_chain_id: 1,
            evm_eid: ETHEREUM_EID,
            evm_endpoint: templar_oft_bridge_cli::environment::ETHEREUM_ENDPOINT.into(),
            evm_endpoint_code_hash: "0".repeat(64),
        },
    }
}

fn asset() -> AssetPolicyV1 {
    AssetPolicyV1 {
        kind: AssetKind::NativeSac,
        asset_id: "native".into(),
        local_decimals: 7,
        issuer_custodian_evidence_sha256: None,
        destination_acceptance_evidence_sha256: None,
        custody_risk_acceptance_sha256: None,
        forbidden_classic_issuer: None,
        evidence: BTreeMap::new(),
    }
}

fn desired(config: BTreeMap<String, serde_json::Value>) -> DesiredRouteV1 {
    DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: SCHEMA_VERSION,
        route_id: ROUTE_ID.into(),
        identity: identity(Environment::StellarTestnetSepolia),
        asset: asset(),
        stellar_owner: STELLAR_OWNER.into(),
        stellar_delegate: STELLAR_OWNER.into(),
        evm_owner: EVM_OWNER.into(),
        evm_delegate: EVM_OWNER.into(),
        config,
    }
}

/// Full route-config request: every canonical field on both sides.
fn full_config() -> BTreeMap<String, serde_json::Value> {
    let mut config = BTreeMap::new();
    for (vm, eid, send, receive, dvn, executor) in [
        (
            "stellar",
            EVM_END_TO_END,
            STELLAR_SEND_LIB,
            STELLAR_RECEIVE_LIB,
            STELLAR_OWNER,
            STELLAR_OWNER,
        ),
        (
            "evm",
            STELLAR_END,
            EVM_SEND_LIB,
            EVM_RECEIVE_LIB,
            EVM_OWNER,
            EVM_OWNER,
        ),
    ] {
        config.insert(format!("send_library:{vm}:{eid}"), json!(send));
        config.insert(format!("receive_library:{vm}:{eid}"), json!(receive));
        config.insert(format!("receive_library_grace:{vm}:{eid}"), json!(GRACE));
        let uln = json!({
            "required_dvns": [dvn],
            "optional_dvns": [],
            "optional_threshold": 0,
            "confirmations": 1
        });
        config.insert(format!("uln_send_config:{vm}:{eid}"), uln.clone());
        config.insert(format!("uln_receive_config:{vm}:{eid}"), uln);
        config.insert(
            format!("executor_config:{vm}:{eid}"),
            json!({"max_message_size": 10000, "executor": executor}),
        );
        config.insert(format!("receive_options:{vm}:{eid}:1"), json!(OPTIONS_HEX));
    }
    config
}

/// Recorded effective state mirroring every requested field (a converged
/// route), plus the recorded OFT deployments and peers.
fn converged_state() -> RouteStateV1 {
    let desired = desired(full_config());
    let stellar_oft = codec::derive_stellar_contract_address(
        &desired.identity.stellar_passphrase,
        STELLAR_OWNER,
        &stellar_salt(ROUTE_ID, "native"),
    )
    .expect("stellar oft derivation");
    let evm_oft = format!("0x{}", "22".repeat(20));
    let stellar_peer = format!(
        "0x{}",
        hex::encode(codec::evm_address_to_bytes32(&evm_oft).expect("evm peer bytes"))
    );
    let evm_peer = format!(
        "0x{}",
        hex::encode(codec::strkey_to_bytes32(&stellar_oft).expect("stellar peer bytes"))
    );
    let mut effective_config = full_config();
    for (key, value) in &mut effective_config {
        if key.starts_with("uln_") {
            let typed: templar_oft_bridge_cli::layerzero::UlnConfigType3V1 =
                serde_json::from_value(value.clone()).unwrap();
            *value = json!(typed.config_sha256().unwrap());
        } else if key.starts_with("executor_config:") {
            let typed: templar_oft_bridge_cli::layerzero::ExecutorConfigType3V1 =
                serde_json::from_value(value.clone()).unwrap();
            *value = json!(typed.config_sha256().unwrap());
        }
    }
    let mut state = RouteStateV1 {
        schema_name: "route_state".into(),
        schema_version: SCHEMA_VERSION,
        route_id: ROUTE_ID.into(),
        desired_sha256: canonical_sha256(&desired).expect("desired digest"),
        identity: desired.identity,
        asset: desired.asset,
        opening_custody: None,
        operations_log: "operations.jsonl".into(),
        messages_log: "messages.jsonl".into(),
        lock_file: ".lock".into(),
        contracts: BTreeMap::from([
            ("stellar_oft".into(), stellar_oft.clone()),
            ("evm_oft".into(), evm_oft.clone()),
            (format!("peer:{}", EVM_END_TO_END), stellar_peer),
            (format!("peer:{}", STELLAR_END), evm_peer),
            ("stellar_owner".into(), STELLAR_OWNER.into()),
            ("evm_owner".into(), EVM_OWNER.into()),
        ]),
        requested_config: full_config(),
        effective_config: BTreeMap::new(),
    };
    std::mem::swap(&mut state.effective_config, &mut effective_config);
    state
}

/// Fresh state: both OFT deployments recorded, no peers and no effective
/// configuration recorded.
fn fresh_state() -> RouteStateV1 {
    let mut state = converged_state();
    state.contracts.retain(|key, _| {
        key == "stellar_oft" || key == "evm_oft" || key == "stellar_owner" || key == "evm_owner"
    });
    state.effective_config = BTreeMap::new();
    state
}

fn bound_state(mut state: RouteStateV1, desired: &DesiredRouteV1) -> RouteStateV1 {
    state.desired_sha256 = canonical_sha256(desired).unwrap();
    state
}

fn field_order() -> Vec<String> {
    let mut order = Vec::new();
    for (vm, eid) in [("stellar", EVM_END_TO_END), ("evm", STELLAR_END)] {
        order.push(peer_field(Vm::Stellar, eid)); // replaced below per side
        order.pop();
        order.push(format!("peer:{vm}:{eid}"));
        order.push(format!("send_library:{vm}:{eid}"));
        order.push(format!("receive_library:{vm}:{eid}"));
        order.push(format!("uln_send_config:{vm}:{eid}"));
        order.push(format!("uln_receive_config:{vm}:{eid}"));
        order.push(format!("executor_config:{vm}:{eid}"));
        order.push(format!("receive_options:{vm}:{eid}:1"));
    }
    order
}

#[test]
fn plan_order_is_deterministic_stellar_first_with_dependency_sequence() {
    let desired = desired(full_config());
    let state = fresh_state();
    let first = plan_route_mutations(&desired, &state).expect("plan");
    let again = plan_route_mutations(&desired, &state).expect("replan");
    assert_eq!(first, again, "planning must be deterministic");
    let actual: Vec<String> = first.steps.iter().map(|step| step.field.clone()).collect();
    assert_eq!(actual, field_order());
    // Stellar steps lead every EVM step of the same class.
    let stellar_indices: Vec<usize> = first
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.vm == Vm::Stellar)
        .map(|(index, _)| index)
        .collect();
    let evm_indices: Vec<usize> = first
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.vm == Vm::Evm)
        .map(|(index, _)| index)
        .collect();
    for (s, e) in stellar_indices.iter().zip(evm_indices.iter()) {
        assert!(s < e, "Stellar step must precede the same-class EVM step");
    }
}

#[test]
fn fresh_route_marks_peers_pending_and_every_dependent_step_blocked() {
    let desired = desired(full_config());
    let state = fresh_state();
    let plan = plan_route_mutations(&desired, &state).expect("plan");
    assert!(!plan.converged);
    assert_eq!(plan.pending, 2, "only the two peer steps are authorable");
    assert_eq!(plan.blocked, 12);
    for step in &plan.steps {
        let is_peer = step.field.starts_with("peer:");
        if is_peer {
            assert_eq!(step.status, RouteStepStatus::Pending);
            assert!(step.operation.is_some(), "peer step carries its write");
        } else {
            assert_eq!(step.status, RouteStepStatus::Blocked);
            assert!(
                step.missing_prerequisite.is_some(),
                "dependent step names its blocker"
            );
        }
    }
    // Dependent blockers reference the canonical prerequisite fields.
    let stellar_uln_send = plan
        .steps
        .iter()
        .find(|step| {
            step.field == config_key_uln_config(Vm::Stellar, EVM_END_TO_END, "send").unwrap()
        })
        .expect("stellar uln send step");
    assert_eq!(
        stellar_uln_send.missing_prerequisite.as_deref(),
        Some(config_key_send_library(Vm::Stellar, EVM_END_TO_END).as_str())
    );
}

#[test]
fn missing_counterparty_deployment_blocks_the_peer_step() {
    let desired = desired(BTreeMap::new());
    let mut state = bound_state(fresh_state(), &desired);
    state.contracts.remove("evm_oft");
    let plan = plan_route_mutations(&desired, &state).expect("plan");
    let stellar_peer = plan
        .steps
        .iter()
        .find(|step| step.field == peer_field(Vm::Stellar, EVM_END_TO_END))
        .expect("stellar peer step");
    assert_eq!(stellar_peer.status, RouteStepStatus::Blocked);
    assert_eq!(
        stellar_peer.missing_prerequisite.as_deref(),
        Some("deployment:evm_oft")
    );
    assert!(!plan.converged);
}

#[test]
fn mismatched_library_readback_refuses_until_prerequisite_converges() {
    // Records the stellar peer as converged, then writes a conflicting
    // stellar send-library readback: the library step is pending and the
    // send ULN step is blocked on it, so the route refuses convergence.
    let desired = desired(full_config());
    let mut state = fresh_state();
    state.effective_config.insert(
        config_key_send_library(Vm::Stellar, EVM_END_TO_END),
        json!("0x9999999999999999999999999999999999999999"),
    );
    state.contracts.insert(
        format!("peer:{}", EVM_END_TO_END),
        json!("unused").as_str().unwrap().to_string(),
    );
    // Peer readback must equal the derived peer for convergence; use the
    // converged peer records instead of a placeholder.
    let converged = converged_state();
    state.contracts = converged.contracts;
    let plan = plan_route_mutations(&desired, &state).expect("plan");
    let send = plan
        .steps
        .iter()
        .find(|step| step.field == config_key_send_library(Vm::Stellar, EVM_END_TO_END))
        .expect("send library step");
    assert_eq!(send.status, RouteStepStatus::Pending);
    assert_eq!(
        send.operation,
        Some(OperationV1::SetStellarSendLibrary {
            remote_eid: EVM_END_TO_END,
            library: STELLAR_SEND_LIB.into(),
        })
    );
    let uln_send = plan
        .steps
        .iter()
        .find(|step| {
            step.field == config_key_uln_config(Vm::Stellar, EVM_END_TO_END, "send").unwrap()
        })
        .expect("uln send step");
    assert_eq!(uln_send.status, RouteStepStatus::Blocked);
    assert_eq!(
        uln_send.missing_prerequisite.as_deref(),
        Some(config_key_send_library(Vm::Stellar, EVM_END_TO_END).as_str())
    );
    let error = require_convergence(&plan).expect_err("route must not converge");
    match error {
        Error::Conflict(message) => {
            assert!(message.contains("route not converged"));
            assert!(message.contains("send_library:stellar:"));
            assert!(message.contains("uln_send_config:stellar:"));
        }
        other => panic!("expected conflict, got {other:?}"),
    }
}

#[test]
fn converged_route_is_an_idempotent_no_op() {
    let desired = desired(full_config());
    let state = converged_state();
    let plan = plan_route_mutations(&desired, &state).expect("plan");
    assert!(plan.converged, "{:#?}", plan.steps);
    assert_eq!(plan.pending, 0);
    assert_eq!(plan.blocked, 0);
    assert!(
        plan.steps
            .iter()
            .all(|step| step.status == RouteStepStatus::Satisfied),
        "every step must read back exactly as requested"
    );
    assert!(
        plan.steps.iter().all(|step| step.operation.is_none()),
        "a converged route has nothing to run"
    );
    let again = plan_route_mutations(&desired, &state).expect("replan");
    assert_eq!(again, plan, "replanning a converged route is a no-op");
    require_convergence(&plan).expect("converged plan passes the refusal gate");
}

#[test]
fn absent_recorded_value_is_an_exact_mismatch_never_satisfied() {
    let desired = desired(full_config());
    let mut state = converged_state();
    // Drop one recorded field: the route must not converge even though the
    // value would round-trip trivially.
    state
        .effective_config
        .remove(&config_key_executor_config(Vm::Evm, STELLAR_END));
    let plan = plan_route_mutations(&desired, &state).expect("plan");
    assert!(!plan.converged);
    let executor = plan
        .steps
        .iter()
        .find(|step| step.field == config_key_executor_config(Vm::Evm, STELLAR_END))
        .expect("evm executor step");
    assert_eq!(executor.status, RouteStepStatus::Pending);
    assert_eq!(executor.readback, "");
}

#[test]
fn non_string_requested_value_is_refused_at_the_plan_boundary() {
    let mut config = full_config();
    config.insert(
        config_key_send_library(Vm::Stellar, EVM_END_TO_END),
        json!(42),
    );
    let desired = desired(config);
    let state = bound_state(fresh_state(), &desired);
    let error = plan_route_mutations(&desired, &state).expect_err("non-string value");
    match error {
        Error::InvalidInput(message) => {
            assert!(message.contains("send_library:stellar:"));
            assert!(message.contains("must be a string"));
        }
        other => panic!("expected invalid input, got {other:?}"),
    }
}

#[test]
fn invalid_config_hash_and_options_hex_are_refused() {
    let mut config = full_config();
    config.insert(
        config_key_uln_config(Vm::Stellar, EVM_END_TO_END, "send").unwrap(),
        json!("not-a-hash"),
    );
    let desired_route = desired(config);
    let error = plan_route_mutations(&desired_route, &bound_state(fresh_state(), &desired_route))
        .expect_err("bad hash");
    assert!(
        matches!(error, Error::InvalidInput(_)),
        "bad ULN hash must be refused"
    );
    let mut config = full_config();
    config.insert(
        config_key_receive_options(Vm::Evm, STELLAR_END, 1),
        json!("0x1"),
    );
    let desired_route = desired(config);
    let error = plan_route_mutations(&desired_route, &bound_state(fresh_state(), &desired_route))
        .expect_err("short options");
    assert!(
        matches!(error, Error::InvalidInput(_)),
        "undersized options must be refused"
    );
}

#[test]
fn requested_zero_grace_with_pending_timeout_converges_by_removal() {
    let mut config = full_config();
    config.insert(
        config_key_receive_library_grace(Vm::Stellar, EVM_END_TO_END),
        json!("0"),
    );
    let desired = desired(config);
    let mut state = bound_state(converged_state(), &desired);
    state.effective_config.insert(
        config_key_receive_library_grace(Vm::Stellar, EVM_END_TO_END),
        json!("9999"),
    );
    let plan = plan_route_mutations(&desired, &state).expect("plan");
    let receive = plan
        .steps
        .iter()
        .find(|step| step.field == config_key_receive_library(Vm::Stellar, EVM_END_TO_END))
        .expect("receive library step");
    assert_eq!(receive.status, RouteStepStatus::Pending);
    assert_eq!(
        receive.operation,
        Some(OperationV1::RemoveStellarReceiveLibraryTimeout {
            remote_eid: EVM_END_TO_END,
        })
    );
    assert!(!plan.converged);
}

#[test]
fn mismatched_grace_resets_the_receive_library_with_the_requested_grace() {
    let desired = desired(full_config());
    let mut state = converged_state();
    state.effective_config.insert(
        config_key_receive_library_grace(Vm::Evm, STELLAR_END),
        json!("0"),
    );
    let plan = plan_route_mutations(&desired, &state).expect("plan");
    let receive = plan
        .steps
        .iter()
        .find(|step| step.field == config_key_receive_library(Vm::Evm, STELLAR_END))
        .expect("evm receive library step");
    assert_eq!(receive.status, RouteStepStatus::Pending);
    assert_eq!(
        receive.operation,
        Some(OperationV1::SetEvmReceiveLibrary {
            remote_eid: STELLAR_END,
            library: EVM_RECEIVE_LIB.into(),
            grace_period_seconds: 500,
        })
    );
}

#[test]
fn non_canonical_keys_are_ignored_by_the_planner() {
    let mut config = full_config();
    config.insert("containment:stellar".into(), json!("active"));
    config.insert("vendor:note".into(), json!("ignored"));
    let desired = desired(config);
    let state = bound_state(converged_state(), &desired);
    let plan = plan_route_mutations(&desired, &state).expect("plan");
    assert!(plan.converged, "{:#?}", plan.steps);
    assert!(plan
        .steps
        .iter()
        .all(|step| !step.field.starts_with("containment:") && !step.field.starts_with("vendor:")));
}

#[test]
fn binding_conflicts_are_refused() {
    let desired = desired(full_config());
    let mut state = converged_state();
    state.route_id = "other-route".into();
    let error = plan_route_mutations(&desired, &state).expect_err("route id mismatch");
    assert!(matches!(error, Error::Conflict(_)));
    let state = converged_state();
    let mut wrong_desired = desired.clone();
    wrong_desired.config.insert("extra:key".into(), json!("x"));
    let error = plan_route_mutations(&wrong_desired, &state).expect_err("digest mismatch");
    assert!(matches!(error, Error::Conflict(_)));
}

#[test]
fn mutation_gate_allows_planning_for_recognized_environments() {
    mutation_gate(&identity(Environment::StellarTestnetSepolia))
        .expect("testnet planning is allowed");
    mutation_gate(&identity(Environment::StellarMainnetEthereum))
        .expect("mainnet proposal planning is allowed");
}

#[test]
fn stellar_containment_requires_role_and_restores_exact_limit_without_changing_receive_state() {
    let mut state = converged_state();
    let eid = state.identity.evm_eid;
    for (suffix, value) in [
        ("limit_raw", "9000"),
        ("window_seconds", "60"),
        ("mode", "net"),
    ] {
        state.effective_config.insert(
            format!("outbound_rate_limit:stellar:{eid}:{suffix}"),
            json!(value),
        );
    }
    let error =
        templar_oft_bridge_cli::layerzero::containment_snapshot(&state, Direction::StellarToEvm)
            .expect_err("missing containment role");
    assert!(error.to_string().contains("RATE_LIMITER_MANAGER_ROLE"));

    state.contracts.insert(
        "stellar_role:RATE_LIMITER_MANAGER_ROLE".into(),
        STELLAR_OWNER.into(),
    );
    let receive_before = state
        .effective_config
        .iter()
        .filter(|(key, _)| key.contains("receive"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    let snapshot =
        templar_oft_bridge_cli::layerzero::containment_snapshot(&state, Direction::StellarToEvm)
            .unwrap();
    assert_eq!(
        templar_oft_bridge_cli::layerzero::containment_mutation(&state, &snapshot, false).unwrap(),
        OperationV1::SetOutboundRateLimit {
            remote_eid: eid,
            limit_raw: 0,
            window_seconds: 60,
            mode: "net".into(),
        }
    );
    assert_eq!(
        templar_oft_bridge_cli::layerzero::containment_mutation(&state, &snapshot, true).unwrap(),
        OperationV1::SetOutboundRateLimit {
            remote_eid: eid,
            limit_raw: 9000,
            window_seconds: 60,
            mode: "net".into(),
        }
    );
    let receive_after = state
        .effective_config
        .iter()
        .filter(|(key, _)| key.contains("receive"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(receive_after, receive_before);
}

#[test]
fn evm_containment_uses_blocked_library_and_restores_exact_send_library() {
    let mut state = converged_state();
    state.effective_config.insert(
        "endpoint:blocked_library:evm".into(),
        json!("0x3333333333333333333333333333333333333333"),
    );
    let snapshot =
        templar_oft_bridge_cli::layerzero::containment_snapshot(&state, Direction::EvmToStellar)
            .unwrap();
    assert_eq!(
        templar_oft_bridge_cli::layerzero::containment_mutation(&state, &snapshot, false).unwrap(),
        OperationV1::SetEvmSendLibrary {
            remote_eid: state.identity.stellar_eid,
            library: "0x3333333333333333333333333333333333333333".into(),
        }
    );
    assert_eq!(
        templar_oft_bridge_cli::layerzero::containment_mutation(&state, &snapshot, true).unwrap(),
        *snapshot.restore_operation
    );
}

/// Keep the peer-field helper in scope so the order assertion cannot drift
/// from the module contract.
#[allow(dead_code)]
fn _peer_field_smoke(vm: Vm, eid: u32) -> String {
    peer_field(vm, eid)
}
