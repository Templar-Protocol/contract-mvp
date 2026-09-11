use stellar_baselib::keypair::KeypairBehavior as _;

use crate::{
    domain::{OperationV1, RouteStateV1},
    error::{Error, Result},
};

pub const FREEZE_ACKNOWLEDGEMENT: &str = "I understand TTL configuration freeze is irreversible";

fn frozen(state: &RouteStateV1) -> Result<bool> {
    state
        .effective_config
        .get("ttl:is_frozen")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| Error::Custody("ttl:is_frozen readback must be boolean".into()))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

pub fn set_config(
    state: &RouteStateV1,
    instance_threshold: u32,
    instance_extend_to: u32,
    persistent_threshold: u32,
    persistent_extend_to: u32,
) -> Result<OperationV1> {
    if frozen(state)? {
        return Err(Error::Policy("TTL configuration is already frozen".into()));
    }
    if instance_threshold == 0
        || persistent_threshold == 0
        || instance_extend_to < instance_threshold
        || persistent_extend_to < persistent_threshold
    {
        return Err(Error::InvalidInput(
            "TTL extend-to values must be at least their nonzero thresholds".into(),
        ));
    }
    Ok(OperationV1::SetTtlConfig {
        instance_threshold,
        instance_extend_to,
        persistent_threshold,
        persistent_extend_to,
    })
}

pub fn freeze(state: &RouteStateV1, acknowledgement: &str) -> Result<OperationV1> {
    if acknowledgement != FREEZE_ACKNOWLEDGEMENT {
        return Err(Error::InvalidInput(format!(
            "--acknowledge-irreversible must equal {FREEZE_ACKNOWLEDGEMENT:?}"
        )));
    }
    if frozen(state)? {
        return Err(Error::Conflict(
            "TTL configuration is already frozen".into(),
        ));
    }
    Ok(OperationV1::FreezeTtlConfig {
        acknowledgement: acknowledgement.into(),
    })
}

pub fn extend_instance(state: &RouteStateV1, ledgers: u32) -> Result<OperationV1> {
    if ledgers == 0 {
        return Err(Error::InvalidInput(
            "instance TTL extension must be nonzero".into(),
        ));
    }
    let observed_u32 = |key: &str| {
        state.effective_config.get(key).map(|value| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
                .ok_or_else(|| Error::Custody(format!("{key} is not a u32 integer")))
        })
    };
    if let (Some(current), Some(live_until)) = (
        observed_u32("ttl:current_ledger"),
        observed_u32("ttl:instance_live_until"),
    ) {
        let current = current?;
        let live_until = live_until?;
        if live_until.saturating_sub(current) >= ledgers {
            return Err(Error::Conflict(
                "instance TTL already has at least the requested margin".into(),
            ));
        }
    }
    Ok(OperationV1::ExtendInstanceTtl { ledgers })
}

pub fn require_classic_role_address(address: &str) -> Result<()> {
    stellar_baselib::keypair::Keypair::from_public_key(address)
        .map(|_| ())
        .map_err(|error| {
            Error::InvalidInput(format!(
                "role holder must be a classic G... account: {error}"
            ))
        })
}

pub fn require_role(role: &str) -> Result<()> {
    if role.trim().is_empty()
        || !role
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(Error::InvalidInput(
            "role must be a nonempty alphanumeric/underscore identifier".into(),
        ));
    }
    Ok(())
}
