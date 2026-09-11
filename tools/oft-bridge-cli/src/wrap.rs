//! `asset wrap` planner: resolves typed asset topology and emits the typed
//! operation plan for a non-USDC route. Every node is concrete — peers and
//! salts are derived, never placeholders. The pre-dispatch USDC classifier
//! runs before any state, artifact, or signer access.

use sha2::{Digest, Sha256};

use crate::domain::{
    AssetKind, AssetPolicyV1, DesiredRouteV1, OperationV1, SCHEMA_VERSION, SHARED_DECIMALS,
};
use crate::error::{Error, Result};

/// Deterministic plan emitted by `asset wrap`. Every operation is fully
/// bound: the EVM deployer nonce is reserved by the caller (live adapter or
/// an explicit operator value), and both peers are derived.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct WrapPlanV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub desired_sha256: String,
    pub asset: AssetPolicyV1,
    pub name: String,
    pub symbol: String,
    pub stellar_token_contract: String,
    pub stellar_salt: String,
    pub stellar_deploy_account: String,
    pub stellar_oft: String,
    pub evm_deployer: String,
    pub evm_nonce: u64,
    pub evm_oft: String,
    pub operations: Vec<OperationV1>,
}

/// Resolves the Stellar token contract for the asset: the deterministic
/// native SAC identifier for `native_sac`, or the operator-supplied contract
/// for `issued_sep41`.
pub fn token_contract(passphrase: &str, asset: &AssetPolicyV1) -> Result<String> {
    match asset.kind {
        AssetKind::NativeSac => crate::codec::derive_native_sac_contract(passphrase),
        AssetKind::IssuedSep41 => {
            if !asset.asset_id.starts_with('C') {
                return Err(Error::InvalidInput(
                    "issued_sep41 wrap requires the Stellar contract identifier as asset_id".into(),
                ));
            }
            // Parse-boundary validation of the contract StrKey.
            crate::codec::strkey_to_bytes32(&asset.asset_id)?;
            Ok(asset.asset_id.clone())
        }
        AssetKind::Usdc => Err(Error::Policy("unsupported_use_cctp".into())),
        AssetKind::TestOnly => Err(Error::InvalidInput(
            "wrap requires native_sac or issued_sep41".into(),
        )),
    }
}

/// Enforces the evidence boundary: issued assets must carry issuer custody,
/// destination acceptance, and custody-risk acceptance digests.
fn enforce_asset_evidence(asset: &AssetPolicyV1) -> Result<()> {
    if asset.kind != AssetKind::IssuedSep41 {
        return Ok(());
    }
    for (name, digest) in [
        (
            "issuer_custodian_evidence_sha256",
            asset.issuer_custodian_evidence_sha256.as_deref(),
        ),
        (
            "destination_acceptance_evidence_sha256",
            asset.destination_acceptance_evidence_sha256.as_deref(),
        ),
        (
            "custody_risk_acceptance_sha256",
            asset.custody_risk_acceptance_sha256.as_deref(),
        ),
    ] {
        match digest {
            Some(value) if !value.trim().is_empty() => {}
            _ => return Err(Error::Policy(format!("issued asset wrap requires {name}"))),
        }
    }
    Ok(())
}

/// The forbidden classic issuer must never be a route operator account.
fn enforce_issuer_exclusion(asset: &AssetPolicyV1, accounts: &[&str]) -> Result<()> {
    if let Some(forbidden) = asset.forbidden_classic_issuer.as_deref() {
        if !forbidden.trim().is_empty() && accounts.iter().any(|account| *account == forbidden) {
            return Err(Error::Policy(
                "forbidden classic issuer used as route operator".into(),
            ));
        }
    }
    Ok(())
}

/// Deterministic Stellar deploy salt for this route: `SHA-256` of the
/// canonical `(route_id, asset_id)` pair, so re-planning the same route
/// reproduces the same deployment instead of drifting.
pub fn stellar_salt(route_id: &str, asset_id: &str) -> [u8; 32] {
    Sha256::digest(format!("tmplr-oft:{route_id}:{asset_id}").as_bytes()).into()
}

/// Plans the wrap. `evm_nonce` is the reserved live deployer nonce the
/// caller bound (live adapter read or explicit operator reservation).
/// `require_evidence` is true when the evidence boundary is demanded
/// (mainnet semantics); the operator/issuer exclusion always holds.
pub fn plan_wrap(
    desired: &DesiredRouteV1,
    desired_sha256: &str,
    name: &str,
    symbol: &str,
    evm_nonce: u64,
    require_evidence: bool,
) -> Result<WrapPlanV1> {
    // Pre-dispatch USDC boundary: reject before touching anything else.
    let asset = desired.asset.clone().parse()?;
    if name.trim().is_empty() {
        return Err(Error::InvalidInput("wrap name must not be empty".into()));
    }
    if symbol.trim().is_empty() {
        return Err(Error::InvalidInput("wrap symbol must not be empty".into()));
    }
    let accounts = [
        desired.stellar_owner.as_str(),
        desired.stellar_delegate.as_str(),
        desired.evm_owner.as_str(),
        desired.evm_delegate.as_str(),
    ];
    enforce_issuer_exclusion(&asset, &accounts)?;
    if require_evidence {
        enforce_asset_evidence(&asset)?;
    }
    let token_contract = token_contract(&desired.identity.stellar_passphrase, &asset)?;
    let salt = stellar_salt(&desired.route_id, &asset.asset_id);
    let stellar_oft = crate::codec::derive_stellar_contract_address(
        &desired.identity.stellar_passphrase,
        &desired.stellar_owner,
        &salt,
    )?;
    let evm_deployer = crate::evm::parse_address(&desired.evm_owner)?;
    let evm_oft =
        crate::evm::canonical_address(crate::evm::derive_create_address(evm_deployer, evm_nonce));
    let stellar_peer_bytes = crate::codec::strkey_to_bytes32(&stellar_oft)?;
    let evm_peer_bytes = crate::codec::evm_address_to_bytes32(&evm_oft)?;
    let lock = crate::artifacts::embedded_lock()?;
    let wasm_sha256 = lock.stellar.oft_wasm_sha256.clone();
    let operations = vec![
        OperationV1::InstallStellarWasm {
            wasm_sha256: wasm_sha256.clone(),
        },
        OperationV1::DeployStellarOft {
            deployer: desired.stellar_owner.clone(),
            salt: hex::encode(salt),
            wasm_sha256: wasm_sha256.clone(),
            token: token_contract.clone(),
            shared_decimals: SHARED_DECIMALS,
            endpoint: desired.identity.stellar_endpoint.clone(),
            delegate: desired.stellar_delegate.clone(),
            expected_address: stellar_oft.clone(),
        },
        OperationV1::DeployEvmOft {
            deployer: desired.evm_owner.clone(),
            nonce: evm_nonce,
            creation_bytecode_keccak256: lock.evm.creation_bytecode_keccak256,
            name: name.to_string(),
            symbol: symbol.to_string(),
            endpoint: desired.identity.evm_endpoint.clone(),
            owner_delegate: desired.evm_delegate.clone(),
            expected_address: evm_oft.clone(),
        },
        OperationV1::SetStellarPeer {
            remote_eid: desired.identity.evm_eid,
            peer: format!("0x{}", hex::encode(evm_peer_bytes)),
        },
        OperationV1::SetEvmPeer {
            remote_eid: desired.identity.stellar_eid,
            peer: format!("0x{}", hex::encode(stellar_peer_bytes)),
        },
    ];
    Ok(WrapPlanV1 {
        schema_name: "wrap_plan".into(),
        schema_version: SCHEMA_VERSION,
        route_id: desired.route_id.clone(),
        desired_sha256: desired_sha256.to_string(),
        asset,
        name: name.to_string(),
        symbol: symbol.to_string(),
        stellar_token_contract: token_contract,
        stellar_salt: hex::encode(salt),
        stellar_deploy_account: desired.stellar_owner.clone(),
        stellar_oft,
        evm_deployer: desired.evm_owner.clone(),
        evm_nonce,
        evm_oft,
        operations,
    })
}
