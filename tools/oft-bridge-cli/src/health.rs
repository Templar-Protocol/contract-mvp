use std::collections::BTreeMap;

use crate::{
    domain::{Environment, MessageStageV1},
    error::{Error, Result},
    output::CommandData,
    state::{OperationEventV1, RouteStore},
};

fn finding(code: &str, detail: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"code": code, "detail": detail})
}

fn raw_config(config: &BTreeMap<String, serde_json::Value>, key: &str) -> Result<Option<u64>> {
    config
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
                .ok_or_else(|| Error::Custody(format!("health field {key} is not a u64 integer")))
        })
        .transpose()
}

fn comparable_config_value(key: &str, value: &serde_json::Value) -> Result<serde_json::Value> {
    if key.starts_with("uln_send_config:") || key.starts_with("uln_receive_config:") {
        if value.as_str().is_some_and(|value| value.len() == 64) {
            return Ok(value.clone());
        }
        let config: crate::layerzero::UlnConfigType3V1 = serde_json::from_value(value.clone())?;
        return Ok(serde_json::Value::String(config.config_sha256()?));
    }
    if key.starts_with("executor_config:") {
        if value.as_str().is_some_and(|value| value.len() == 64) {
            return Ok(value.clone());
        }
        let config: crate::layerzero::ExecutorConfigType3V1 =
            serde_json::from_value(value.clone())?;
        return Ok(serde_json::Value::String(config.config_sha256()?));
    }
    if key.starts_with("receive_library_grace:") {
        return value
            .as_str()
            .map(str::to_owned)
            .or_else(|| value.as_u64().map(|value| value.to_string()))
            .map(serde_json::Value::String)
            .ok_or_else(|| Error::Custody(format!("health field {key} is not a u64 integer")));
    }
    Ok(value.clone())
}

pub fn check(state_path: &std::path::Path) -> Result<Vec<serde_json::Value>> {
    let store = RouteStore::open(state_path)?;
    let state = store.load_state()?;
    let operations = store.verify_log::<OperationEventV1>(&state.operations_log, "operations")?;
    let messages = store.load_messages()?;
    let mut findings = Vec::new();

    if state.opening_custody.is_none() {
        findings.push(finding("opening_custody_missing", serde_json::json!({})));
    }

    if state_path.join(".evidence-import.json").exists() {
        findings.push(finding(
            "evidence_import_pending",
            serde_json::json!({"path": ".evidence-import.json"}),
        ));
    }
    if let Some(opening) = state.opening_custody.as_ref() {
        match crate::artifacts::lock_sha256() {
            Ok(current) if current != opening.artifact_lock_sha256 => findings.push(finding(
                "artifact_drift",
                serde_json::json!({
                    "recorded": opening.artifact_lock_sha256,
                    "current": current,
                }),
            )),
            Err(error) => findings.push(finding(
                "artifact_verification_failed",
                serde_json::json!({"error_code": error.code(), "message": error.to_string()}),
            )),
            _ => {}
        }
    }
    for contract in [
        "stellar_owner",
        "stellar_delegate",
        "evm_owner",
        "evm_delegate",
        "stellar_oft",
        "evm_oft",
    ] {
        if state.contracts.get(contract).is_none_or(String::is_empty) {
            findings.push(finding(
                "contract_binding_missing",
                serde_json::json!({"field": contract}),
            ));
        }
    }
    for key in state.requested_config.keys() {
        let requested = comparable_config_value(key, &state.requested_config[key])?;
        let effective = state
            .effective_config
            .get(key)
            .map(|value| comparable_config_value(key, value))
            .transpose()?;
        if effective.as_ref() != Some(&requested) {
            findings.push(finding(
                "config_drift",
                serde_json::json!({
                    "field": key,
                    "requested": requested,
                    "effective": effective,
                }),
            ));
        }
    }

    if let (Some(stellar_oft), Some(evm_oft)) = (
        state.contracts.get("stellar_oft"),
        state.contracts.get("evm_oft"),
    ) {
        for (eid, expected) in [
            (
                state.identity.evm_eid,
                format!(
                    "0x{}",
                    hex::encode(crate::codec::evm_address_to_bytes32(evm_oft)?)
                ),
            ),
            (
                state.identity.stellar_eid,
                format!(
                    "0x{}",
                    hex::encode(crate::codec::strkey_to_bytes32(stellar_oft)?)
                ),
            ),
        ] {
            let actual = state.contracts.get(&format!("peer:{eid}"));
            if actual != Some(&expected) {
                findings.push(finding(
                    "route_peer_drift",
                    serde_json::json!({"eid": eid, "requested": expected, "effective": actual}),
                ));
            }
        }
    }
    for (key, expected) in state
        .requested_config
        .iter()
        .filter(|(key, _)| key.starts_with("authority:stellar:role:"))
    {
        let role = key.trim_start_matches("authority:stellar:role:");
        let observed = state
            .contracts
            .get(&format!("stellar_role:{role}"))
            .cloned()
            .map(serde_json::Value::String)
            .or_else(|| state.effective_config.get(key).cloned());
        if observed.as_ref() != Some(expected) {
            findings.push(finding(
                "authority_role_drift",
                serde_json::json!({"role": role, "requested": expected, "effective": observed}),
            ));
        }
    }

    for vm in ["stellar", "evm"] {
        let key = format!("containment:{vm}");
        if let Some(value) = state.effective_config.get(&key) {
            let status = value.get("status").and_then(serde_json::Value::as_str);
            let snapshot = value
                .get("snapshot_sha256")
                .and_then(serde_json::Value::as_str);
            if !matches!(status, Some("confirmed" | "restored"))
                || snapshot.is_none()
                || snapshot.is_some_and(|digest| {
                    !state
                        .effective_config
                        .contains_key(&format!("containment:snapshot:{digest}"))
                })
            {
                findings.push(finding(
                    "containment_state_incomplete",
                    serde_json::json!({"vm": vm, "state": value}),
                ));
            }
        }
    }

    let current_ledger = raw_config(&state.effective_config, "ttl:current_ledger")?;
    for (live_key, margin_key, code) in [
        (
            "ttl:instance_live_until",
            "ttl:min_instance_margin",
            "ttl_instance_risk",
        ),
        (
            "ttl:persistent_min_live_until",
            "ttl:min_persistent_margin",
            "ttl_persistent_risk",
        ),
        (
            "ttl:archive_live_until",
            "ttl:min_archive_margin",
            "ttl_archive_risk",
        ),
    ] {
        match (
            current_ledger,
            raw_config(&state.effective_config, live_key)?,
            raw_config(&state.requested_config, margin_key)?,
        ) {
            (Some(current), Some(live_until), Some(minimum))
                if live_until.saturating_sub(current) < minimum =>
            {
                findings.push(finding(
                    code,
                    serde_json::json!({
                        "current_ledger": current.to_string(),
                        "live_until": live_until.to_string(),
                        "minimum_margin": minimum.to_string(),
                    }),
                ));
            }
            (_, None, Some(_)) | (None, Some(_), Some(_)) => findings.push(finding(
                "ttl_observation_missing",
                serde_json::json!({"field": live_key}),
            )),
            _ => {}
        }
    }

    let now = crate::now_unix()?;
    let stuck_after = raw_config(&state.requested_config, "health:max_stuck_seconds")?;
    let mut nonces: BTreeMap<(u32, String), Vec<u64>> = BTreeMap::new();
    for message in &messages {
        let latest = message
            .status_events
            .iter()
            .rev()
            .find(|event| event.stage != MessageStageV1::Reobserved)
            .ok_or_else(|| {
                Error::Custody(format!("message {} has no custody status", message.guid))
            })?;
        let terminal = matches!(
            latest.stage,
            MessageStageV1::ForwardMinted | MessageStageV1::ReverseUnlocked
        );
        if !terminal && message.reconciliation_classification.is_none() {
            findings.push(finding(
                "unknown_custody_record",
                serde_json::json!({"guid": message.guid, "stage": latest.stage}),
            ));
        }
        if let Some(max_age) = stuck_after {
            if !terminal && now.saturating_sub(latest.observed_at_unix) > max_age {
                findings.push(finding(
                    "stuck_delivery",
                    serde_json::json!({"guid": message.guid, "stage": latest.stage}),
                ));
            }
        }
        let nonce = message
            .nonce
            .parse()
            .map_err(|_| Error::Custody(format!("message {} has invalid nonce", message.guid)))?;
        nonces
            .entry((message.source_eid, message.sender.clone()))
            .or_default()
            .push(nonce);
    }
    for ((source_eid, sender), mut values) in nonces {
        values.sort_unstable();
        for pair in values.windows(2) {
            if pair[1] != pair[0] + 1 {
                findings.push(finding(
                    "source_nonce_gap",
                    serde_json::json!({
                        "source_eid": source_eid,
                        "sender": sender,
                        "before": pair[0].to_string(),
                        "after": pair[1].to_string(),
                    }),
                ));
            }
        }
    }
    match crate::reconcile::run_command(state_path, false) {
        Ok(data) if data.result["deficit_raw"] != "0" => {
            findings.push(finding("custody_deficit", data.result))
        }
        Err(error) => findings.push(finding(
            "reconciliation_unavailable",
            serde_json::json!({"error_code": error.code(), "message": error.to_string()}),
        )),
        _ => {}
    }
    if state.identity.environment == Environment::StellarMainnetEthereum && !operations.is_empty() {
        findings.push(finding(
            "mainnet_mutation_attempt_recorded",
            serde_json::json!({"operation_events": operations.len()}),
        ));
    }
    Ok(findings)
}

pub fn command(state_path: &std::path::Path) -> Result<CommandData> {
    let findings = check(state_path)?;
    if !findings.is_empty() {
        return Err(Error::Health(findings));
    }
    Ok(CommandData {
        result: serde_json::json!({"healthy": true, "findings": []}),
        artifact: None,
    })
}
