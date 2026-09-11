//! Canary leg intent, live observation, send, watch, recovery, and evidence
//! import flows. Quote is non-authoritative; send re-reads live state and
//! refuses drift before signing.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::domain::Direction;
use crate::error::{Error, Result};
use crate::output::CommandData;
use crate::state::RouteStore;

/// Read-only route access: allowed on mainnet (inspection/drafting).
fn route_read(state_path: &Path) -> Result<crate::domain::RouteStateV1> {
    let state = RouteStore::open(state_path)?.load_state()?;
    crate::environment::classify(&state.identity)?;
    Ok(state)
}

/// Route access for send/recovery validation and proposal planning. Direct
/// mainnet signing is rejected later at the execution boundary.
fn route_environment(state_path: &Path) -> Result<crate::domain::RouteStateV1> {
    let state = route_read(state_path)?;
    Ok(state)
}

fn config_raw(state: &crate::domain::RouteStateV1, key: &str) -> Result<u128> {
    state
        .effective_config
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Custody(format!("effective route is missing {key}")))?
        .parse()
        .map_err(|_| Error::Custody(format!("effective route field {key} is not decimal")))
}

fn validate_destination(direction: Direction, destination: &str) -> Result<()> {
    match direction {
        Direction::StellarToEvm => {
            crate::evm::parse_address(destination)?;
        }
        Direction::EvmToStellar => {
            crate::codec::strkey_to_bytes32(destination)?;
        }
    }
    Ok(())
}

/// Exact-route convergence gate: every requested configuration field must
/// equal its recorded effective readback. Unknown effective-only fields are
/// recorded evidence, never a convergence failure.
fn require_route_converged(state: &crate::domain::RouteStateV1) -> Result<()> {
    let mut unmatched: Vec<String> = Vec::new();
    for (key, requested) in &state.requested_config {
        match state.effective_config.get(key) {
            Some(actual) if actual == requested => {}
            Some(actual) => unmatched.push(format!(
                "{key}: requested {requested} but effective readback is {actual}"
            )),
            None => unmatched.push(format!(
                "{key}: requested but no effective readback is recorded"
            )),
        }
    }
    if unmatched.is_empty() {
        Ok(())
    } else {
        Err(Error::Conflict(format!(
            "route is not fully converged: {}",
            unmatched.join("; ")
        )))
    }
}

fn direction_label(direction: Direction) -> &'static str {
    match direction {
        Direction::StellarToEvm => "stellar_to_evm",
        Direction::EvmToStellar => "evm_to_stellar",
    }
}

/// Validates an address on the source side of the leg: Stellar for the
/// forward leg, EVM for the reverse leg.
fn validate_source_address(direction: Direction, source: &str) -> Result<()> {
    match direction {
        Direction::StellarToEvm => crate::codec::strkey_to_bytes32(source).map(|_| ()),
        Direction::EvmToStellar => crate::evm::parse_address(source).map(|_| ()),
    }
}

/// Recorded source-specific canary sender. The forward leg originates on
/// Stellar, the reverse leg on EVM.
fn sender_for(state: &crate::domain::RouteStateV1, direction: Direction) -> Result<String> {
    let key = format!("canary:sender:{}", direction_label(direction));
    let sender = state
        .effective_config
        .get(&key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Custody(format!("effective route is missing {key}")))?
        .to_string();
    validate_source_address(direction, &sender)?;
    Ok(sender)
}

/// Recorded source refund address. Without recorded evidence the refund
/// defaults to the source sender, which is the canonical OFT `refundAddress`
/// protocol default; nothing is synthesized beyond that default.
fn refund_for(
    state: &crate::domain::RouteStateV1,
    direction: Direction,
    sender: &str,
) -> Result<String> {
    let key = format!("canary:refund_address:{}", direction_label(direction));
    let refund = match state
        .effective_config
        .get(&key)
        .and_then(serde_json::Value::as_str)
    {
        Some(recorded) => recorded.to_string(),
        None => sender.to_string(),
    };
    validate_source_address(direction, &refund)?;
    Ok(refund)
}

/// Canonical intent derived exclusively from converged recorded state and
/// finalized custody evidence. `leg quote` writes this; `leg send` re-derives
/// it to refuse any drift before signing or proposal. Missing fee, rate,
/// option, or balance evidence is refused, never fabricated.
fn build_intent(
    state: &crate::domain::RouteStateV1,
    direction: Direction,
    amount_raw: u128,
    to: &str,
    now_unix: u64,
    outstanding_raw: u128,
) -> Result<crate::domain::LegIntentV1> {
    require_route_converged(state)?;
    let opening = state
        .opening_custody
        .as_ref()
        .ok_or_else(|| Error::Custody("opening custody is not finalized".into()))?;
    validate_destination(direction, to)?;
    let cap = config_raw(state, "canary:max_amount_raw")?;
    if amount_raw == 0 || amount_raw > cap {
        return Err(Error::Policy(format!(
            "canary amount must be between 1 and {cap} raw"
        )));
    }
    let direction_label = direction_label(direction);
    let sender = sender_for(state, direction)?;
    let refund_address = refund_for(state, direction, &sender)?;
    let fee_bps = config_raw(state, &format!("fee_bps:{direction_label}"))?;
    if fee_bps > 10_000 {
        return Err(Error::Custody("recorded fee bps exceeds 100%".into()));
    }
    let minimum_received_raw = amount_raw
        .checked_sub(
            amount_raw
                .checked_mul(fee_bps)
                .ok_or_else(|| Error::Custody("quote fee overflow".into()))?
                / 10_000,
        )
        .ok_or_else(|| Error::Custody("quote fee exceeds amount".into()))?;
    let native_fee_raw = config_raw(
        state,
        &format!("canary:quoted_native_fee_raw:{direction_label}"),
    )?;
    let maximum_native_fee_raw = config_raw(
        state,
        &format!("canary:max_native_fee_raw:{direction_label}"),
    )?;
    if native_fee_raw > maximum_native_fee_raw {
        return Err(Error::Custody(format!(
            "recorded canary native fee exceeds the recorded ceiling for {direction_label}"
        )));
    }
    let extra_options = state
        .effective_config
        .get(&format!("canary:extra_options:{direction_label}"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Custody("effective route is missing canary extra options".into()))?
        .to_string();
    let ttl = config_raw(state, "canary:quote_ttl_seconds")?;
    let finality_policy = state
        .effective_config
        .get("canary:finality_policy")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            Error::Custody("effective route is missing canary:finality_policy".into())
        })?;
    let finality_policy = match finality_policy {
        "confirmed" => crate::domain::LegFinalityPolicyV1::Confirmed,
        other => {
            return Err(Error::Policy(format!(
                "unsupported canary finality policy {other:?}; expected confirmed"
            )))
        }
    };
    let fee_ceiling = match direction {
        Direction::StellarToEvm => crate::domain::LegFeeCeilingV1::Stellar {
            resource_fee_ceiling_raw: config_raw(
                state,
                &format!("canary:stellar_resource_fee_ceiling_raw:{direction_label}"),
            )?
            .to_string(),
        },
        Direction::EvmToStellar => crate::domain::LegFeeCeilingV1::Evm {
            max_fee_per_gas_wei: config_raw(
                state,
                &format!("canary:evm_max_fee_per_gas_wei:{direction_label}"),
            )?
            .to_string(),
            max_priority_fee_per_gas_wei: config_raw(
                state,
                &format!("canary:evm_max_priority_fee_per_gas_wei:{direction_label}"),
            )?
            .to_string(),
            gas_limit: config_raw(state, &format!("canary:evm_gas_limit:{direction_label}"))?
                .try_into()
                .map_err(|_| Error::Custody("recorded EVM gas limit exceeds u64".into()))?,
        },
    };
    let outstanding_cap_raw = config_raw(state, "canary:max_outstanding_obligation_raw")?;
    let peer_snapshot_sha256 = crate::canonical_sha256(&peer_records(state))?;
    let additional_obligation = crate::domain::LegAdditionalObligationV1 {
        outstanding_raw: outstanding_raw.to_string(),
        cap_raw: outstanding_cap_raw.to_string(),
    };
    let expires_at_unix = now_unix
        .checked_add(
            ttl.try_into()
                .map_err(|_| Error::Custody("quote TTL exceeds u64".into()))?,
        )
        .ok_or_else(|| Error::Custody("quote expiry overflow".into()))?;
    let intent = crate::domain::LegIntentV1 {
        schema_name: "leg_intent".into(),
        schema_version: crate::domain::SCHEMA_VERSION,
        route_id: state.route_id.clone(),
        desired_sha256: state.desired_sha256.clone(),
        direction,
        amount_raw: amount_raw.to_string(),
        destination_eid: match direction {
            Direction::StellarToEvm => state.identity.evm_eid,
            Direction::EvmToStellar => state.identity.stellar_eid,
        },
        to: to.to_string(),
        sender,
        refund_address,
        minimum_received_raw: minimum_received_raw.to_string(),
        native_fee_raw: native_fee_raw.to_string(),
        extra_options,
        maximum_native_fee_raw: maximum_native_fee_raw.to_string(),
        config_snapshot_sha256: crate::canonical_sha256(&state.effective_config)?,
        custody_snapshot_sha256: crate::canonical_sha256(opening)?,
        peer_snapshot_sha256,
        quote_source_ledger: None,
        quote_source_block: None,
        observed_sequence_nonce: None,
        fee_ceiling: Some(fee_ceiling),
        pre_send_snapshot: None,
        finality_policy: Some(finality_policy),
        additional_obligation: Some(additional_obligation),
        expires_at_unix,
    }
    .parse()?;
    Ok(intent)
}

/// Re-derives the canonical intent for the same direction, amount, and
/// destination from current recorded state and refuses the quoted intent when
/// any bound input drifted. Expiry is governed separately by the intent's own
/// ceiling and the current TTL policy, never by a re-derived clock value.
fn revalidate_intent(
    state: &crate::domain::RouteStateV1,
    intent: &crate::domain::LegIntentV1,
    now_unix: u64,
    outstanding_raw: u128,
) -> Result<()> {
    let amount: u128 = intent.amount_raw.parse().map_err(|_| {
        Error::InvalidInput("leg intent amount_raw is not a decimal integer".into())
    })?;
    let rederived = build_intent(
        state,
        intent.direction,
        amount,
        &intent.to,
        now_unix,
        outstanding_raw,
    )?;
    let mut drift: Vec<&'static str> = Vec::new();
    if intent.route_id != rederived.route_id {
        drift.push("route_id");
    }
    if intent.desired_sha256 != rederived.desired_sha256 {
        drift.push("desired_sha256");
    }
    if intent.direction != rederived.direction {
        drift.push("direction");
    }
    if intent.amount_raw != rederived.amount_raw {
        drift.push("amount_raw");
    }
    if intent.destination_eid != rederived.destination_eid {
        drift.push("destination_eid");
    }
    if intent.to != rederived.to {
        drift.push("destination");
    }
    if intent.sender != rederived.sender {
        drift.push("sender");
    }
    if intent.refund_address != rederived.refund_address {
        drift.push("refund_address");
    }
    if intent.minimum_received_raw != rederived.minimum_received_raw {
        drift.push("minimum_received_raw");
    }
    if intent.native_fee_raw != rederived.native_fee_raw {
        drift.push("native_fee_raw");
    }
    if intent.extra_options != rederived.extra_options {
        drift.push("extra_options");
    }
    if intent.maximum_native_fee_raw != rederived.maximum_native_fee_raw {
        drift.push("maximum_native_fee_raw");
    }
    if intent.config_snapshot_sha256 != rederived.config_snapshot_sha256 {
        drift.push("config_snapshot_sha256");
    }
    if intent.custody_snapshot_sha256 != rederived.custody_snapshot_sha256 {
        drift.push("custody_snapshot_sha256");
    }
    if intent.peer_snapshot_sha256 != rederived.peer_snapshot_sha256 {
        drift.push("peer_snapshot_sha256");
    }
    if intent.fee_ceiling != rederived.fee_ceiling {
        drift.push("fee_ceiling");
    }
    if intent.finality_policy != rederived.finality_policy {
        drift.push("finality_policy");
    }
    if intent.additional_obligation != rederived.additional_obligation {
        drift.push("additional_obligation");
    }
    if !drift.is_empty() {
        return Err(Error::Conflict(format!(
            "leg intent no longer matches current route state; drifted: {}",
            drift.join(", ")
        )));
    }
    Ok(())
}

/// Sums outstanding unresolved packet obligations from the append-only
/// message ledger, mirroring the canonical reconcile classification: locked
/// and not yet minted forwards, and burned and not yet unlocked reverses.
/// Ledger records are the only obligation evidence; a stuck reverse cannot
/// trigger another send unless the operator explicitly allows it within the
/// recorded cap.
fn outstanding_obligations(store: &crate::state::RouteStore) -> Result<u128> {
    use crate::domain::MessageStageV1;
    let messages = store.load_messages()?;
    let mut outstanding: u128 = 0;
    for message in &messages {
        let stage = message
            .status_events
            .iter()
            .rev()
            .find(|event| event.stage != MessageStageV1::Reobserved)
            .ok_or_else(|| {
                Error::Custody(format!("message {} has no recorded stage", message.guid))
            })?
            .stage;
        let amount_field = match (message.direction, stage) {
            (
                Direction::StellarToEvm,
                MessageStageV1::ForwardLocked
                | MessageStageV1::ForwardVerified
                | MessageStageV1::ForwardCommitted,
            ) => Some(&message.net_locked_raw[..]),
            (
                Direction::EvmToStellar,
                MessageStageV1::ReverseBurned
                | MessageStageV1::ReverseVerified
                | MessageStageV1::ReverseCommitted,
            ) => Some(&message.burned_raw[..]),
            (
                Direction::StellarToEvm,
                MessageStageV1::ForwardMinted | MessageStageV1::ForwardSourceAccepted,
            )
            | (
                Direction::EvmToStellar,
                MessageStageV1::ReverseUnlocked | MessageStageV1::ReverseSourceAccepted,
            ) => None,
            _ => {
                return Err(Error::Custody(format!(
                    "message {} has a stage inconsistent with its direction",
                    message.guid
                )))
            }
        };
        if let Some(raw) = amount_field {
            let value: u128 = raw.parse().map_err(|_| {
                Error::Custody(format!("message {} amount is not decimal", message.guid))
            })?;
            outstanding = outstanding
                .checked_add(value)
                .ok_or_else(|| Error::Custody("outstanding obligation overflow".into()))?;
        }
    }
    Ok(outstanding)
}

/// Produces a route-bound, expiring, non-signable intent solely from
/// converged exact state and recorded finalized quote/config/custody
/// evidence. No nonce reservation or signable transaction construction.
pub fn quote(
    state_path: &Path,
    direction: Direction,
    amount_raw: u128,
    to: &str,
    out: &Path,
) -> Result<CommandData> {
    let state = route_read(state_path)?;
    let store = crate::state::RouteStore::open(state_path)?;
    let outstanding = outstanding_obligations(&store)?;
    let intent = build_intent(
        &state,
        direction,
        amount_raw,
        to,
        crate::now_unix()?,
        outstanding,
    )?;
    crate::state::write_create_new_json(out, &intent)?;
    Ok(CommandData {
        result: serde_json::to_value(&intent)?,
        artifact: None,
    })
}

pub fn quote_live(
    state_path: &Path,
    direction: Direction,
    amount_raw: u128,
    to: &str,
    out: &Path,
    stellar: &dyn crate::stellar::StellarChain,
    evm: &dyn crate::evm::EvmChain,
) -> Result<CommandData> {
    let state = route_read(state_path)?;
    let store = crate::state::RouteStore::open(state_path)?;
    let outstanding = outstanding_obligations(&store)?;
    let mut intent = build_intent(
        &state,
        direction,
        amount_raw,
        to,
        crate::now_unix()?,
        outstanding,
    )?;
    let observation = observe_leg(&state, direction, stellar, evm)?;
    intent.quote_source_ledger = observation.quote_source_ledger;
    intent.quote_source_block = observation.quote_source_block;
    intent.observed_sequence_nonce = Some(observation.observed_sequence_nonce);
    intent.pre_send_snapshot = Some(observation.pre_send_snapshot);
    let intent = intent.parse()?;
    crate::state::write_create_new_json(out, &intent)?;
    Ok(CommandData {
        result: serde_json::to_value(&intent)?,
        artifact: None,
    })
}

/// Validates the exact quoted intent against current custody/config/obligation
/// state and returns the source-VM operation. Every bound input is re-derived
/// from recorded state; any drift, expiry breach, cap breach, or source
/// authority mismatch rejects the intent before signing or proposal. No loose
/// send flags are reconstructed.
pub fn send_operation(
    state_path: &Path,
    intent_path: &Path,
    allow_additional_obligation: bool,
) -> Result<crate::domain::OperationV1> {
    let intent: crate::domain::LegIntentV1 = crate::state::read_json(intent_path)?;
    validate_send_intent(state_path, intent, allow_additional_obligation)
}

fn validate_send_intent(
    state_path: &Path,
    intent: crate::domain::LegIntentV1,
    allow_additional_obligation: bool,
) -> Result<crate::domain::OperationV1> {
    let state = route_environment(state_path)?;
    let store = crate::state::RouteStore::open(state_path)?;
    let intent = intent.parse()?;
    if intent.route_id != state.route_id || intent.desired_sha256 != state.desired_sha256 {
        return Err(Error::Conflict(
            "leg intent does not bind to this route state".into(),
        ));
    }
    let now = crate::now_unix()?;
    if now >= intent.expires_at_unix {
        return Err(Error::Conflict("leg intent has expired".into()));
    }
    let ttl = config_raw(&state, "canary:quote_ttl_seconds")?;
    let ttl_ceiling = now
        .checked_add(
            ttl.try_into()
                .map_err(|_| Error::Custody("quote TTL exceeds u64".into()))?,
        )
        .ok_or_else(|| Error::Custody("quote TTL ceiling overflow".into()))?;
    if intent.expires_at_unix > ttl_ceiling {
        return Err(Error::Conflict(
            "leg intent expiry exceeds the currently recorded quote TTL ceiling; re-quote".into(),
        ));
    }
    let outstanding = outstanding_obligations(&store)?;
    revalidate_intent(&state, &intent, now, outstanding)?;
    let amount: u128 = intent.amount_raw.parse().map_err(|_| {
        Error::InvalidInput("leg intent amount_raw is not a decimal integer".into())
    })?;
    if amount > config_raw(&state, "canary:max_amount_raw")? {
        return Err(Error::Policy(
            "leg intent exceeds the current canary cap".into(),
        ));
    }
    if outstanding > 0 && !allow_additional_obligation {
        return Err(Error::Policy(
            "an outstanding bridge obligation requires --allow-additional-obligation".into(),
        ));
    }
    let cap: u128 = intent
        .additional_obligation
        .as_ref()
        .ok_or_else(|| Error::Custody("leg intent is missing the recorded obligation cap".into()))?
        .cap_raw
        .parse()
        .map_err(|_| Error::Custody("leg intent obligation cap is not decimal".into()))?;
    let resulting = outstanding
        .checked_add(amount)
        .ok_or_else(|| Error::Custody("resulting obligation overflow".into()))?;
    if resulting > cap {
        return Err(Error::Policy(format!(
            "resulting outstanding obligation {resulting} exceeds the recorded cap {cap}"
        )));
    }
    Ok(crate::domain::OperationV1::SendLeg {
        vm: match intent.direction {
            Direction::StellarToEvm => crate::domain::Vm::Stellar,
            Direction::EvmToStellar => crate::domain::Vm::Evm,
        },
        intent: Box::new(intent),
    })
}

/// Exact `peer:*` records bound by the canonical intent. The OFT economic and
/// rate-limit surface lives in `effective_config` and is already bound by
/// `config_snapshot_sha256`; peer bindings are chain state, not config, so they
/// are hashed explicitly here.
fn peer_records(state: &crate::domain::RouteStateV1) -> serde_json::Map<String, serde_json::Value> {
    state
        .contracts
        .iter()
        .filter(|(key, _)| key.starts_with("peer:"))
        .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
        .collect()
}

/// Read-only live observation bound by a quoted leg: source chain height,
/// source-account sequence/nonce, and the pre-send
/// balance/lockbox/supply snapshot. The quote never reserves the sequence or
/// nonce and never constructs a signable transaction; send re-reads these same
/// observations and refuses any drift before signing.
struct LegLiveObservationV1 {
    quote_source_ledger: Option<u32>,
    quote_source_block: Option<u64>,
    observed_sequence_nonce: String,
    pre_send_snapshot: crate::domain::LegPreSendSnapshotV1,
}

/// Executes a read-only EVM view and decodes its 32-byte word result as a
/// decimal string, mirroring the recorded economic readback pattern.
fn evm_word(
    evm: &dyn crate::evm::EvmChain,
    to: &str,
    signature: &str,
    words: Vec<[u8; 32]>,
) -> Result<String> {
    let target = crate::evm::parse_address(to)?;
    let mut calldata = crate::evm::keccak256_of(signature.as_bytes())[..4].to_vec();
    for word in words {
        calldata.extend(word);
    }
    let bytes = crate::block_on_result(evm.call(target, calldata))?;
    if bytes.len() != 32 {
        return Err(Error::Chain(format!(
            "{signature} returned a short EVM word"
        )));
    }
    Ok(alloy::primitives::U256::from_be_slice(&bytes).to_string())
}

fn observe_leg(
    state: &crate::domain::RouteStateV1,
    direction: Direction,
    stellar: &dyn crate::stellar::StellarChain,
    evm: &dyn crate::evm::EvmChain,
) -> Result<LegLiveObservationV1> {
    let stellar_token = state
        .contracts
        .get("stellar_token")
        .ok_or_else(|| Error::Custody("route has no recorded stellar_token".into()))?;
    let stellar_oft = state
        .contracts
        .get("stellar_oft")
        .ok_or_else(|| Error::Custody("route has no recorded stellar_oft".into()))?;
    let evm_token = state
        .contracts
        .get("evm_token")
        .ok_or_else(|| Error::Custody("route has no recorded evm_token".into()))?;
    let evm_oft = state
        .contracts
        .get("evm_oft")
        .ok_or_else(|| Error::Custody("route has no recorded evm_oft".into()))?;
    let stellar_source = state
        .contracts
        .get("stellar_owner")
        .ok_or_else(|| Error::Custody("route has no recorded stellar_owner".into()))?;
    let sender = sender_for(state, direction)?;
    let (quote_source_ledger, quote_source_block, observed_sequence_nonce) = match direction {
        Direction::StellarToEvm => (
            Some(stellar.latest_ledger()?),
            None,
            stellar.account_sequence(&sender)?,
        ),
        Direction::EvmToStellar => {
            let sender_address = crate::evm::parse_address(&sender)?;
            let block = crate::block_on_result(evm.latest_block())?;
            let nonce = crate::block_on_result(evm.account_nonce(sender_address))?;
            (None, Some(block), nonce.to_string())
        }
    };
    let source_balance_raw = match direction {
        Direction::StellarToEvm => stellar.token_balance(stellar_token, &sender, stellar_source)?,
        Direction::EvmToStellar => evm_word(
            evm,
            evm_token,
            "balanceOf(address)",
            vec![crate::codec::evm_address_to_bytes32(&sender)?],
        )?,
    };
    let lockbox_raw = stellar.token_balance(stellar_token, stellar_oft, stellar_source)?;
    let evm_supply_raw = evm_word(evm, evm_oft, "totalSupply()", Vec::new())?;
    Ok(LegLiveObservationV1 {
        quote_source_ledger,
        quote_source_block,
        observed_sequence_nonce,
        pre_send_snapshot: crate::domain::LegPreSendSnapshotV1 {
            source_balance_raw,
            lockbox_raw,
            evm_supply_raw,
        },
    })
}

/// Refuses the leg when any live observation no longer matches the values the
/// quote bound. Source height and sequence/nonce are compared only for the
/// leg's own source VM; the cross-VM height is not observed and cannot drift.
fn reject_live_drift(
    intent: &crate::domain::LegIntentV1,
    observation: &LegLiveObservationV1,
) -> Result<()> {
    let mut drift: Vec<&'static str> = Vec::new();
    match intent.direction {
        Direction::StellarToEvm => {
            if intent.quote_source_ledger.is_none()
                || intent.quote_source_ledger != observation.quote_source_ledger
            {
                drift.push("quote_source_ledger");
            }
        }
        Direction::EvmToStellar => {
            if intent.quote_source_block.is_none()
                || intent.quote_source_block != observation.quote_source_block
            {
                drift.push("quote_source_block");
            }
        }
    }
    if intent.observed_sequence_nonce.as_deref()
        != Some(observation.observed_sequence_nonce.as_str())
    {
        drift.push("observed_sequence_nonce");
    }
    match &intent.pre_send_snapshot {
        Some(snapshot) => {
            if snapshot.source_balance_raw != observation.pre_send_snapshot.source_balance_raw {
                drift.push("source_balance_raw");
            }
            if snapshot.lockbox_raw != observation.pre_send_snapshot.lockbox_raw {
                drift.push("lockbox_raw");
            }
            if snapshot.evm_supply_raw != observation.pre_send_snapshot.evm_supply_raw {
                drift.push("evm_supply_raw");
            }
        }
        None => drift.push("pre_send_snapshot"),
    }
    if drift.is_empty() {
        return Ok(());
    }
    Err(Error::Conflict(format!(
        "leg send live re-read rejects drift before signing: {}",
        drift.join(", ")
    )))
}

/// Validates the exact quoted intent against recorded state and then re-reads
/// the live observations the quote bound, refusing the leg before any signing
/// or nonce acquisition when the source height, sequence/nonce, or pre-send
/// balance/lockbox/supply drifted. The quote performed no reservation; the
/// send acquires the sequence/nonce only after this re-read passes.
pub fn send_operation_live(
    state_path: &Path,
    intent_path: &Path,
    allow_additional_obligation: bool,
    stellar: &dyn crate::stellar::StellarChain,
    evm: &dyn crate::evm::EvmChain,
) -> Result<crate::domain::OperationV1> {
    let intent: crate::domain::LegIntentV1 = crate::state::read_json(intent_path)?;
    let operation = validate_send_intent(state_path, intent.clone(), allow_additional_obligation)?;
    let state = route_environment(state_path)?;
    let observation = observe_leg(&state, intent.direction, stellar, evm)?;
    reject_live_drift(&intent, &observation)?;
    Ok(operation)
}

/// Revalidates the exact already-selected send operation while the caller
/// holds the sender-domain mutation guard. The operation's embedded intent is
/// the single source of truth, so replacing the quote file cannot swap the
/// object that is later planned and signed.
pub fn revalidate_locked_send(
    state_path: &Path,
    operation: &crate::domain::OperationV1,
    allow_additional_obligation: bool,
    stellar: &dyn crate::stellar::StellarChain,
    evm: &dyn crate::evm::EvmChain,
) -> Result<()> {
    let crate::domain::OperationV1::SendLeg { intent, .. } = operation else {
        return Ok(());
    };
    let validated = validate_send_intent(
        state_path,
        intent.as_ref().clone(),
        allow_additional_obligation,
    )?;
    if &validated != operation {
        return Err(Error::Conflict(
            "locked leg validation produced a different operation".into(),
        ));
    }
    let state = route_environment(state_path)?;
    let observation = observe_leg(&state, intent.direction, stellar, evm)?;
    reject_live_drift(intent, &observation)
}

/// Proves the plan used the same source sequence/nonce observed by the locked
/// validation immediately before planning.
pub fn verify_plan_source_slot(
    operation: &crate::domain::OperationV1,
    plan: &crate::domain::ExecutablePlanV1,
) -> Result<()> {
    let crate::domain::OperationV1::SendLeg { vm, intent } = operation else {
        return Ok(());
    };
    let observed = intent.observed_sequence_nonce.as_deref().ok_or_else(|| {
        Error::Conflict("live send intent omitted its observed source sequence/nonce".into())
    })?;
    let planned = match vm {
        crate::domain::Vm::Stellar => plan
            .stellar
            .as_ref()
            .map(|binding| binding.sequence.as_str()),
        crate::domain::Vm::Evm => plan.evm.as_ref().map(|binding| binding.nonce.as_str()),
    }
    .ok_or_else(|| Error::Conflict("send plan omitted its source binding".into()))?;
    if planned != observed {
        return Err(Error::Conflict(format!(
            "send plan source slot {planned} differs from locked observation {observed}"
        )));
    }
    Ok(())
}

/// Returns true only after the successful EVM receipt survived into a later
/// block and a fresh canonical receipt lookup still binds the same block.
pub fn evm_source_finalized(
    intent: &crate::domain::LegIntentV1,
    first: &crate::evm::EvmReceiptV1,
    latest_block: u64,
    canonical: Option<&crate::evm::EvmReceiptV1>,
) -> Result<bool> {
    match intent.finality_policy {
        Some(crate::domain::LegFinalityPolicyV1::Confirmed) => {}
        None => return Err(Error::Policy("send intent has no finality policy".into())),
    }
    if first.succeeded != Some(true) {
        return Ok(false);
    }
    let block_hash = first
        .raw
        .get("blockHash")
        .and_then(serde_json::Value::as_str);
    let (Some(block_number), Some(block_hash)) = (first.block_number, block_hash) else {
        return Ok(false);
    };
    if latest_block <= block_number {
        return Ok(false);
    }
    let Some(canonical) = canonical else {
        return Ok(false);
    };
    Ok(canonical.succeeded == Some(true)
        && canonical
            .transaction_hash
            .eq_ignore_ascii_case(&first.transaction_hash)
        && canonical.block_number == Some(block_number)
        && canonical
            .raw
            .get("blockHash")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case(block_hash)))
}

/// Returns true only after the successful Stellar transaction remains in the
/// same closed ledger while the RPC has advanced to a later ledger.
pub fn stellar_source_finalized(
    intent: &crate::domain::LegIntentV1,
    first: &crate::stellar::StellarTransactionStatusV1,
    latest_ledger: u32,
    canonical: &crate::stellar::StellarTransactionStatusV1,
) -> Result<bool> {
    match intent.finality_policy {
        Some(crate::domain::LegFinalityPolicyV1::Confirmed) => {}
        None => return Err(Error::Policy("send intent has no finality policy".into())),
    }
    let Some(ledger) = first.ledger else {
        return Ok(false);
    };
    Ok(first.status == "success"
        && latest_ledger > ledger
        && canonical.status == "success"
        && canonical.ledger == Some(ledger))
}

fn required_config<'a>(
    state: &'a crate::domain::RouteStateV1,
    key: &str,
) -> Result<&'a serde_json::Value> {
    state
        .effective_config
        .get(key)
        .ok_or_else(|| Error::Custody(format!("effective route is missing {key}")))
}

fn required_config_string(state: &crate::domain::RouteStateV1, key: &str) -> Result<String> {
    required_config(state, key)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::Custody(format!("effective route field {key} is not a string")))
}

fn build_source_message_record(
    state: &crate::domain::RouteStateV1,
    intent: &crate::domain::LegIntentV1,
    evidence: crate::layerzero::SourceSendEvidenceV1,
    source_height: String,
    source_transaction: String,
) -> Result<crate::domain::MessageRecordV1> {
    use sha2::{Digest as _, Sha256};

    if crate::canonical_sha256(&state.effective_config)? != intent.config_snapshot_sha256 {
        return Err(Error::Conflict(
            "effective route config changed after the source-send intent was signed".into(),
        ));
    }
    if evidence.encoded_packet.len() < 153 {
        return Err(Error::Custody(
            "LayerZero OFT packet is shorter than header, GUID, and OFT message".into(),
        ));
    }
    let packet_header = &evidence.encoded_packet[..81];
    let guid: [u8; 32] = evidence.encoded_packet[81..113]
        .try_into()
        .map_err(|_| Error::Custody("LayerZero packet GUID is malformed".into()))?;
    let message = &evidence.encoded_packet[113..];
    let header = crate::layerzero::decode_packet_header(&hex::encode(packet_header))?;
    let expected_recipient = match intent.direction {
        Direction::StellarToEvm => crate::codec::evm_address_to_bytes32(&intent.to)?,
        Direction::EvmToStellar => crate::codec::strkey_to_bytes32(&intent.to)?,
    };
    if message[..32] != expected_recipient {
        return Err(Error::Custody(
            "OFT packet recipient differs from the signed leg intent".into(),
        ));
    }
    let amount_sd = u64::from_be_bytes(
        message[32..40]
            .try_into()
            .map_err(|_| Error::Custody("OFT packet amount is malformed".into()))?,
    );
    let source_decimals = match intent.direction {
        Direction::StellarToEvm => state.asset.local_decimals,
        Direction::EvmToStellar => required_config_string(state, "asset:evm_decimals")?
            .parse()
            .map_err(|_| Error::Custody("asset:evm_decimals is not a u8 decimal".into()))?,
    };
    let event_amount_sd = crate::codec::to_shared(evidence.amount_received_ld, source_decimals)?;
    if u128::from(amount_sd) != event_amount_sd {
        return Err(Error::Custody(
            "OFT packet amount differs from the authenticated OFTSent event".into(),
        ));
    }
    let requested: u128 = intent
        .amount_raw
        .parse()
        .map_err(|_| Error::Custody("leg intent amount is not decimal".into()))?;
    let minimum: u128 = intent
        .minimum_received_raw
        .parse()
        .map_err(|_| Error::Custody("leg intent minimum amount is not decimal".into()))?;
    if evidence.amount_sent_ld > requested
        || evidence.amount_received_ld > evidence.amount_sent_ld
        || evidence.amount_received_ld < minimum
    {
        return Err(Error::Custody(
            "authenticated OFTSent amounts violate the signed leg intent".into(),
        ));
    }

    let (source_vm, destination_vm, source_eid, destination_eid) = match intent.direction {
        Direction::StellarToEvm => (
            crate::domain::Vm::Stellar,
            crate::domain::Vm::Evm,
            state.identity.stellar_eid,
            state.identity.evm_eid,
        ),
        Direction::EvmToStellar => (
            crate::domain::Vm::Evm,
            crate::domain::Vm::Stellar,
            state.identity.evm_eid,
            state.identity.stellar_eid,
        ),
    };
    if header.source_eid != source_eid || header.destination_eid != destination_eid {
        return Err(Error::Custody(
            "LayerZero packet EIDs differ from the route".into(),
        ));
    }
    let send_key = crate::route::config_key_send_library(source_vm, destination_eid);
    let configured_send_library = required_config_string(state, &send_key)?;
    if !configured_send_library.eq_ignore_ascii_case(&evidence.send_library) {
        return Err(Error::Custody(
            "PacketSent send library differs from the effective route".into(),
        ));
    }
    let receive_key = crate::route::config_key_receive_library(destination_vm, source_eid);
    let current_receive_library = required_config_string(state, &receive_key)?;
    let uln_key = crate::route::config_key_uln_config(source_vm, destination_eid, "send")?;
    let uln = required_config(state, &uln_key)?;
    let executor_key = crate::route::config_key_executor_config(source_vm, destination_eid);
    let executor = required_config(state, &executor_key)?;
    let uln_typed: crate::layerzero::UlnConfigType3V1 = serde_json::from_value(uln.clone())?;
    let dvn_snapshot = serde_json::json!({
        "required_dvns": uln_typed.required_dvns,
        "optional_dvns": uln_typed.optional_dvns,
        "optional_threshold": uln_typed.optional_threshold,
    });
    let mut payload = Vec::with_capacity(32 + message.len());
    payload.extend(guid);
    payload.extend(message);
    let evidence_sha256 = crate::canonical_sha256(&serde_json::json!({
        "source_transaction": source_transaction,
        "source_height": source_height,
        "source_event_coordinate": evidence.event_coordinate,
        "packet_sha256": hex::encode(Sha256::digest(&evidence.encoded_packet)),
    }))?;
    let (stage, net_locked_raw, burned_raw) = match intent.direction {
        Direction::StellarToEvm => (
            crate::domain::MessageStageV1::ForwardLocked,
            evidence.amount_received_ld.to_string(),
            "0".to_string(),
        ),
        Direction::EvmToStellar => (
            crate::domain::MessageStageV1::ReverseBurned,
            "0".to_string(),
            evidence.amount_sent_ld.to_string(),
        ),
    };
    Ok(crate::domain::MessageRecordV1 {
        schema_name: "message_record".into(),
        schema_version: crate::domain::SCHEMA_VERSION,
        source_eid,
        sender: hex::encode(header.sender),
        nonce: header.nonce.to_string(),
        guid: hex::encode(guid),
        direction: intent.direction,
        amount_raw: amount_sd.to_string(),
        packet_sha256: hex::encode(Sha256::digest(&evidence.encoded_packet)),
        packet_header: hex::encode(packet_header),
        message: hex::encode(message),
        payload_keccak256: hex::encode(crate::evm::keccak256_of(&payload)),
        origin: format!(
            "{source_eid}:{}:{}",
            hex::encode(header.sender),
            header.nonce
        ),
        receiver: state
            .contracts
            .get(match destination_vm {
                crate::domain::Vm::Stellar => "stellar_oft",
                crate::domain::Vm::Evm => "evm_oft",
            })
            .cloned()
            .ok_or_else(|| Error::Custody("destination OFT is not recorded".into()))?,
        current_receive_library,
        old_receive_library: None,
        receive_grace_until: None,
        send_library: evidence.send_library,
        uln_snapshot_sha256: crate::canonical_sha256(uln)?,
        dvn_snapshot_sha256: crate::canonical_sha256(&dvn_snapshot)?,
        executor_snapshot_sha256: crate::canonical_sha256(executor)?,
        config_snapshot_sha256: intent.config_snapshot_sha256.clone(),
        source_height,
        source_event_coordinate: evidence.event_coordinate,
        source_transaction,
        destination_transaction: None,
        recovery_transactions: Vec::new(),
        debited_raw: evidence.amount_sent_ld.to_string(),
        net_locked_raw,
        minted_raw: "0".into(),
        burned_raw,
        unlocked_raw: "0".into(),
        external_fee_raw: (evidence.amount_sent_ld - evidence.amount_received_ld).to_string(),
        dust_raw: (requested - evidence.amount_sent_ld).to_string(),
        reconciliation_classification: None,
        status_events: vec![crate::domain::MessageStatusEventV1 {
            stage,
            observed_at_unix: crate::now_unix()?,
            evidence_sha256,
        }],
    })
}

pub fn record_stellar_source_message(
    store: &RouteStore,
    intent: &crate::domain::LegIntentV1,
    status: &crate::stellar::StellarTransactionStatusV1,
    transaction_hash: &str,
) -> Result<()> {
    if store.has_message_for_source_transaction(transaction_hash)? {
        return Ok(());
    }
    let state = store.load_state()?;
    let endpoint = &state.identity.stellar_endpoint;
    let oft = state
        .contracts
        .get("stellar_oft")
        .ok_or_else(|| Error::Custody("source Stellar OFT is not recorded".into()))?;
    let evidence = crate::layerzero::decode_stellar_source_send(
        &status.contract_events,
        endpoint,
        oft,
        intent.destination_eid,
    )?;
    store.append_message(build_source_message_record(
        &state,
        intent,
        evidence,
        status
            .ledger
            .ok_or_else(|| Error::Custody("finalized Stellar send has no ledger".into()))?
            .to_string(),
        transaction_hash.to_string(),
    )?)
}

pub fn record_evm_source_message(
    store: &RouteStore,
    intent: &crate::domain::LegIntentV1,
    receipt: &crate::evm::EvmReceiptV1,
) -> Result<()> {
    if store.has_message_for_source_transaction(&receipt.transaction_hash)? {
        return Ok(());
    }
    let state = store.load_state()?;
    let oft = state
        .contracts
        .get("evm_oft")
        .ok_or_else(|| Error::Custody("source EVM OFT is not recorded".into()))?;
    let evidence = crate::layerzero::decode_evm_source_send(
        &receipt.logs,
        &state.identity.evm_endpoint,
        oft,
        intent.destination_eid,
    )?;
    store.append_message(build_source_message_record(
        &state,
        intent,
        evidence,
        receipt
            .block_number
            .ok_or_else(|| Error::Custody("finalized EVM send has no block".into()))?
            .to_string(),
        receipt.transaction_hash.clone(),
    )?)
}

/// Refuses signing when the live Stellar envelope fee exceeds the
/// resource-fee ceiling the quote bound.
pub fn verify_stellar_plan_fee_ceiling(
    intent: &crate::domain::LegIntentV1,
    binding: &crate::domain::StellarPlanBindingV1,
) -> Result<()> {
    let Some(crate::domain::LegFeeCeilingV1::Stellar {
        resource_fee_ceiling_raw,
    }) = &intent.fee_ceiling
    else {
        return Ok(());
    };
    let ceiling: u64 = resource_fee_ceiling_raw.parse().map_err(|_| {
        Error::Custody("recorded stellar resource fee ceiling is not decimal".into())
    })?;
    use stellar_baselib::transaction::{Transaction, TransactionBehavior as _};
    let transaction = std::panic::catch_unwind(|| {
        Transaction::from_xdr_envelope(&binding.envelope_xdr, &binding.network_passphrase)
    })
    .map_err(|_| Error::InvalidInput("invalid Stellar transaction envelope XDR".into()))?;
    if u64::from(transaction.fee) > ceiling {
        return Err(Error::Policy(format!(
            "live Stellar transaction fee {} stroops exceeds the quoted resource-fee ceiling {ceiling}",
            transaction.fee
        )));
    }
    Ok(())
}

/// Refuses signing when the live EVM fee policy exceeds the ceilings the quote
/// bound.
pub fn verify_evm_plan_fee_ceiling(
    intent: &crate::domain::LegIntentV1,
    binding: &crate::domain::EvmPlanBindingV1,
) -> Result<()> {
    let Some(crate::domain::LegFeeCeilingV1::Evm {
        max_fee_per_gas_wei,
        max_priority_fee_per_gas_wei,
        gas_limit,
    }) = &intent.fee_ceiling
    else {
        return Ok(());
    };
    let max_fee: u128 = max_fee_per_gas_wei
        .parse()
        .map_err(|_| Error::Custody("recorded EVM max fee is not decimal".into()))?;
    let max_priority: u128 = max_priority_fee_per_gas_wei
        .parse()
        .map_err(|_| Error::Custody("recorded EVM max priority fee is not decimal".into()))?;
    let live_gas: u64 = binding
        .gas_limit
        .parse()
        .map_err(|_| Error::Chain("plan gas limit is not decimal".into()))?;
    let live_max_fee: u128 = binding
        .max_fee_per_gas_wei
        .parse()
        .map_err(|_| Error::Chain("plan max fee is not decimal".into()))?;
    let live_max_priority: u128 = binding
        .max_priority_fee_per_gas_wei
        .parse()
        .map_err(|_| Error::Chain("plan max priority fee is not decimal".into()))?;
    let mut breaches: Vec<&'static str> = Vec::new();
    if live_gas > *gas_limit {
        breaches.push("gas_limit");
    }
    if live_max_fee > max_fee {
        breaches.push("max_fee_per_gas_wei");
    }
    if live_max_priority > max_priority {
        breaches.push("max_priority_fee_per_gas_wei");
    }
    if !breaches.is_empty() {
        return Err(Error::Policy(format!(
            "live EVM fee policy exceeds the quoted ceiling; breached: {}",
            breaches.join(", ")
        )));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DestinationPacketState {
    Unverified,
    Verified,
    Committed,
    Executed,
}

pub trait DestinationPacketReader {
    fn packet_state(
        &self,
        state: &crate::domain::RouteStateV1,
        message: &crate::domain::MessageRecordV1,
    ) -> Result<DestinationPacketState>;
}

pub struct LiveDestinationPacketReader<'a> {
    pub stellar: &'a dyn crate::stellar::StellarChain,
    pub evm: &'a dyn crate::evm::EvmChain,
}

impl DestinationPacketReader for LiveDestinationPacketReader<'_> {
    fn packet_state(
        &self,
        state: &crate::domain::RouteStateV1,
        message: &crate::domain::MessageRecordV1,
    ) -> Result<DestinationPacketState> {
        use stellar_baselib::xdr::{Limits, ScBytes, ScVal, WriteXdr as _};

        let header = crate::layerzero::qualify_message_for_route(state, message)?;
        let payload: [u8; 32] = hex::decode(message.payload_keccak256.trim_start_matches("0x"))
            .map_err(|_| Error::Custody("payload hash is not hex".into()))?
            .try_into()
            .map_err(|_| Error::Custody("payload hash must be 32 bytes".into()))?;
        if message.direction == Direction::EvmToStellar {
            let encode = |value: ScVal| {
                value
                    .to_xdr(Limits::none())
                    .map(hex::encode)
                    .map_err(|error| {
                        Error::Chain(format!("Stellar view arg encoding failed: {error}"))
                    })
            };
            let bytes = |value: Vec<u8>| {
                Ok::<_, Error>(ScVal::Bytes(ScBytes(value.try_into().map_err(
                    |error| Error::InvalidInput(format!("Soroban bytes too large: {error}")),
                )?)))
            };
            let packet_header = bytes(
                hex::decode(message.packet_header.trim_start_matches("0x"))
                    .map_err(|_| Error::Custody("packet header is not hex".into()))?,
            )?;
            let payload_hash = bytes(payload.to_vec())?;
            let verifiable = match self.stellar.invoke_view(
                &message.current_receive_library,
                "verifiable",
                &[encode(packet_header)?, encode(payload_hash.clone())?],
                &state
                    .contracts
                    .get("stellar_recovery_executor")
                    .cloned()
                    .unwrap_or_else(|| state.contracts["stellar_owner"].clone()),
            )? {
                ScVal::Bool(value) => value,
                _ => {
                    return Err(Error::Chain(
                        "Stellar receive ULN verifiable returned non-bool".into(),
                    ))
                }
            };
            let receiver = crate::layerzero::stellar_address(&message.receiver)?;
            let inbound_nonce = match self.stellar.invoke_view(
                &state.identity.stellar_endpoint,
                "inbound_nonce",
                &[
                    encode(receiver.clone())?,
                    encode(ScVal::U32(header.source_eid))?,
                    encode(bytes(header.sender.to_vec())?)?,
                ],
                &state.contracts["stellar_owner"],
            )? {
                ScVal::U64(value) => value,
                _ => {
                    return Err(Error::Chain(
                        "Stellar inbound_nonce returned non-u64".into(),
                    ))
                }
            };
            let committed = match self.stellar.invoke_view(
                &state.identity.stellar_endpoint,
                "inbound_payload_hash",
                &[
                    encode(receiver)?,
                    encode(ScVal::U32(header.source_eid))?,
                    encode(bytes(header.sender.to_vec())?)?,
                    encode(ScVal::U64(header.nonce))?,
                ],
                &state.contracts["stellar_owner"],
            )? {
                ScVal::Bytes(value) => value.0.as_slice() == payload,
                ScVal::Void => false,
                _ => {
                    return Err(Error::Chain(
                        "Stellar inbound_payload_hash returned an unexpected value".into(),
                    ))
                }
            };
            return Ok(if inbound_nonce >= header.nonce && !committed {
                DestinationPacketState::Executed
            } else if committed {
                DestinationPacketState::Committed
            } else if verifiable {
                DestinationPacketState::Verified
            } else {
                DestinationPacketState::Unverified
            });
        }

        let receiver = crate::evm::parse_address(&message.receiver)?;
        let endpoint = crate::evm::parse_address(&state.identity.evm_endpoint)?;
        let call = |to, signature: &str, words: Vec<[u8; 32]>| {
            let mut calldata = crate::evm::keccak256_of(signature.as_bytes())[..4].to_vec();
            calldata.extend(words.into_iter().flatten());
            crate::block_on_result(self.evm.call(to, calldata))
        };
        let address_word = |address: alloy::primitives::Address| {
            let mut word = [0u8; 32];
            word[12..].copy_from_slice(address.as_slice());
            word
        };
        let u32_word = |value: u32| {
            let mut word = [0u8; 32];
            word[28..].copy_from_slice(&value.to_be_bytes());
            word
        };
        let u64_word = |value: u64| {
            let mut word = [0u8; 32];
            word[24..].copy_from_slice(&value.to_be_bytes());
            word
        };
        let inbound_nonce_bytes = call(
            endpoint,
            "inboundNonce(address,uint32,bytes32)",
            vec![
                address_word(receiver),
                u32_word(header.source_eid),
                header.sender,
            ],
        )?;
        if inbound_nonce_bytes.len() != 32 {
            return Err(Error::Chain(
                "EVM inboundNonce returned malformed data".into(),
            ));
        }
        let mut inbound_nonce = [0u8; 8];
        inbound_nonce.copy_from_slice(&inbound_nonce_bytes[24..32]);
        let inbound_nonce = u64::from_be_bytes(inbound_nonce);
        let committed_bytes = call(
            endpoint,
            "inboundPayloadHash(address,uint32,bytes32,uint64)",
            vec![
                address_word(receiver),
                u32_word(header.source_eid),
                header.sender,
                u64_word(header.nonce),
            ],
        )?;
        if committed_bytes.len() != 32 {
            return Err(Error::Chain(
                "EVM inboundPayloadHash returned malformed data".into(),
            ));
        }
        let committed = committed_bytes.as_slice() == payload;
        if inbound_nonce >= header.nonce && !committed {
            return Ok(DestinationPacketState::Executed);
        }
        if committed {
            return Ok(DestinationPacketState::Committed);
        }
        use alloy::sol_types::SolCall as _;
        alloy::sol! {
            interface IReceiveUlnView {
                function verifiable(bytes calldata packetHeader, bytes32 payloadHash)
                    external view returns (bool);
            }
        }
        let calldata = IReceiveUlnView::verifiableCall {
            packetHeader: hex::decode(message.packet_header.trim_start_matches("0x"))
                .map_err(|_| Error::Custody("packet header is not hex".into()))?
                .into(),
            payloadHash: alloy::primitives::FixedBytes(payload),
        }
        .abi_encode();
        let verifiable = crate::block_on_result(self.evm.call(
            crate::evm::parse_address(&message.current_receive_library)?,
            calldata,
        ))?;
        if verifiable.len() != 32 {
            return Err(Error::Chain(
                "EVM receive ULN verifiable returned malformed data".into(),
            ));
        }
        Ok(if verifiable[31] == 1 {
            DestinationPacketState::Verified
        } else {
            DestinationPacketState::Unverified
        })
    }
}

/// Applies one finalized LayerZero Scan observation to an existing message.
pub fn watch_with_scan(
    state_path: &Path,
    guid: &str,
    scan: &dyn crate::scan::ScanClient,
    destination: &dyn DestinationPacketReader,
) -> Result<CommandData> {
    let state = route_read(state_path)?;
    let store = RouteStore::open(state_path)?;
    let _lock = store.lock()?;
    let message = store
        .load_messages()?
        .into_iter()
        .find(|message| message.guid.eq_ignore_ascii_case(guid))
        .ok_or_else(|| Error::InvalidInput(format!("message guid is not recorded: {guid}")))?;
    let evidence =
        crate::block_on_result(scan.messages_by_transaction(&message.source_transaction))?
            .into_iter()
            .find(|evidence| evidence.guid.eq_ignore_ascii_case(guid))
            .ok_or_else(|| Error::Chain(format!("LayerZero Scan has no message {guid}")))?;
    let status = evidence.status.to_ascii_lowercase();
    let scan_terminal = matches!(status.as_str(), "delivered" | "succeeded" | "completed");
    if scan_terminal && evidence.destination_transaction.is_none() {
        return Err(Error::Chain(
            "LayerZero Scan terminal status is not finalized".into(),
        ));
    }
    let packet_state = destination.packet_state(&state, &message)?;
    if scan_terminal && packet_state != DestinationPacketState::Executed {
        return Err(Error::Conflict(
            "LayerZero Scan claims terminal delivery but destination RPC does not".into(),
        ));
    }
    let (verified, committed, terminal) = match message.direction {
        Direction::StellarToEvm => (
            crate::domain::MessageStageV1::ForwardVerified,
            crate::domain::MessageStageV1::ForwardCommitted,
            crate::domain::MessageStageV1::ForwardMinted,
        ),
        Direction::EvmToStellar => (
            crate::domain::MessageStageV1::ReverseVerified,
            crate::domain::MessageStageV1::ReverseCommitted,
            crate::domain::MessageStageV1::ReverseUnlocked,
        ),
    };
    let last = message
        .status_events
        .iter()
        .rev()
        .find(|event| event.stage != crate::domain::MessageStageV1::Reobserved)
        .ok_or_else(|| Error::Custody("message has no custody stage".into()))?
        .stage;
    let mut stages = match packet_state {
        DestinationPacketState::Unverified => Vec::new(),
        DestinationPacketState::Verified => vec![verified],
        DestinationPacketState::Committed => vec![verified, committed],
        DestinationPacketState::Executed if scan_terminal => vec![verified, committed, terminal],
        DestinationPacketState::Executed => Vec::new(),
    };
    if let Some(position) = stages.iter().position(|stage| *stage == last) {
        stages.drain(..=position);
    }
    let observed_at_unix = crate::now_unix()?;
    let evidence_sha256 = crate::canonical_sha256(&serde_json::json!({
        "scan": evidence,
        "destination_state": format!("{packet_state:?}")
    }))?;
    if stages.is_empty() {
        store.append_message_event(
            &message.identity(),
            crate::domain::MessageStatusEventV1 {
                stage: crate::domain::MessageStageV1::Reobserved,
                observed_at_unix,
                evidence_sha256: evidence_sha256.clone(),
            },
        )?;
    } else {
        for stage in stages {
            let event = crate::domain::MessageStatusEventV1 {
                stage,
                observed_at_unix,
                evidence_sha256: evidence_sha256.clone(),
            };
            if stage == terminal {
                store.append_message_destination_event(
                    &message.identity(),
                    evidence.destination_transaction.clone().ok_or_else(|| {
                        Error::Chain("terminal destination transaction is absent".into())
                    })?,
                    event,
                )?;
            } else {
                store.append_message_event(&message.identity(), event)?;
            }
        }
    }
    Ok(CommandData {
        result: serde_json::json!({
            "guid": guid,
            "scan": evidence,
            "destination_state": format!("{packet_state:?}"),
            "terminal": scan_terminal && packet_state == DestinationPacketState::Executed,
        }),
        artifact: None,
    })
}

/// Derives the permissionless recovery operation from exact packet custody.
pub fn recovery_operation(state_path: &Path, guid: &str) -> Result<crate::domain::OperationV1> {
    let state = route_environment(state_path)?;
    let message = RouteStore::open(state_path)?
        .load_messages()?
        .into_iter()
        .find(|message| message.guid.eq_ignore_ascii_case(guid))
        .ok_or_else(|| Error::InvalidInput(format!("message guid is not recorded: {guid}")))?;
    crate::layerzero::qualify_message_for_route(&state, &message)?;
    let stage = message
        .status_events
        .iter()
        .rev()
        .find(|event| event.stage != crate::domain::MessageStageV1::Reobserved)
        .ok_or_else(|| Error::Custody("message has no custody stage".into()))?
        .stage;
    let vm = match message.direction {
        Direction::StellarToEvm => crate::domain::Vm::Evm,
        Direction::EvmToStellar => crate::domain::Vm::Stellar,
    };
    let verified = matches!(
        (message.direction, stage),
        (
            Direction::StellarToEvm,
            crate::domain::MessageStageV1::ForwardVerified
        ) | (
            Direction::EvmToStellar,
            crate::domain::MessageStageV1::ReverseVerified
        )
    );
    let committed = matches!(
        (message.direction, stage),
        (
            Direction::StellarToEvm,
            crate::domain::MessageStageV1::ForwardCommitted
        ) | (
            Direction::EvmToStellar,
            crate::domain::MessageStageV1::ReverseCommitted
        )
    );
    if verified {
        let remote_eid = message.source_eid;
        let current_key = crate::route::config_key_receive_library(vm, remote_eid);
        let effective_library = state
            .effective_config
            .get(&current_key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Custody(format!("recovery requires effective {current_key}")))?;
        if !effective_library.eq_ignore_ascii_case(&message.current_receive_library) {
            let now = crate::now_unix()?;
            if message
                .old_receive_library
                .as_deref()
                .is_none_or(|old| !old.eq_ignore_ascii_case(&message.current_receive_library))
                || message.receive_grace_until.is_none_or(|until| now > until)
            {
                return Err(Error::Policy(
                    "send-time receive library is no longer current or inside its grace period"
                        .into(),
                ));
            }
        }
        return Ok(crate::domain::OperationV1::CommitVerification {
            vm,
            message: Box::new(message),
        });
    }
    if committed {
        return Ok(crate::domain::OperationV1::ExecuteReceive {
            vm,
            message: Box::new(message),
        });
    }
    Err(Error::Policy(format!(
        "message {guid} has neither verified-not-committed nor committed-not-executed evidence"
    )))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct EvidenceBundleV1 {
    schema_name: String,
    schema_version: u32,
    route_id: String,
    desired_sha256: String,
    observed_lockbox_raw: String,
    normalized_evm_supply_raw: String,
    messages: Vec<crate::domain::MessageRecordV1>,
}

fn reject_secret_keys(value: &serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                let key = key.to_ascii_lowercase();
                if ["private_key", "secret", "seed", "password", "mnemonic"]
                    .iter()
                    .any(|needle| key.contains(needle))
                {
                    return Err(Error::Policy(
                        "evidence bundle contains secret-bearing fields".into(),
                    ));
                }
                reject_secret_keys(value)?;
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                reject_secret_keys(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Imports exact public packet evidence into a fresh append-only message log.
#[derive(serde::Deserialize, serde::Serialize)]
struct EvidenceImportMarkerV1 {
    schema_name: String,
    schema_version: u32,
    bundle_sha256: String,
    evidence: EvidenceBundleV1,
}

/// Local evidence custody remains allowed for known mainnet routes.
pub fn import_evidence(state_path: &Path, bundle: &Path, write: bool) -> Result<CommandData> {
    let raw: serde_json::Value = crate::state::read_json(bundle)?;
    reject_secret_keys(&raw)?;
    let evidence: EvidenceBundleV1 = serde_json::from_value(raw)?;
    if evidence.schema_name != "evidence_bundle"
        || evidence.schema_version != crate::domain::SCHEMA_VERSION
    {
        return Err(Error::InvalidInput(
            "unsupported evidence bundle schema".into(),
        ));
    }
    evidence
        .observed_lockbox_raw
        .parse::<u128>()
        .map_err(|_| Error::InvalidInput("observed_lockbox_raw must be decimal".into()))?;
    evidence
        .normalized_evm_supply_raw
        .parse::<u128>()
        .map_err(|_| Error::InvalidInput("normalized_evm_supply_raw must be decimal".into()))?;
    if evidence.messages.is_empty() {
        return Err(Error::InvalidInput(
            "evidence bundle contains no packet evidence".into(),
        ));
    }
    let digest = crate::canonical_sha256(&evidence)?;
    let message_count = evidence.messages.len();
    if !write {
        let state = route_read(state_path)?;
        bind_evidence(&state, &evidence)?;
        return Ok(CommandData {
            result: serde_json::json!({
                "verified": true,
                "written": false,
                "bundle_sha256": digest,
                "message_count": message_count,
            }),
            artifact: None,
        });
    }
    let store = RouteStore::open(state_path)?;
    let _lock = store.lock()?;
    let marker_path = store.root().join(".evidence-import.json");
    let marker = if marker_path.exists() {
        let marker: EvidenceImportMarkerV1 = crate::state::read_json(&marker_path)?;
        if marker.schema_name != "evidence_import_marker"
            || marker.schema_version != crate::domain::SCHEMA_VERSION
            || marker.bundle_sha256 != digest
            || crate::canonical_sha256(&marker.evidence)? != digest
        {
            return Err(Error::Conflict(
                "pending evidence import binds a different bundle".into(),
            ));
        }
        marker
    } else {
        let state = store.load_state()?;
        crate::environment::classify(&state.identity)?;
        bind_evidence(&state, &evidence)?;
        if !store.load_messages()?.is_empty()
            || state
                .effective_config
                .contains_key("custody:evidence_bundle_sha256")
        {
            return Err(Error::Conflict(
                "historical evidence import requires an unimported empty message ledger".into(),
            ));
        }
        let marker = EvidenceImportMarkerV1 {
            schema_name: "evidence_import_marker".into(),
            schema_version: crate::domain::SCHEMA_VERSION,
            bundle_sha256: digest.clone(),
            evidence: evidence.clone(),
        };
        crate::state::write_create_new_json(&marker_path, &marker)?;
        marker
    };
    let existing = store.load_messages()?;
    if existing.is_empty() {
        store.append_messages_batch(marker.evidence.messages.clone())?;
    } else if existing != marker.evidence.messages {
        return Err(Error::Conflict(
            "message ledger differs from the pending evidence import".into(),
        ));
    }
    let mut imported = store.load_state()?;
    bind_evidence(&imported, &marker.evidence)?;
    imported.effective_config.insert(
        "custody:observed_lockbox_raw".into(),
        serde_json::Value::String(marker.evidence.observed_lockbox_raw.clone()),
    );
    imported.effective_config.insert(
        "custody:normalized_evm_supply_raw".into(),
        serde_json::Value::String(marker.evidence.normalized_evm_supply_raw.clone()),
    );
    imported.effective_config.insert(
        "custody:evidence_bundle_sha256".into(),
        serde_json::Value::String(digest.clone()),
    );
    store.save_state(&imported)?;
    std::fs::remove_file(&marker_path)?;
    std::fs::File::open(store.root())?.sync_all()?;
    Ok(CommandData {
        result: serde_json::json!({
            "verified": true,
            "written": true,
            "bundle_sha256": digest,
            "message_count": message_count,
        }),
        artifact: None,
    })
}

/// The bundle must bind to the route state it is imported into.
fn bind_evidence(state: &crate::domain::RouteStateV1, evidence: &EvidenceBundleV1) -> Result<()> {
    if evidence.route_id != state.route_id || evidence.desired_sha256 != state.desired_sha256 {
        return Err(Error::Conflict(
            "evidence bundle does not bind to this route state".into(),
        ));
    }
    Ok(())
}
