use std::collections::BTreeSet;

use crate::{
    domain::{Direction, MessageRecordV1, MessageStageV1},
    error::{Error, Result},
};

/// Uniquely identifies an appended packet record:
/// `(source_eid, sender, nonce, guid)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct PacketKey {
    pub source_eid: u32,
    pub sender: [u8; 32],
    pub nonce: u64,
    pub guid: [u8; 32],
}

/// Mutually exclusive custody stages a packet can occupy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketStage {
    /// Locked on Stellar, not yet minted on EVM.
    ForwardLocked,
    /// Burned on EVM, not yet unlocked on Stellar.
    ReverseAfterBurn,
    /// Fully settled on both chains; contributes no outstanding obligation.
    Delivered,
}

impl PacketStage {
    /// Classifies a message-ledger stage kind. Unknown kinds are custody
    /// failures so unknown records can never contribute silently.
    pub fn classify(kind: &str) -> Result<Self> {
        match kind {
            "forward_locked" => Ok(Self::ForwardLocked),
            "reverse_after_burn" => Ok(Self::ReverseAfterBurn),
            "delivered" => Ok(Self::Delivered),
            other => Err(Error::Custody(format!(
                "unknown packet stage kind {other:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketRecord {
    pub key: PacketKey,
    pub stage: PacketStage,
    pub amount_raw: u128,
}

/// Outstanding forward and reverse obligations in raw common units.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StageDeltas {
    pub pending_forward_raw: u128,
    pub pending_reverse_raw: u128,
}

/// Observed on-chain custody snapshot in raw common units. External fees are
/// paid in the native gas asset outside the lockbox, so they are reported,
/// never folded into the conservation equation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ObservedCustody {
    pub observed_lockbox_raw: u128,
    pub normalized_evm_supply_raw: u128,
    pub lockbox_retained_fee_or_dust_raw: u128,
    pub external_fee_reported_raw: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconcileReport {
    pub expected_lockbox_raw: u128,
    pub observed_lockbox_raw: u128,
    pub deficit_raw: u128,
    pub surplus_raw: u128,
    pub external_fee_reported_raw: u128,
}

/// Aggregates stage deltas from packet records. Each unique packet key
/// contributes exactly one mutually exclusive delta; duplicate keys are
/// conflicts and unknown stages are custody failures, so neither ever
/// contributes silently.
pub fn aggregate(records: &[PacketRecord]) -> Result<StageDeltas> {
    let mut seen = BTreeSet::new();
    let mut deltas = StageDeltas::default();
    for record in records {
        if !seen.insert(record.key) {
            return Err(Error::Conflict(format!(
                "duplicate packet record for guid {}",
                hex::encode(record.key.guid)
            )));
        }
        let target = match record.stage {
            PacketStage::ForwardLocked => &mut deltas.pending_forward_raw,
            PacketStage::ReverseAfterBurn => &mut deltas.pending_reverse_raw,
            PacketStage::Delivered => continue,
        };
        *target = (*target)
            .checked_add(record.amount_raw)
            .ok_or_else(|| Error::Custody("pending obligation overflow".into()))?;
    }
    Ok(deltas)
}

/// Computes the custody conservation report where the expected lockbox equals
/// the sum of normalized EVM supply, pending reverse after burn, lockbox
/// retained fee/dust, and pending forward locked-not-minted; deficit is
/// `max(0, expected - observed)` and surplus is `max(0, observed - expected)`.
pub fn reconcile(observed: &ObservedCustody, deltas: &StageDeltas) -> Result<ReconcileReport> {
    let expected_lockbox_raw = observed
        .normalized_evm_supply_raw
        .checked_add(deltas.pending_reverse_raw)
        .and_then(|partial| partial.checked_add(deltas.pending_forward_raw))
        .and_then(|partial| partial.checked_add(observed.lockbox_retained_fee_or_dust_raw))
        .ok_or_else(|| Error::Custody("expected lockbox computation overflows".into()))?;
    let deficit_raw = expected_lockbox_raw.saturating_sub(observed.observed_lockbox_raw);
    let surplus_raw = observed
        .observed_lockbox_raw
        .saturating_sub(expected_lockbox_raw);
    Ok(ReconcileReport {
        expected_lockbox_raw,
        observed_lockbox_raw: observed.observed_lockbox_raw,
        deficit_raw,
        surplus_raw,
        external_fee_reported_raw: observed.external_fee_reported_raw,
    })
}

fn parse_raw(value: &str, field: &str) -> Result<u128> {
    value.parse().map_err(|_| {
        Error::Custody(format!(
            "message {field} must be a canonical nonnegative decimal integer"
        ))
    })
}

fn packet_records(messages: &[MessageRecordV1]) -> Result<(Vec<PacketRecord>, u128, u128)> {
    let mut records = Vec::with_capacity(messages.len());
    let mut retained = 0u128;
    let mut external = 0u128;
    for message in messages {
        let nonce = message
            .nonce
            .parse::<u64>()
            .map_err(|_| Error::Custody("message nonce is not a u64 decimal integer".into()))?;
        let header = hex::decode(message.packet_header.trim_start_matches("0x"))
            .map_err(|_| Error::Custody("message packet_header is not hex".into()))?;
        if header.len() != 81 || header[0] != 1 {
            return Err(Error::Custody(
                "message packet_header must be the 81-byte LayerZero v1 header".into(),
            ));
        }
        let header_nonce = u64::from_be_bytes(
            header[1..9]
                .try_into()
                .map_err(|_| Error::Custody("message header nonce is malformed".into()))?,
        );
        let header_source_eid = u32::from_be_bytes(
            header[9..13]
                .try_into()
                .map_err(|_| Error::Custody("message header source EID is malformed".into()))?,
        );
        if header_nonce != nonce || header_source_eid != message.source_eid {
            return Err(Error::Custody(
                "message identity differs from its packet header".into(),
            ));
        }
        let sender: [u8; 32] = header[13..45]
            .try_into()
            .map_err(|_| Error::Custody("message header sender is malformed".into()))?;
        let guid = decode_key(&message.guid, "guid")?;
        let stage = message
            .status_events
            .iter()
            .rev()
            .find(|event| event.stage != MessageStageV1::Reobserved)
            .ok_or_else(|| {
                Error::Custody(format!("message {} has no custody stage", message.guid))
            })?
            .stage;
        let (stage, amount_raw) = match (message.direction, stage) {
            (Direction::StellarToEvm, MessageStageV1::ForwardSourceAccepted) => {
                (PacketStage::Delivered, 0)
            }
            (
                Direction::StellarToEvm,
                MessageStageV1::ForwardLocked
                | MessageStageV1::ForwardVerified
                | MessageStageV1::ForwardCommitted,
            ) => (
                PacketStage::ForwardLocked,
                parse_raw(&message.net_locked_raw, "net_locked_raw")?,
            ),
            (Direction::StellarToEvm, MessageStageV1::ForwardMinted)
            | (Direction::EvmToStellar, MessageStageV1::ReverseUnlocked) => {
                (PacketStage::Delivered, 0)
            }
            (Direction::EvmToStellar, MessageStageV1::ReverseSourceAccepted) => {
                (PacketStage::Delivered, 0)
            }
            (
                Direction::EvmToStellar,
                MessageStageV1::ReverseBurned
                | MessageStageV1::ReverseVerified
                | MessageStageV1::ReverseCommitted,
            ) => (
                PacketStage::ReverseAfterBurn,
                parse_raw(&message.burned_raw, "burned_raw")?,
            ),
            _ => {
                return Err(Error::Custody(format!(
                    "message {} has a stage inconsistent with its direction",
                    message.guid
                )))
            }
        };
        retained = retained
            .checked_add(parse_raw(&message.dust_raw, "dust_raw")?)
            .ok_or_else(|| Error::Custody("retained dust overflow".into()))?;
        external = external
            .checked_add(parse_raw(&message.external_fee_raw, "external_fee_raw")?)
            .ok_or_else(|| Error::Custody("external fee overflow".into()))?;
        records.push(PacketRecord {
            key: PacketKey {
                source_eid: message.source_eid,
                sender,
                nonce,
                guid,
            },
            stage,
            amount_raw,
        });
    }
    Ok((records, retained, external))
}

fn decode_key(value: &str, field: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(value.trim_start_matches("0x"))
        .map_err(|_| Error::Custody(format!("message {field} is not hex")))?;
    bytes
        .try_into()
        .map_err(|_| Error::Custody(format!("message {field} must be 32 bytes")))
}

fn observed_value(
    route: &crate::domain::RouteStateV1,
    key: &str,
    fallback: u128,
    messages_present: bool,
) -> Result<u128> {
    match route.effective_config.get(key) {
        Some(value) => value
            .as_str()
            .ok_or_else(|| Error::Custody(format!("{key} observation must be a decimal string")))
            .and_then(|value| parse_raw(value, key)),
        None if messages_present => Err(Error::Custody(format!(
            "current {key} observation is required when packet history is nonempty"
        ))),
        None => Ok(fallback),
    }
}

/// `reconcile` command: verifies log integrity then reports custody from
/// state-bound observations. Fails closed when opening custody is absent.
pub fn run_command(
    state: &std::path::Path,
    fail_on_deficit: bool,
) -> Result<crate::output::CommandData> {
    let store = crate::state::RouteStore::open(state)?;
    let route = store.load_state()?;
    store.verify_log::<crate::state::OperationEventV1>(&route.operations_log, "operations")?;
    let Some(opening) = route.opening_custody.as_ref() else {
        return Err(Error::Custody(
            "opening custody is not finalized; reconciliation requires adopted baseline".into(),
        ));
    };
    let messages = store.load_messages()?;
    let (records, retained, external) = packet_records(&messages)?;
    let deltas = aggregate(&records)?;
    let observed = ObservedCustody {
        observed_lockbox_raw: observed_value(
            &route,
            "custody:observed_lockbox_raw",
            opening.lockbox_raw,
            !messages.is_empty(),
        )?,
        normalized_evm_supply_raw: observed_value(
            &route,
            "custody:normalized_evm_supply_raw",
            opening.evm_supply_raw,
            !messages.is_empty(),
        )?,
        lockbox_retained_fee_or_dust_raw: retained,
        external_fee_reported_raw: external,
    };
    let report = reconcile(&observed, &deltas)?;
    if fail_on_deficit && report.deficit_raw > 0 {
        return Err(Error::Custody(format!(
            "custody deficit {} raw; failing per --fail-on-deficit",
            report.deficit_raw
        )));
    }
    Ok(crate::output::CommandData {
        result: serde_json::json!({
            "expected_lockbox_raw": report.expected_lockbox_raw.to_string(),
            "observed_lockbox_raw": report.observed_lockbox_raw.to_string(),
            "normalized_evm_supply_raw": observed.normalized_evm_supply_raw.to_string(),
            "pending_forward_raw": deltas.pending_forward_raw.to_string(),
            "pending_reverse_raw": deltas.pending_reverse_raw.to_string(),
            "lockbox_retained_fee_or_dust_raw": retained.to_string(),
            "external_fee_reported_raw": external.to_string(),
            "deficit_raw": report.deficit_raw.to_string(),
            "surplus_raw": report.surplus_raw.to_string()
        }),
        artifact: None,
    })
}
