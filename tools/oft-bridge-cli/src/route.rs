//! Route mutation DAG and exact readback convergence.
//!
//! Owns the deterministic ordering, prerequisite enforcement, and exact
//! readback comparison for route configuration writes. Pure decisions over
//! typed inputs — no live chain mutation. The recorded [`RouteStateV1`]
//! snapshot is the readback oracle: callers that observe on-chain state fold
//! the observations into `effective_config` and `contracts` (the same
//! convention as [`crate::layerzero::compare_routes`]) and then re-plan.
//! Re-planning a fully converged route is an idempotent no-op: zero pending
//! steps and the same plan on every re-run.
//!
//! Canonical route-config keys owned by this module (values are strings):
//!
//! | key | value |
//! |---|---|
//! | `send_library:{vm}:{eid}` | send library address |
//! | `receive_library:{vm}:{eid}` | receive library address |
//! | `receive_library_grace:{vm}:{eid}` | receive-library grace seconds; optional |
//! | `uln_send_config:{vm}:{eid}` | SHA-256 of the send ULN config document |
//! | `uln_receive_config:{vm}:{eid}` | SHA-256 of the receive ULN config document |
//! | `executor_config:{vm}:{eid}` | SHA-256 of the executor config document |
//! | `receive_options:{vm}:{eid}:{msg_type}` | Type-3 enforced options hex (types 1 and 2) |
//!
//! Peers are not config keys: the requested peer value is derived from the
//! recorded counterparty OFT deployment (`contracts["evm_oft"]` /
//! `contracts["stellar_oft"]`), matching the peer operations emitted by
//! [`crate::wrap::plan_wrap`]; the readback is the recorded
//! `contracts["peer:{eid}"]` entry. A missing counterparty deployment blocks
//! the peer step as an unsatisfied prerequisite.
//!
//! Steps are ordered deterministically: Stellar before EVM, and within a VM
//! peer → send library → receive library → send ULN → receive ULN → executor
//! → receive options. A step whose prerequisite was requested but is not yet
//! converged is `Blocked`, never planned onto the stale value. A prerequisite
//! that was not requested does not block (the operator accepts the recorded
//! effective value for that field). Keys outside this scheme are ignored;
//! generic drift across every key is reported by
//! [`crate::layerzero::compare_routes`], not by this planner.
//!
//! Route configuration is testnet-only in v1: [`mutation_gate`] refuses
//! mainnet-classified identities with `production_mutation_unsupported_v1`
//! before any route write is planned onto a live chain.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::domain::{ChainIdentityV1, DesiredRouteV1, OperationV1, RouteStateV1, Vm};
use crate::error::{Error, Result};

/// Status of one planned route-configuration step. Only converged steps are
/// `Satisfied`; anything else keeps a route non-converged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteStepStatus {
    /// Recorded effective state exactly equals the requested value; nothing
    /// to run.
    Satisfied,
    /// Recorded effective state differs from the requested value and every
    /// prerequisite is converged; the step must run.
    Pending,
    /// Recorded effective state differs but a requested prerequisite is not
    /// converged (or the counterparty deployment is missing); the step must
    /// wait for the prerequisite before it can run.
    Blocked,
}

/// One deterministic step of a route mutation plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RouteStepV1 {
    /// Canonical config key this step converges (see module docs).
    pub field: String,
    pub vm: Vm,
    pub remote_eid: u32,
    /// `send`/`receive` for ULN steps; `None` otherwise.
    #[serde(default)]
    pub direction: Option<String>,
    /// The exact operation to run once unblocked. `None` for a converged
    /// no-op or when the operation cannot be authored yet (a peer awaiting
    /// its counterparty deployment record).
    pub operation: Option<OperationV1>,
    /// Requested value (derived or from the desired route config).
    pub requested: String,
    /// Recorded effective value (the readback).
    pub readback: String,
    pub status: RouteStepStatus,
    /// Canonical field whose non-convergence blocks this step.
    #[serde(default)]
    pub missing_prerequisite: Option<String>,
}

/// Deterministic route mutation plan: every requested route-config field in
/// dependency order with exact readback comparison.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RouteMutationPlanV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub desired_sha256: String,
    pub steps: Vec<RouteStepV1>,
    pub pending: usize,
    pub blocked: usize,
    pub converged: bool,
}

/// Canonical VM label used inside config keys.
pub fn vm_label(vm: Vm) -> &'static str {
    match vm {
        Vm::Stellar => "stellar",
        Vm::Evm => "evm",
    }
}

/// Canonical peer field for a VM. The recorded value lives in
/// `contracts["peer:{remote_eid}"]`; the requested value is derived from the
/// recorded counterparty OFT deployment.
pub fn peer_field(vm: Vm, remote_eid: u32) -> String {
    format!("peer:{}:{}", vm_label(vm), remote_eid)
}

/// Canonical config key for a send-library write.
pub fn config_key_send_library(vm: Vm, remote_eid: u32) -> String {
    format!("send_library:{}:{}", vm_label(vm), remote_eid)
}

/// Canonical config key for a receive-library write.
pub fn config_key_receive_library(vm: Vm, remote_eid: u32) -> String {
    format!("receive_library:{}:{}", vm_label(vm), remote_eid)
}

/// Canonical config key for a receive-library grace period (seconds).
pub fn config_key_receive_library_grace(vm: Vm, remote_eid: u32) -> String {
    format!("receive_library_grace:{}:{}", vm_label(vm), remote_eid)
}

/// Canonical config key for a ULN config document hash; `direction` must be
/// `send` or `receive`.
pub fn config_key_uln_config(vm: Vm, remote_eid: u32, direction: &str) -> Result<String> {
    match direction {
        "send" | "receive" => Ok(format!(
            "uln_{direction}_config:{}:{}",
            vm_label(vm),
            remote_eid
        )),
        other => Err(Error::InvalidInput(format!(
            "uln direction must be send or receive, got {other}"
        ))),
    }
}

/// Canonical config key for an executor config document hash.
pub fn config_key_executor_config(vm: Vm, remote_eid: u32) -> String {
    format!("executor_config:{}:{}", vm_label(vm), remote_eid)
}

/// Canonical config key for enforced Type-3 receive options.
pub fn config_key_receive_options(vm: Vm, remote_eid: u32, message_type: u16) -> String {
    format!(
        "receive_options:{}:{}:{}",
        vm_label(vm),
        remote_eid,
        message_type
    )
}

/// The remote EID a VM writes route configuration for: the counterparty side.
fn remote_eid(vm: Vm, identity: &ChainIdentityV1) -> u32 {
    match vm {
        Vm::Stellar => identity.evm_eid,
        Vm::Evm => identity.stellar_eid,
    }
}

/// Reads a requested config value. Route-config fields are strings by
/// contract; any other JSON value is refused at the plan boundary.
fn requested_string(
    config: &BTreeMap<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>> {
    match config.get(key) {
        None => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        Some(other) => Err(Error::InvalidInput(format!(
            "route config field {key} must be a string, got {other}"
        ))),
    }
}

/// Reads a recorded effective value; missing or non-string records read back
/// as empty (an exact mismatch against any requested non-empty value).
fn recorded_string(config: &BTreeMap<String, serde_json::Value>, key: &str) -> String {
    config
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Validates enforced Type-3 options hex with the same minimum payload rule
/// as the calldata encoder (worker id plus size).
fn require_options_hex(key: &str, value: &str) -> Result<()> {
    let hex_body = value.strip_prefix("0x").unwrap_or(value);
    let decoded = hex::decode(hex_body)
        .map_err(|_| Error::InvalidInput(format!("route config field {key} is not hex")))?;
    if decoded.len() < 2 {
        return Err(Error::InvalidInput(format!(
            "route config field {key} must carry at least a worker id and size"
        )));
    }
    Ok(())
}

/// A requested prerequisite that is not yet converged, or `None` when the
/// step may run. A prerequisite that was never requested does not block: the
/// operator accepts the recorded effective value for that field.
fn blocked_by(
    prereq_field: &str,
    planned: &BTreeSet<String>,
    steps: &[RouteStepV1],
) -> Option<String> {
    if !planned.contains(prereq_field) {
        return None;
    }
    let prereq_satisfied = steps
        .iter()
        .find(|step| step.field == prereq_field)
        .is_some_and(|step| step.status == RouteStepStatus::Satisfied);
    if prereq_satisfied {
        None
    } else {
        Some(prereq_field.to_string())
    }
}

/// Classifies a step from exact readback and prerequisite state.
fn step_status(satisfied: bool, missing_prerequisite: Option<String>) -> RouteStepStatus {
    if satisfied {
        RouteStepStatus::Satisfied
    } else if missing_prerequisite.is_some() {
        RouteStepStatus::Blocked
    } else {
        RouteStepStatus::Pending
    }
}

fn peer_operation(vm: Vm, remote_eid: u32, peer: String) -> OperationV1 {
    match vm {
        Vm::Stellar => OperationV1::SetStellarPeer { remote_eid, peer },
        Vm::Evm => OperationV1::SetEvmPeer { remote_eid, peer },
    }
}

/// Builds the always-planned peer step. The requested peer is the recorded
/// counterparty OFT in canonical LayerZero bytes32 form; a missing
/// counterparty deployment is a hard prerequisite that blocks the step.
fn build_peer_step(vm: Vm, remote_eid: u32, state: &RouteStateV1) -> Result<RouteStepV1> {
    let (requested, deployment) = match vm {
        Vm::Stellar => (
            state
                .contracts
                .get("evm_oft")
                .map(|address| {
                    crate::codec::evm_address_to_bytes32(address)
                        .map(|bytes| format!("0x{}", hex::encode(bytes)))
                })
                .transpose()?,
            "deployment:evm_oft",
        ),
        Vm::Evm => (
            state
                .contracts
                .get("stellar_oft")
                .map(|contract| {
                    crate::codec::strkey_to_bytes32(contract)
                        .map(|bytes| format!("0x{}", hex::encode(bytes)))
                })
                .transpose()?,
            "deployment:stellar_oft",
        ),
    };
    let field = peer_field(vm, remote_eid);
    let readback = state
        .contracts
        .get(format!("peer:{remote_eid}").as_str())
        .cloned()
        .unwrap_or_default();
    let satisfied = requested.as_ref().is_some_and(|peer| peer == &readback);
    let missing_prerequisite = (!satisfied && requested.is_none()).then(|| deployment.to_string());
    let operation = match (&requested, satisfied) {
        (Some(peer), false) => Some(peer_operation(vm, remote_eid, peer.clone())),
        _ => None,
    };
    Ok(RouteStepV1 {
        field,
        vm,
        remote_eid,
        direction: None,
        operation,
        requested: requested.unwrap_or_default(),
        readback,
        status: step_status(satisfied, missing_prerequisite.clone()),
        missing_prerequisite,
    })
}

fn send_library_operation(vm: Vm, remote_eid: u32, library: String) -> OperationV1 {
    match vm {
        Vm::Stellar => OperationV1::SetStellarSendLibrary {
            remote_eid,
            library,
        },
        Vm::Evm => OperationV1::SetEvmSendLibrary {
            remote_eid,
            library,
        },
    }
}

fn receive_library_operation(
    vm: Vm,
    remote_eid: u32,
    library: String,
    grace_period_seconds: u64,
) -> OperationV1 {
    match vm {
        Vm::Stellar => OperationV1::SetStellarReceiveLibrary {
            remote_eid,
            library,
            grace_period_seconds,
        },
        Vm::Evm => OperationV1::SetEvmReceiveLibrary {
            remote_eid,
            library,
            grace_period_seconds,
        },
    }
}

fn remove_timeout_operation(vm: Vm, remote_eid: u32) -> OperationV1 {
    match vm {
        Vm::Stellar => OperationV1::RemoveStellarReceiveLibraryTimeout { remote_eid },
        Vm::Evm => OperationV1::RemoveEvmReceiveLibraryTimeout { remote_eid },
    }
}

/// Builds a send-library step. A requested but non-converged peer blocks it.
fn build_send_library_step(
    vm: Vm,
    remote_eid: u32,
    requested: String,
    planned: &BTreeSet<String>,
    steps: &[RouteStepV1],
    state: &RouteStateV1,
) -> Result<RouteStepV1> {
    let field = config_key_send_library(vm, remote_eid);
    let readback = recorded_string(&state.effective_config, &field);
    let converged = readback == requested;
    let prereq = peer_field(vm, remote_eid);
    let missing_prerequisite = if converged {
        None
    } else {
        blocked_by(&prereq, planned, steps)
    };
    Ok(RouteStepV1 {
        field,
        vm,
        remote_eid,
        direction: None,
        operation: (!converged).then(|| send_library_operation(vm, remote_eid, requested.clone())),
        requested,
        readback,
        status: step_status(converged, missing_prerequisite.clone()),
        missing_prerequisite,
    })
}

/// Builds a receive-library step, folding the optional grace period. A
/// requested grace of `0` with a recorded pending timeout converges through
/// the remove-timeout operation; an absent or malformed recorded grace reads
/// back as `0` (no pending timeout to read back).
fn build_receive_library_step(
    vm: Vm,
    remote_eid: u32,
    requested_library: String,
    requested_grace: String,
    planned: &BTreeSet<String>,
    steps: &[RouteStepV1],
    state: &RouteStateV1,
) -> Result<RouteStepV1> {
    let field = config_key_receive_library(vm, remote_eid);
    let grace_field = config_key_receive_library_grace(vm, remote_eid);
    let readback = recorded_string(&state.effective_config, &field);
    let readback_grace = state
        .effective_config
        .get(&grace_field)
        .and_then(|value| value.as_str())
        .unwrap_or("0")
        .to_string();
    let grace: u64 = requested_grace.parse().map_err(|_| {
        Error::InvalidInput(format!(
            "route config field {grace_field} must be a decimal string"
        ))
    })?;
    let library_equal = readback == requested_library;
    let grace_equal = requested_grace == readback_grace;
    let prereq = peer_field(vm, remote_eid);
    let missing_prerequisite = if library_equal && grace_equal {
        None
    } else {
        blocked_by(&prereq, planned, steps)
    };
    let operation = if library_equal && grace_equal {
        None
    } else if requested_grace == "0" && library_equal {
        Some(remove_timeout_operation(vm, remote_eid))
    } else {
        Some(receive_library_operation(
            vm,
            remote_eid,
            requested_library.clone(),
            grace,
        ))
    };
    Ok(RouteStepV1 {
        field,
        vm,
        remote_eid,
        direction: None,
        operation,
        requested: requested_library,
        readback,
        status: step_status(library_equal && grace_equal, missing_prerequisite.clone()),
        missing_prerequisite,
    })
}

fn uln_operation(
    vm: Vm,
    remote_eid: u32,
    direction: String,
    caller: String,
    oapp: String,
    library: String,
    config_sha256: String,
    config: serde_json::Value,
) -> OperationV1 {
    match vm {
        Vm::Stellar => OperationV1::SetStellarUlnConfig {
            remote_eid,
            direction,
            caller,
            oapp,
            library,
            config_sha256,
            config,
        },
        Vm::Evm => OperationV1::SetEvmUlnConfig {
            remote_eid,
            direction,
            caller,
            oapp,
            library,
            config_sha256,
            config,
        },
    }
}

fn executor_operation(
    vm: Vm,
    remote_eid: u32,
    caller: String,
    oapp: String,
    library: String,
    config_sha256: String,
    config: serde_json::Value,
) -> OperationV1 {
    match vm {
        Vm::Stellar => OperationV1::SetStellarExecutorConfig {
            remote_eid,
            caller,
            oapp,
            library,
            config_sha256,
            config,
        },
        Vm::Evm => OperationV1::SetEvmExecutorConfig {
            remote_eid,
            caller,
            oapp,
            library,
            config_sha256,
            config,
        },
    }
}

fn options_operation(vm: Vm, remote_eid: u32, message_type: u16, options: String) -> OperationV1 {
    match vm {
        Vm::Stellar => OperationV1::SetStellarReceiveOptions {
            remote_eid,
            message_type,
            options,
        },
        Vm::Evm => OperationV1::SetEvmReceiveOptions {
            remote_eid,
            message_type,
            options,
        },
    }
}

/// Builds a ULN config-hash step. The config targets the direction's
/// effective library, so a requested but non-converged library blocks it.
fn build_uln_step(
    vm: Vm,
    remote_eid: u32,
    direction: &str,
    requested: String,
    config: serde_json::Value,
    planned: &BTreeSet<String>,
    steps: &[RouteStepV1],
    state: &RouteStateV1,
) -> Result<RouteStepV1> {
    let field = config_key_uln_config(vm, remote_eid, direction)?;
    let readback = recorded_string(&state.effective_config, &field);
    let converged = readback == requested;
    let prereq = match direction {
        "send" => config_key_send_library(vm, remote_eid),
        _ => config_key_receive_library(vm, remote_eid),
    };
    let missing_prerequisite = (!converged)
        .then(|| blocked_by(&prereq, planned, steps))
        .flatten();
    let contract = |stellar: &str, evm: &str| {
        state
            .contracts
            .get(match vm {
                Vm::Stellar => stellar,
                Vm::Evm => evm,
            })
            .cloned()
            .ok_or_else(|| {
                Error::Custody(format!("route contract is not recorded: {stellar}/{evm}"))
            })
    };
    let operation = if converged {
        None
    } else {
        Some(uln_operation(
            vm,
            remote_eid,
            direction.to_string(),
            contract("stellar_owner", "evm_owner")?,
            contract("stellar_oft", "evm_oft")?,
            requested_string(&state.requested_config, &prereq)?
                .ok_or_else(|| Error::Custody("route message library is not requested".into()))?,
            requested.clone(),
            config,
        ))
    };
    Ok(RouteStepV1 {
        field,
        vm,
        remote_eid,
        direction: Some(direction.to_string()),
        operation,
        requested,
        readback,
        status: step_status(converged, missing_prerequisite.clone()),
        missing_prerequisite,
    })
}

/// Builds an executor config step. The executor delivers the receive path, so
/// a requested but non-converged receive library blocks it.
fn build_executor_step(
    vm: Vm,
    remote_eid: u32,
    requested: String,
    config: serde_json::Value,
    planned: &BTreeSet<String>,
    steps: &[RouteStepV1],
    state: &RouteStateV1,
) -> Result<RouteStepV1> {
    let field = config_key_executor_config(vm, remote_eid);
    let readback = recorded_string(&state.effective_config, &field);
    let converged = readback == requested;
    let prereq = config_key_receive_library(vm, remote_eid);
    let missing_prerequisite = (!converged)
        .then(|| blocked_by(&prereq, planned, steps))
        .flatten();
    let contract = |stellar: &str, evm: &str| {
        state
            .contracts
            .get(match vm {
                Vm::Stellar => stellar,
                Vm::Evm => evm,
            })
            .cloned()
            .ok_or_else(|| {
                Error::Custody(format!("route contract is not recorded: {stellar}/{evm}"))
            })
    };
    let operation = if converged {
        None
    } else {
        Some(executor_operation(
            vm,
            remote_eid,
            contract("stellar_owner", "evm_owner")?,
            contract("stellar_oft", "evm_oft")?,
            requested_string(&state.requested_config, &prereq)?
                .ok_or_else(|| Error::Custody("route receive library is not requested".into()))?,
            requested.clone(),
            config,
        ))
    };
    Ok(RouteStepV1 {
        field,
        vm,
        remote_eid,
        direction: None,
        operation,
        requested,
        readback,
        status: step_status(converged, missing_prerequisite.clone()),
        missing_prerequisite,
    })
}

/// Builds an enforced Type-3 options step. Options are consumed by the
/// executor, so a requested but non-converged executor config blocks them.
fn build_options_step(
    vm: Vm,
    remote_eid: u32,
    message_type: u16,
    requested: String,
    planned: &BTreeSet<String>,
    steps: &[RouteStepV1],
    state: &RouteStateV1,
) -> Result<RouteStepV1> {
    let field = config_key_receive_options(vm, remote_eid, message_type);
    let readback = recorded_string(&state.effective_config, &field);
    let converged = readback == requested;
    let prereq = config_key_executor_config(vm, remote_eid);
    let missing_prerequisite = if converged {
        None
    } else {
        blocked_by(&prereq, planned, steps)
    };
    Ok(RouteStepV1 {
        field,
        vm,
        remote_eid,
        direction: None,
        operation: (!converged)
            .then(|| options_operation(vm, remote_eid, message_type, requested.clone())),
        requested,
        readback,
        status: step_status(converged, missing_prerequisite.clone()),
        missing_prerequisite,
    })
}

/// Plans every route-configuration mutation for one VM in dependency order.
fn plan_vm_steps(
    vm: Vm,
    desired: &DesiredRouteV1,
    state: &RouteStateV1,
    planned: &BTreeSet<String>,
    steps: &mut Vec<RouteStepV1>,
) -> Result<()> {
    let remote = remote_eid(vm, &desired.identity);
    steps.push(build_peer_step(vm, remote, state)?);
    if let Some(requested) =
        requested_string(&desired.config, &config_key_send_library(vm, remote))?
    {
        let step = build_send_library_step(vm, remote, requested, planned, steps, state)?;
        steps.push(step);
    }
    if let Some(requested_library) =
        requested_string(&desired.config, &config_key_receive_library(vm, remote))?
    {
        let requested_grace = requested_string(
            &desired.config,
            &config_key_receive_library_grace(vm, remote),
        )?
        .unwrap_or_else(|| "0".to_string());
        let step = build_receive_library_step(
            vm,
            remote,
            requested_library,
            requested_grace,
            planned,
            steps,
            state,
        )?;
        steps.push(step);
    }
    for direction in ["send", "receive"] {
        let key = config_key_uln_config(vm, remote, direction)?;
        if let Some(config) = desired.config.get(&key).cloned() {
            let typed: crate::layerzero::UlnConfigType3V1 = serde_json::from_value(config.clone())
                .map_err(|error| {
                    Error::InvalidInput(format!("{key} is not a typed ULN config: {error}"))
                })?;
            typed.validate()?;
            let requested = typed.config_sha256()?;
            let step = build_uln_step(
                vm, remote, direction, requested, config, planned, steps, state,
            )?;
            steps.push(step);
        }
    }
    let executor_key = config_key_executor_config(vm, remote);
    if let Some(config) = desired.config.get(&executor_key).cloned() {
        let typed: crate::layerzero::ExecutorConfigType3V1 = serde_json::from_value(config.clone())
            .map_err(|error| {
                Error::InvalidInput(format!(
                    "{executor_key} is not a typed executor config: {error}"
                ))
            })?;
        typed.validate()?;
        let requested = typed.config_sha256()?;
        let step = build_executor_step(vm, remote, requested, config, planned, steps, state)?;
        steps.push(step);
    }
    for message_type in [1u16, 2u16] {
        let key = config_key_receive_options(vm, remote, message_type);
        if let Some(requested) = requested_string(&desired.config, &key)? {
            require_options_hex(&key, &requested)?;
            let step =
                build_options_step(vm, remote, message_type, requested, planned, steps, state)?;
            steps.push(step);
        }
    }
    Ok(())
}

/// Plans the route-configuration mutations that converge the recorded
/// effective state to the requested route. Deterministic: Stellar steps
/// first, then EVM; within a VM the fixed dependency order (peer → send
/// library → receive library → send ULN → receive ULN → executor → receive
/// options). The plan never writes; the caller drives each non-satisfied
/// step through the existing granular command/proposal path and replans
/// after readback.
pub fn plan_route_mutations(
    desired: &DesiredRouteV1,
    state: &RouteStateV1,
) -> Result<RouteMutationPlanV1> {
    if desired.route_id != state.route_id {
        return Err(Error::Conflict(format!(
            "route state {} does not match desired route {}",
            state.route_id, desired.route_id
        )));
    }
    let desired_sha256 = crate::canonical_sha256(desired)?;
    if desired_sha256 != state.desired_sha256 {
        return Err(Error::Conflict(
            "desired route does not bind to this route state".into(),
        ));
    }
    let mut planned = BTreeSet::new();
    for vm in [Vm::Stellar, Vm::Evm] {
        let remote = remote_eid(vm, &desired.identity);
        planned.insert(peer_field(vm, remote));
        for key in [
            config_key_send_library(vm, remote),
            config_key_receive_library(vm, remote),
            config_key_receive_library_grace(vm, remote),
            config_key_uln_config(vm, remote, "send")?,
            config_key_uln_config(vm, remote, "receive")?,
            config_key_executor_config(vm, remote),
            config_key_receive_options(vm, remote, 1),
            config_key_receive_options(vm, remote, 2),
        ] {
            if desired.config.contains_key(&key) {
                planned.insert(key);
            }
        }
    }
    let mut steps = Vec::new();
    plan_vm_steps(Vm::Stellar, desired, state, &planned, &mut steps)?;
    plan_vm_steps(Vm::Evm, desired, state, &planned, &mut steps)?;
    let pending = steps
        .iter()
        .filter(|step| step.status == RouteStepStatus::Pending)
        .count();
    let blocked = steps
        .iter()
        .filter(|step| step.status == RouteStepStatus::Blocked)
        .count();
    Ok(RouteMutationPlanV1 {
        schema_name: "route_mutation_plan".into(),
        schema_version: 1,
        route_id: desired.route_id.clone(),
        desired_sha256,
        steps,
        pending,
        blocked,
        converged: pending == 0 && blocked == 0,
    })
}

/// Mismatch refusal: a route that is not fully converged is refused with a
/// typed conflict naming every pending and blocked field. Callers gate
/// live-route operations (canaries, legs, health) on this before relying on
/// the requested configuration being effective.
pub fn require_convergence(plan: &RouteMutationPlanV1) -> Result<()> {
    if plan.converged {
        return Ok(());
    }
    let mut unmatched = Vec::new();
    for step in &plan.steps {
        if step.status == RouteStepStatus::Satisfied {
            continue;
        }
        let reason = match &step.missing_prerequisite {
            Some(prereq) => format!("{} blocked by {}", step.field, prereq),
            None => format!(
                "{} requested {:?} readback {:?}",
                step.field, step.requested, step.readback
            ),
        };
        unmatched.push(reason);
    }
    Err(Error::Conflict(format!(
        "route not converged: {}",
        unmatched.join("; ")
    )))
}

/// Testnet-only mutation gate for route configuration writes. V1 permits no
/// production mutation: a mainnet-classified identity is refused with
/// `production_mutation_unsupported_v1` before any write is planned.
pub fn mutation_gate(identity: &ChainIdentityV1) -> Result<()> {
    crate::environment::require_testnet(identity)
}

fn decode_evm_word_address(value: &[u8], label: &str) -> Result<String> {
    if value.len() < 32 {
        return Err(Error::Chain(format!("{label} returned a short EVM word")));
    }
    Ok(crate::evm::canonical_address(
        alloy::primitives::Address::from_slice(&value[value.len() - 20..]),
    ))
}

fn decode_evm_dynamic_bytes(value: &[u8], label: &str) -> Result<Vec<u8>> {
    if value.len() < 64 || value[..24].iter().any(|byte| *byte != 0) {
        return Err(Error::Chain(format!(
            "{label} returned malformed dynamic bytes"
        )));
    }
    let mut offset = [0u8; 8];
    offset.copy_from_slice(&value[24..32]);
    let offset = usize::from_be_bytes(offset);
    if offset.checked_add(32).is_none_or(|end| end > value.len())
        || value[offset..offset + 24].iter().any(|byte| *byte != 0)
    {
        return Err(Error::Chain(format!(
            "{label} returned an invalid bytes offset"
        )));
    }
    let mut length = [0u8; 8];
    length.copy_from_slice(&value[offset + 24..offset + 32]);
    let length = usize::from_be_bytes(length);
    let start = offset + 32;
    let end = start
        .checked_add(length)
        .filter(|end| *end <= value.len())
        .ok_or_else(|| Error::Chain(format!("{label} returned truncated bytes")))?;
    Ok(value[start..end].to_vec())
}

fn evm_call_data(signature: &str, words: impl IntoIterator<Item = [u8; 32]>) -> Vec<u8> {
    let mut data = crate::evm::keccak256_of(signature.as_bytes())[..4].to_vec();
    data.extend(words.into_iter().flatten());
    data
}

fn evm_address_word(value: &str) -> Result<[u8; 32]> {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(crate::evm::parse_address(value)?.as_slice());
    Ok(word)
}

fn evm_u32_word(value: u32) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[28..].copy_from_slice(&value.to_be_bytes());
    word
}

fn stellar_arg(value: stellar_baselib::xdr::ScVal) -> Result<String> {
    use stellar_baselib::xdr::{Limits, WriteXdr as _};
    value
        .to_xdr(Limits::none())
        .map(hex::encode)
        .map_err(|error| Error::Chain(format!("Stellar readback arg encoding failed: {error}")))
}

fn stellar_symbol_arg(value: &str) -> Result<stellar_baselib::xdr::ScVal> {
    use stellar_baselib::xdr::{ScSymbol, ScVal, StringM};
    Ok(ScVal::Symbol(ScSymbol(
        StringM::try_from(value.as_bytes().to_vec())
            .map_err(|error| Error::InvalidInput(format!("invalid Stellar symbol: {error}")))?,
    )))
}

fn stellar_address_result(value: &stellar_baselib::xdr::ScVal) -> Result<String> {
    use stellar_baselib::{
        address::{Address, AddressTrait as _},
        xdr::ScVal,
    };
    let ScVal::Address(address) = value else {
        return Err(Error::Chain(
            "Stellar readback returned a non-address".into(),
        ));
    };
    Address::from_sc_address(address)
        .map(|address| address.to_string())
        .map_err(|error| Error::Chain(format!("invalid Stellar readback address: {error}")))
}

fn stellar_resolved_library(value: &stellar_baselib::xdr::ScVal) -> Result<String> {
    use stellar_baselib::xdr::ScVal;
    let ScVal::Map(Some(map)) = value else {
        return Err(Error::Chain(
            "Stellar library readback returned a non-map".into(),
        ));
    };
    map.0
        .iter()
        .find_map(|entry| match &entry.key {
            ScVal::Symbol(symbol) if symbol.0.as_slice() == b"lib" => {
                Some(stellar_address_result(&entry.val))
            }
            _ => None,
        })
        .transpose()?
        .ok_or_else(|| Error::Chain("Stellar resolved library has no lib field".into()))
}

fn require_address(actual: &str, expected: &str, label: &str) -> Result<()> {
    let matches = if expected.starts_with("0x") {
        actual.eq_ignore_ascii_case(expected)
    } else {
        actual == expected
    };
    if matches {
        Ok(())
    } else {
        Err(Error::Conflict(format!(
            "{label} readback {actual} differs from requested {expected}"
        )))
    }
}

/// Reads the EVM EndpointV2 `blockedLibrary()` view with an exact eth_call
/// and returns the canonical address. Does not persist; callers fold the
/// value into state only after their own authoritative use.
pub fn read_evm_blocked_library(evm: &dyn crate::evm::EvmChain, endpoint: &str) -> Result<String> {
    let target = crate::evm::parse_address(endpoint)?;
    let value = crate::block_on_result(evm.call(target, evm_call_data("blockedLibrary()", [])))?;
    let address = decode_evm_word_address(&value, "blockedLibrary")?;
    if address == "0x0000000000000000000000000000000000000000" {
        return Err(Error::Conflict(
            "EndpointV2 blockedLibrary readback is zero".into(),
        ));
    }
    Ok(address)
}

/// Reads the just-mutated route field from its authoritative contract and
/// records it only after an exact match.
pub fn apply_live_readback(
    stellar: &dyn crate::stellar::StellarChain,
    evm: &dyn crate::evm::EvmChain,
    state: &mut RouteStateV1,
    operation: &OperationV1,
) -> Result<()> {
    use stellar_baselib::xdr::{ScBytes, ScVal};

    let stellar_source = state
        .contracts
        .get("stellar_owner")
        .ok_or_else(|| Error::Custody("route has no recorded stellar_owner".into()))?;
    let stellar_oft = state
        .contracts
        .get("stellar_oft")
        .ok_or_else(|| Error::Custody("route has no recorded stellar_oft".into()))?;
    let evm_oft = state
        .contracts
        .get("evm_oft")
        .ok_or_else(|| Error::Custody("route has no recorded evm_oft".into()))?;
    let evm_call = |to: &str, signature: &str, words: Vec<[u8; 32]>| {
        crate::block_on_result(evm.call(
            crate::evm::parse_address(to)?,
            evm_call_data(signature, words),
        ))
    };
    match operation {
        OperationV1::SetStellarPeer { remote_eid, peer } => {
            let value = stellar.invoke_view(
                stellar_oft,
                "peer",
                &[stellar_arg(ScVal::U32(*remote_eid))?],
                stellar_source,
            )?;
            let expected = if peer.starts_with("0x") {
                crate::codec::evm_address_to_bytes32(peer)?
            } else {
                crate::codec::strkey_to_bytes32(peer)?
            };
            let ScVal::Bytes(actual) = value else {
                return Err(Error::Conflict("Stellar peer readback is absent".into()));
            };
            if actual.0.as_slice() != expected {
                return Err(Error::Conflict("Stellar peer readback mismatch".into()));
            }
            state
                .contracts
                .insert(format!("peer:{remote_eid}"), peer.clone());
        }
        OperationV1::SetEvmPeer { remote_eid, peer } => {
            let value = evm_call(evm_oft, "peers(uint32)", vec![evm_u32_word(*remote_eid)])?;
            let expected = crate::codec::strkey_to_bytes32(peer)?;
            if value.as_slice() != expected {
                return Err(Error::Conflict("EVM peer readback mismatch".into()));
            }
            state
                .contracts
                .insert(format!("peer:{remote_eid}"), peer.clone());
        }
        OperationV1::SetStellarSendLibrary {
            remote_eid,
            library,
        }
        | OperationV1::SetStellarReceiveLibrary {
            remote_eid,
            library,
            ..
        } => {
            let receive = matches!(operation, OperationV1::SetStellarReceiveLibrary { .. });
            let actual = stellar.invoke_view(
                &state.identity.stellar_endpoint,
                if receive {
                    "get_receive_library"
                } else {
                    "get_send_library"
                },
                &[
                    stellar_arg(crate::layerzero::stellar_address(stellar_oft)?)?,
                    stellar_arg(ScVal::U32(*remote_eid))?,
                ],
                stellar_source,
            )?;
            require_address(
                &stellar_resolved_library(&actual)?,
                library,
                "Stellar library",
            )?;
            state.effective_config.insert(
                if receive {
                    config_key_receive_library(Vm::Stellar, *remote_eid)
                } else {
                    config_key_send_library(Vm::Stellar, *remote_eid)
                },
                serde_json::Value::String(library.clone()),
            );
            if let OperationV1::SetStellarReceiveLibrary {
                grace_period_seconds,
                ..
            } = operation
            {
                state.effective_config.insert(
                    config_key_receive_library_grace(Vm::Stellar, *remote_eid),
                    serde_json::Value::String(grace_period_seconds.to_string()),
                );
            }
        }
        OperationV1::SetEvmSendLibrary {
            remote_eid,
            library,
        }
        | OperationV1::SetEvmReceiveLibrary {
            remote_eid,
            library,
            ..
        } => {
            let receive = matches!(operation, OperationV1::SetEvmReceiveLibrary { .. });
            let signature = if receive {
                "getReceiveLibrary(address,uint32)"
            } else {
                "getSendLibrary(address,uint32)"
            };
            let value = evm_call(
                &state.identity.evm_endpoint,
                signature,
                vec![evm_address_word(evm_oft)?, evm_u32_word(*remote_eid)],
            )?;
            require_address(
                &decode_evm_word_address(&value[..32.min(value.len())], signature)?,
                library,
                "EVM library",
            )?;
            state.effective_config.insert(
                if receive {
                    config_key_receive_library(Vm::Evm, *remote_eid)
                } else {
                    config_key_send_library(Vm::Evm, *remote_eid)
                },
                serde_json::Value::String(library.clone()),
            );
            if let OperationV1::SetEvmReceiveLibrary {
                grace_period_seconds,
                ..
            } = operation
            {
                state.effective_config.insert(
                    config_key_receive_library_grace(Vm::Evm, *remote_eid),
                    serde_json::Value::String(grace_period_seconds.to_string()),
                );
            }
        }
        OperationV1::RemoveStellarReceiveLibraryTimeout { remote_eid } => {
            let value = stellar.invoke_view(
                &state.identity.stellar_endpoint,
                "receive_library_timeout",
                &[
                    stellar_arg(crate::layerzero::stellar_address(stellar_oft)?)?,
                    stellar_arg(ScVal::U32(*remote_eid))?,
                ],
                stellar_source,
            )?;
            if value != ScVal::Void {
                return Err(Error::Conflict(
                    "Stellar receive-library timeout still exists".into(),
                ));
            }
            state
                .effective_config
                .remove(&config_key_receive_library_grace(Vm::Stellar, *remote_eid));
        }
        OperationV1::RemoveEvmReceiveLibraryTimeout { remote_eid } => {
            let value = evm_call(
                &state.identity.evm_endpoint,
                "receiveLibraryTimeout(address,uint32)",
                vec![evm_address_word(evm_oft)?, evm_u32_word(*remote_eid)],
            )?;
            if value.iter().any(|byte| *byte != 0) {
                return Err(Error::Conflict(
                    "EVM receive-library timeout still exists".into(),
                ));
            }
            state
                .effective_config
                .remove(&config_key_receive_library_grace(Vm::Evm, *remote_eid));
        }
        OperationV1::SetStellarReceiveOptions {
            remote_eid,
            message_type,
            options,
        } => {
            let value = stellar.invoke_view(
                stellar_oft,
                "enforced_options",
                &[
                    stellar_arg(ScVal::U32(*remote_eid))?,
                    stellar_arg(ScVal::U32(u32::from(*message_type)))?,
                ],
                stellar_source,
            )?;
            let ScVal::Bytes(ScBytes(actual)) = value else {
                return Err(Error::Conflict(
                    "Stellar enforced options are absent".into(),
                ));
            };
            let expected = hex::decode(options.trim_start_matches("0x"))
                .map_err(|_| Error::InvalidInput("enforced options are not hex".into()))?;
            if actual.as_slice() != expected {
                return Err(Error::Conflict("Stellar enforced options mismatch".into()));
            }
            state.effective_config.insert(
                config_key_receive_options(Vm::Stellar, *remote_eid, *message_type),
                serde_json::Value::String(options.clone()),
            );
        }
        OperationV1::SetEvmReceiveOptions {
            remote_eid,
            message_type,
            options,
        } => {
            let mut message_type_word = [0u8; 32];
            message_type_word[30..].copy_from_slice(&message_type.to_be_bytes());
            let value = evm_call(
                evm_oft,
                "enforcedOptions(uint32,uint16)",
                vec![evm_u32_word(*remote_eid), message_type_word],
            )?;
            let actual = decode_evm_dynamic_bytes(&value, "enforcedOptions")?;
            let expected = hex::decode(options.trim_start_matches("0x"))
                .map_err(|_| Error::InvalidInput("enforced options are not hex".into()))?;
            if actual != expected {
                return Err(Error::Conflict("EVM enforced options mismatch".into()));
            }
            state.effective_config.insert(
                config_key_receive_options(Vm::Evm, *remote_eid, *message_type),
                serde_json::Value::String(options.clone()),
            );
        }
        OperationV1::SetStellarUlnConfig {
            remote_eid,
            direction,
            oapp,
            library,
            config,
            ..
        } => {
            let config_type = match direction.as_str() {
                "send" => 2,
                "receive" => 3,
                _ => return Err(Error::InvalidInput("invalid ULN direction".into())),
            };
            let value = stellar.invoke_view(
                &state.identity.stellar_endpoint,
                "get_config",
                &[
                    stellar_arg(crate::layerzero::stellar_address(oapp)?)?,
                    stellar_arg(crate::layerzero::stellar_address(library)?)?,
                    stellar_arg(ScVal::U32(*remote_eid))?,
                    stellar_arg(ScVal::U32(config_type))?,
                ],
                stellar_source,
            )?;
            let ScVal::Bytes(ScBytes(actual)) = value else {
                return Err(Error::Chain("Stellar get_config returned non-bytes".into()));
            };
            let requested: crate::layerzero::UlnConfigType3V1 =
                serde_json::from_value(config.clone())?;
            let readback = crate::codec::decode_stellar_effective_uln_config(actual.as_slice())?;
            if requested.confirmations != readback.confirmations
                || requested.required_dvns != readback.required_dvns
                || requested.optional_dvns != readback.optional_dvns
                || requested.optional_threshold != readback.optional_threshold
            {
                return Err(Error::Conflict(
                    "Stellar ULN config readback mismatch".into(),
                ));
            }
            state.effective_config.insert(
                config_key_uln_config(Vm::Stellar, *remote_eid, direction)?,
                config.clone(),
            );
        }
        OperationV1::SetEvmUlnConfig {
            remote_eid,
            direction,
            oapp,
            library,
            config,
            ..
        } => {
            let config_type = match direction.as_str() {
                "send" => 2,
                "receive" => 3,
                _ => return Err(Error::InvalidInput("invalid ULN direction".into())),
            };
            let value = evm_call(
                &state.identity.evm_endpoint,
                "getConfig(address,address,uint32,uint32)",
                vec![
                    evm_address_word(oapp)?,
                    evm_address_word(library)?,
                    evm_u32_word(*remote_eid),
                    evm_u32_word(config_type),
                ],
            )?;
            let actual = decode_evm_dynamic_bytes(&value, "getConfig")?;
            let requested: crate::layerzero::UlnConfigType3V1 =
                serde_json::from_value(config.clone())?;
            let readback = crate::codec::decode_evm_uln_config(&actual)?;
            if u64::from(requested.confirmations) != readback.confirmations
                || requested.required_dvns != readback.required_dvns
                || requested.optional_dvns != readback.optional_dvns
                || requested.optional_threshold != readback.optional_dvn_threshold
            {
                return Err(Error::Conflict("EVM ULN config readback mismatch".into()));
            }
            state.effective_config.insert(
                config_key_uln_config(Vm::Evm, *remote_eid, direction)?,
                config.clone(),
            );
        }
        OperationV1::SetStellarExecutorConfig {
            remote_eid,
            oapp,
            library,
            config,
            ..
        } => {
            let value = stellar.invoke_view(
                &state.identity.stellar_endpoint,
                "get_config",
                &[
                    stellar_arg(crate::layerzero::stellar_address(oapp)?)?,
                    stellar_arg(crate::layerzero::stellar_address(library)?)?,
                    stellar_arg(ScVal::U32(*remote_eid))?,
                    stellar_arg(ScVal::U32(1))?,
                ],
                stellar_source,
            )?;
            let ScVal::Bytes(ScBytes(actual)) = value else {
                return Err(Error::Chain("Stellar get_config returned non-bytes".into()));
            };
            let requested: crate::layerzero::ExecutorConfigType3V1 =
                serde_json::from_value(config.clone())?;
            if requested
                != crate::codec::decode_stellar_effective_executor_config(actual.as_slice())?
            {
                return Err(Error::Conflict(
                    "Stellar executor config readback mismatch".into(),
                ));
            }
            state.effective_config.insert(
                config_key_executor_config(Vm::Stellar, *remote_eid),
                config.clone(),
            );
        }
        OperationV1::SetEvmExecutorConfig {
            remote_eid,
            oapp,
            library,
            config,
            ..
        } => {
            let value = evm_call(
                &state.identity.evm_endpoint,
                "getConfig(address,address,uint32,uint32)",
                vec![
                    evm_address_word(oapp)?,
                    evm_address_word(library)?,
                    evm_u32_word(*remote_eid),
                    evm_u32_word(1),
                ],
            )?;
            let actual = decode_evm_dynamic_bytes(&value, "getConfig")?;
            let requested: crate::layerzero::ExecutorConfigType3V1 =
                serde_json::from_value(config.clone())?;
            let readback = crate::codec::decode_evm_executor_config(&actual)?;
            if requested.max_message_size != readback.max_message_size
                || !requested.executor.eq_ignore_ascii_case(&readback.executor)
            {
                return Err(Error::Conflict(
                    "EVM executor config readback mismatch".into(),
                ));
            }
            state.effective_config.insert(
                config_key_executor_config(Vm::Evm, *remote_eid),
                config.clone(),
            );
        }
        OperationV1::SetInboundRateLimit {
            remote_eid,
            limit_raw,
            window_seconds,
            mode,
        }
        | OperationV1::SetOutboundRateLimit {
            remote_eid,
            limit_raw,
            window_seconds,
            mode,
        } => {
            use stellar_baselib::xdr::{ScSymbol, ScVec, StringM, VecM};
            let variant = if matches!(operation, OperationV1::SetInboundRateLimit { .. }) {
                "Inbound"
            } else {
                "Outbound"
            };
            let direction = ScVal::Vec(Some(ScVec(
                VecM::try_from(vec![ScVal::Symbol(ScSymbol(
                    StringM::try_from(variant.as_bytes().to_vec()).map_err(|error| {
                        Error::InvalidInput(format!("invalid rate direction: {error}"))
                    })?,
                ))])
                .map_err(|error| Error::InvalidInput(format!("invalid rate direction: {error}")))?,
            )));
            let value = stellar.invoke_view(
                stellar_oft,
                "rate_limit_config",
                &[
                    stellar_arg(direction)?,
                    stellar_arg(ScVal::U32(*remote_eid))?,
                ],
                stellar_source,
            )?;
            let ScVal::Map(Some(map)) = value else {
                return Err(Error::Conflict("Stellar rate limit is absent".into()));
            };
            let field = |name: &[u8]| {
                map.0.iter().find_map(|entry| match &entry.key {
                    ScVal::Symbol(symbol) if symbol.0.as_slice() == name => Some(&entry.val),
                    _ => None,
                })
            };
            let actual_limit = match field(b"limit") {
                Some(ScVal::I128(parts)) => (i128::from(parts.hi) << 64) | i128::from(parts.lo),
                _ => return Err(Error::Chain("rate limit readback has no i128 limit".into())),
            };
            let actual_window = match field(b"window_seconds") {
                Some(ScVal::U64(value)) => *value,
                _ => return Err(Error::Chain("rate limit readback has no u64 window".into())),
            };
            let actual_mode = match field(b"mode") {
                Some(ScVal::Vec(Some(values))) => values.0.first().and_then(|value| match value {
                    ScVal::Symbol(symbol) => std::str::from_utf8(symbol.0.as_slice()).ok(),
                    _ => None,
                }),
                _ => None,
            }
            .ok_or_else(|| Error::Chain("rate limit readback has no mode".into()))?;
            if u128::try_from(actual_limit).ok() != Some(*limit_raw)
                || actual_window != *window_seconds
                || !actual_mode.eq_ignore_ascii_case(mode)
            {
                return Err(Error::Conflict(
                    "Stellar rate-limit readback mismatch".into(),
                ));
            }
            let prefix = if variant == "Inbound" {
                "inbound_rate_limit"
            } else {
                "outbound_rate_limit"
            };
            state.effective_config.insert(
                format!("{prefix}:stellar:{remote_eid}:limit_raw"),
                serde_json::Value::String(limit_raw.to_string()),
            );
            state.effective_config.insert(
                format!("{prefix}:stellar:{remote_eid}:window_seconds"),
                serde_json::Value::String(window_seconds.to_string()),
            );
            state.effective_config.insert(
                format!("{prefix}:stellar:{remote_eid}:mode"),
                serde_json::Value::String(mode.clone()),
            );
        }
        _ => {
            return Err(Error::InvalidInput(
                "operation has no route configuration readback".into(),
            ))
        }
    }
    Ok(())
}

fn stellar_option(
    value: &stellar_baselib::xdr::ScVal,
) -> Result<Option<&stellar_baselib::xdr::ScVal>> {
    match value {
        stellar_baselib::xdr::ScVal::Vec(None) => Ok(None),
        stellar_baselib::xdr::ScVal::Vec(Some(values)) => match values.0.as_slice() {
            [value] => Ok(Some(value)),
            _ => Err(Error::Chain(
                "Stellar Option readback has invalid cardinality".into(),
            )),
        },
        _ => Err(Error::Chain(
            "Stellar Option readback is not a vector".into(),
        )),
    }
}

fn stellar_optional_address(value: &stellar_baselib::xdr::ScVal) -> Result<Option<String>> {
    stellar_option(value)?
        .map(stellar_address_result)
        .transpose()
}

fn stellar_ttl_config(value: &stellar_baselib::xdr::ScVal) -> Result<Option<(u32, u32)>> {
    use stellar_baselib::xdr::ScVal;
    let Some(value) = stellar_option(value)? else {
        return Ok(None);
    };
    let ScVal::Map(Some(map)) = value else {
        return Err(Error::Chain("TTL config readback is not a map".into()));
    };
    let field = |name: &[u8]| {
        map.0.iter().find_map(|entry| match &entry.key {
            ScVal::Symbol(symbol) if symbol.0.as_slice() == name => Some(&entry.val),
            _ => None,
        })
    };
    let Some(ScVal::U32(threshold)) = field(b"threshold") else {
        return Err(Error::Chain("TTL config readback has no threshold".into()));
    };
    let Some(ScVal::U32(extend_to)) = field(b"extend_to") else {
        return Err(Error::Chain("TTL config readback has no extend_to".into()));
    };
    Ok(Some((*threshold, *extend_to)))
}

/// Exact post-confirmation readback for authority, economics, TTL, pause, and
/// role mutations. State changes are committed only after every comparison
/// for the operation succeeds.
pub fn apply_management_readback(
    stellar: &dyn crate::stellar::StellarChain,
    evm: &dyn crate::evm::EvmChain,
    state: &mut RouteStateV1,
    operation: &OperationV1,
) -> Result<()> {
    use stellar_baselib::xdr::ScVal;

    let mut next = state.clone();
    let source = state
        .contracts
        .get("stellar_owner")
        .ok_or_else(|| Error::Custody("route has no recorded stellar_owner".into()))?;
    let oft = state
        .contracts
        .get("stellar_oft")
        .ok_or_else(|| Error::Custody("route has no recorded stellar_oft".into()))?;
    let view = |contract: &str, function: &str, args: &[String]| {
        stellar.invoke_view(contract, function, args, source)
    };
    let evm_view = |contract: &str, signature: &str, words: Vec<[u8; 32]>| {
        crate::block_on_result(evm.call(
            crate::evm::parse_address(contract)?,
            evm_call_data(signature, words),
        ))
    };

    match operation {
        OperationV1::BeginStellarOwnershipTransfer { new_owner, ttl } => {
            let actual = stellar_optional_address(&view(oft, "pending_owner", &[])?)?
                .ok_or_else(|| Error::Conflict("pending Stellar owner is absent".into()))?;
            require_address(&actual, new_owner, "pending Stellar owner")?;
            next.effective_config.insert(
                "stellar:pending_owner".into(),
                serde_json::Value::String(actual),
            );
            next.effective_config.insert(
                "stellar:pending_owner_ttl".into(),
                serde_json::Value::String(ttl.to_string()),
            );
        }
        OperationV1::AcceptStellarOwnership => {
            let expected = state
                .effective_config
                .get("stellar:pending_owner")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| Error::Custody("pending Stellar owner was not recorded".into()))?;
            let actual = stellar_optional_address(&view(oft, "owner", &[])?)?
                .ok_or_else(|| Error::Conflict("Stellar owner is absent".into()))?;
            require_address(&actual, expected, "Stellar owner")?;
            next.contracts.insert("stellar_owner".into(), actual);
            next.effective_config.remove("stellar:pending_owner");
            next.effective_config.remove("stellar:pending_owner_ttl");
        }
        OperationV1::CancelStellarOwnershipTransfer => {
            if stellar_optional_address(&view(oft, "pending_owner", &[])?)?.is_some() {
                return Err(Error::Conflict("pending Stellar owner still exists".into()));
            }
            next.effective_config.remove("stellar:pending_owner");
            next.effective_config.remove("stellar:pending_owner_ttl");
        }
        OperationV1::SetStellarDelegate { delegate } => {
            let actual = stellar_optional_address(&view(
                &state.identity.stellar_endpoint,
                "delegate",
                &[stellar_arg(crate::layerzero::stellar_address(oft)?)?],
            )?)?
            .ok_or_else(|| Error::Conflict("Stellar delegate is absent".into()))?;
            require_address(&actual, delegate, "Stellar delegate")?;
            next.contracts.insert("stellar_delegate".into(), actual);
        }
        OperationV1::TransferEvmOwnership { new_owner } => {
            let evm_oft = state
                .contracts
                .get("evm_oft")
                .ok_or_else(|| Error::Custody("route has no recorded evm_oft".into()))?;
            let actual = decode_evm_word_address(&evm_view(evm_oft, "owner()", vec![])?, "owner")?;
            require_address(&actual, new_owner, "EVM owner")?;
            next.contracts.insert("evm_owner".into(), actual);
        }
        OperationV1::SetEvmDelegate { delegate } => {
            let evm_oft = state
                .contracts
                .get("evm_oft")
                .ok_or_else(|| Error::Custody("route has no recorded evm_oft".into()))?;
            let actual = decode_evm_word_address(
                &evm_view(
                    &state.identity.evm_endpoint,
                    "delegates(address)",
                    vec![evm_address_word(evm_oft)?],
                )?,
                "delegate",
            )?;
            require_address(&actual, delegate, "EVM delegate")?;
            next.contracts.insert("evm_delegate".into(), actual);
        }
        OperationV1::SetDefaultFee { bps } => {
            match stellar_option(&view(oft, "default_fee_bps", &[])?)? {
                Some(ScVal::U32(actual)) if actual == bps => {}
                _ => return Err(Error::Conflict("default fee readback mismatch".into())),
            }
            next.effective_config.insert(
                "fee_bps:stellar_default".into(),
                serde_json::Value::String(bps.to_string()),
            );
        }
        OperationV1::SetDestinationFee { remote_eid, bps } => {
            match view(oft, "fee_bps", &[stellar_arg(ScVal::U32(*remote_eid))?])? {
                ScVal::U32(actual) if actual == *bps => {}
                _ => return Err(Error::Conflict("destination fee readback mismatch".into())),
            }
            next.effective_config.insert(
                format!("fee_bps:stellar:{remote_eid}"),
                serde_json::Value::String(bps.to_string()),
            );
            if *remote_eid == state.identity.evm_eid {
                next.effective_config.insert(
                    "fee_bps:stellar_to_evm".into(),
                    serde_json::Value::String(bps.to_string()),
                );
            }
        }
        OperationV1::SetFeeRecipient { recipient } => {
            let actual = stellar_optional_address(&view(oft, "fee_deposit_address", &[])?)?
                .ok_or_else(|| Error::Conflict("fee recipient is absent".into()))?;
            require_address(&actual, recipient, "fee recipient")?;
            next.effective_config.insert(
                "fee_deposit_address:stellar".into(),
                serde_json::Value::String(actual),
            );
        }
        OperationV1::SetMessageInspector { inspector } => {
            let actual = stellar_optional_address(&view(oft, "msg_inspector", &[])?)?;
            match (actual.as_deref(), inspector.as_deref()) {
                (None, None) => {}
                (Some(actual), Some(expected)) => {
                    require_address(actual, expected, "message inspector")?;
                }
                _ => {
                    return Err(Error::Conflict(
                        "message inspector readback mismatch".into(),
                    ))
                }
            }
            next.effective_config.insert(
                "message_inspector:stellar".into(),
                actual.map_or(serde_json::Value::Null, serde_json::Value::String),
            );
        }
        OperationV1::SetInboundRateLimit { .. } | OperationV1::SetOutboundRateLimit { .. } => {
            apply_live_readback(stellar, evm, &mut next, operation)?;
        }
        OperationV1::PauseEmergency | OperationV1::UnpauseEmergency => {
            let expected = matches!(operation, OperationV1::PauseEmergency);
            if view(oft, "is_paused", &[])? != ScVal::Bool(expected) {
                return Err(Error::Conflict("pause state readback mismatch".into()));
            }
            next.effective_config.insert(
                "stellar:is_paused".into(),
                serde_json::Value::Bool(expected),
            );
        }
        OperationV1::SetTtlConfig {
            instance_threshold,
            instance_extend_to,
            persistent_threshold,
            persistent_extend_to,
        } => {
            let ScVal::Vec(Some(values)) = view(oft, "ttl_configs", &[])? else {
                return Err(Error::Chain("TTL configs readback is not a tuple".into()));
            };
            if values.0.len() != 2
                || stellar_ttl_config(&values.0[0])?
                    != Some((*instance_threshold, *instance_extend_to))
                || stellar_ttl_config(&values.0[1])?
                    != Some((*persistent_threshold, *persistent_extend_to))
            {
                return Err(Error::Conflict("TTL config readback mismatch".into()));
            }
            for (key, value) in [
                ("ttl:instance_threshold", *instance_threshold),
                ("ttl:instance_extend_to", *instance_extend_to),
                ("ttl:persistent_threshold", *persistent_threshold),
                ("ttl:persistent_extend_to", *persistent_extend_to),
            ] {
                next.effective_config
                    .insert(key.into(), serde_json::Value::String(value.to_string()));
            }
        }
        OperationV1::FreezeTtlConfig { .. } => {
            if view(oft, "is_ttl_configs_frozen", &[])? != ScVal::Bool(true) {
                return Err(Error::Conflict("TTL freeze readback mismatch".into()));
            }
            next.effective_config
                .insert("ttl:is_frozen".into(), serde_json::Value::Bool(true));
        }
        OperationV1::ExtendInstanceTtl { .. } => {
            let current = stellar.latest_ledger()?;
            let live_until = stellar.contract_instance_live_until(oft)?;
            if live_until <= current {
                return Err(Error::Conflict(
                    "contract instance TTL was not extended".into(),
                ));
            }
            next.effective_config.insert(
                "ttl:current_ledger".into(),
                serde_json::Value::String(current.to_string()),
            );
            next.effective_config.insert(
                "ttl:instance_live_until".into(),
                serde_json::Value::String(live_until.to_string()),
            );
        }
        OperationV1::GrantRole { role, address } | OperationV1::RevokeRole { role, address } => {
            let actual = view(
                oft,
                "has_role",
                &[
                    stellar_arg(crate::layerzero::stellar_address(address)?)?,
                    stellar_arg(stellar_symbol_arg(role)?)?,
                ],
            )?;
            let granted = matches!(operation, OperationV1::GrantRole { .. });
            let assigned = matches!(stellar_option(&actual)?, Some(ScVal::U32(_)));
            if granted != assigned {
                return Err(Error::Conflict("role grant readback mismatch".into()));
            }
            let key = format!("stellar_role:{role}");
            if granted {
                next.contracts.insert(key, address.clone());
            } else if next.contracts.get(&key) == Some(address) {
                next.contracts.remove(&key);
            }
        }
        OperationV1::SetRoleAdmin { role, admin_role } => {
            let actual = view(
                oft,
                "get_role_admin",
                &[stellar_arg(stellar_symbol_arg(role)?)?],
            )?;
            let Some(ScVal::Symbol(actual)) = stellar_option(&actual)? else {
                return Err(Error::Conflict("role admin is absent".into()));
            };
            if actual.0.as_slice() != admin_role.as_bytes() {
                return Err(Error::Conflict("role admin readback mismatch".into()));
            }
            next.effective_config.insert(
                format!("stellar_role_admin:{role}"),
                serde_json::Value::String(admin_role.clone()),
            );
        }
        OperationV1::RemoveRoleAdmin { role, .. } => {
            let actual = view(
                oft,
                "get_role_admin",
                &[stellar_arg(stellar_symbol_arg(role)?)?],
            )?;
            if stellar_option(&actual)?.is_some() {
                return Err(Error::Conflict("role admin still exists".into()));
            }
            next.effective_config
                .remove(&format!("stellar_role_admin:{role}"));
        }
        _ => {
            return Err(Error::InvalidInput(
                "operation has no management readback".into(),
            ))
        }
    }
    *state = next;
    Ok(())
}

/// Populates an adopted route exclusively from live contract reads.
pub fn apply_adoption_readback(
    stellar: &dyn crate::stellar::StellarChain,
    evm: &dyn crate::evm::EvmChain,
    state: &mut RouteStateV1,
    desired: &DesiredRouteV1,
) -> Result<()> {
    use stellar_baselib::xdr::ScVal;

    let mut next = state.clone();
    let stellar_oft = next
        .contracts
        .get("stellar_oft")
        .cloned()
        .ok_or_else(|| Error::Custody("adoption omitted stellar_oft".into()))?;
    let evm_oft = next
        .contracts
        .get("evm_oft")
        .cloned()
        .ok_or_else(|| Error::Custody("adoption omitted evm_oft".into()))?;
    let _stellar_code_hash = stellar.contract_code_hash(&stellar_oft)?;
    if crate::block_on_result(evm.code(crate::evm::parse_address(&evm_oft)?))?.is_empty() {
        return Err(Error::Conflict("adopted OFT has no contract code".into()));
    }
    let view = |contract: &str, function: &str, args: &[String]| {
        stellar.invoke_view(contract, function, args, &desired.stellar_owner)
    };
    let stellar_owner = stellar_address_result(&view(&stellar_oft, "owner", &[])?)?;
    require_address(&stellar_owner, &desired.stellar_owner, "Stellar owner")?;
    let stellar_delegate = stellar_address_result(&view(
        &desired.identity.stellar_endpoint,
        "delegate",
        &[stellar_arg(crate::layerzero::stellar_address(
            &stellar_oft,
        )?)?],
    )?)?;
    require_address(
        &stellar_delegate,
        &desired.stellar_delegate,
        "Stellar delegate",
    )?;
    let stellar_token = stellar_address_result(&view(&stellar_oft, "token", &[])?)?;
    if desired.asset.asset_id.starts_with('C') {
        require_address(&stellar_token, &desired.asset.asset_id, "Stellar token")?;
    }
    let ScVal::U32(shared_decimals) = view(&stellar_oft, "shared_decimals", &[])? else {
        return Err(Error::Chain("shared_decimals readback is not u32".into()));
    };
    let call = |contract: &str, signature: &str, words: Vec<[u8; 32]>| {
        crate::block_on_result(evm.call(
            crate::evm::parse_address(contract)?,
            evm_call_data(signature, words),
        ))
    };
    let evm_owner = decode_evm_word_address(&call(&evm_oft, "owner()", vec![])?, "owner")?;
    require_address(&evm_owner, &desired.evm_owner, "EVM owner")?;
    let evm_delegate = decode_evm_word_address(
        &call(
            &desired.identity.evm_endpoint,
            "delegates(address)",
            vec![evm_address_word(&evm_oft)?],
        )?,
        "delegate",
    )?;
    require_address(&evm_delegate, &desired.evm_delegate, "EVM delegate")?;
    let evm_endpoint = decode_evm_word_address(&call(&evm_oft, "endpoint()", vec![])?, "endpoint")?;
    require_address(
        &evm_endpoint,
        &desired.identity.evm_endpoint,
        "EVM endpoint",
    )?;
    let evm_token = decode_evm_word_address(&call(&evm_oft, "token()", vec![])?, "token")?;
    let evm_decimals_bytes = call(&evm_oft, "decimals()", vec![])?;
    if evm_decimals_bytes.len() != 32 || evm_decimals_bytes[..31].iter().any(|byte| *byte != 0) {
        return Err(Error::Chain("decimals readback is not a u8 word".into()));
    }
    let evm_decimals = evm_decimals_bytes[31];
    next.contracts.extend([
        ("stellar_owner".into(), stellar_owner),
        ("stellar_delegate".into(), stellar_delegate),
        ("stellar_token".into(), stellar_token),
        ("evm_owner".into(), evm_owner),
        ("evm_delegate".into(), evm_delegate),
        ("evm_token".into(), evm_token),
    ]);
    next.effective_config.insert(
        "asset:shared_decimals".into(),
        serde_json::Value::String(shared_decimals.to_string()),
    );
    next.effective_config.insert(
        "asset:evm_decimals".into(),
        serde_json::Value::String(evm_decimals.to_string()),
    );
    if let Some(opening) = next.opening_custody.as_ref() {
        let ScVal::I128(parts) = view(
            next.contracts
                .get("stellar_token")
                .ok_or_else(|| Error::Custody("Stellar token was not observed".into()))?,
            "balance",
            &[stellar_arg(crate::layerzero::stellar_address(
                &stellar_oft,
            )?)?],
        )?
        else {
            return Err(Error::Chain("lockbox balance readback is not i128".into()));
        };
        let lockbox = (i128::from(parts.hi) << 64) | i128::from(parts.lo);
        if lockbox.is_negative() || u128::try_from(lockbox).ok() != Some(opening.lockbox_raw) {
            return Err(Error::Conflict(
                "live lockbox balance differs from opening custody".into(),
            ));
        }
        let supply_bytes = call(&evm_oft, "totalSupply()", vec![])?;
        if supply_bytes.len() != 32 {
            return Err(Error::Chain("totalSupply readback is not one word".into()));
        }
        let supply = alloy::primitives::U256::from_be_slice(&supply_bytes).to_string();
        if supply != opening.evm_supply_raw.to_string() {
            return Err(Error::Conflict(
                "live EVM supply differs from opening custody".into(),
            ));
        }
        next.effective_config.insert(
            "custody:observed_lockbox_raw".into(),
            serde_json::Value::String(lockbox.to_string()),
        );
        next.effective_config.insert(
            "custody:normalized_evm_supply_raw".into(),
            serde_json::Value::String(supply),
        );
    }
    let plan = plan_route_mutations(desired, &next)?;
    for operation in plan.steps.into_iter().filter_map(|step| {
        (!matches!(step.status, RouteStepStatus::Satisfied) && step.missing_prerequisite.is_none())
            .then_some(step.operation)
            .flatten()
    }) {
        apply_live_readback(stellar, evm, &mut next, &operation)?;
    }
    if !plan_route_mutations(desired, &next)?.converged {
        return Err(Error::Conflict(
            "adopted route did not converge after live readback".into(),
        ));
    }
    *state = next;
    Ok(())
}
