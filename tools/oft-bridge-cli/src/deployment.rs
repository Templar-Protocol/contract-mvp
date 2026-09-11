//! Wrap deployment/adoption lifecycle: deterministic deployment proofs,
//! artifact-lock binding, exact runtime readback verification, fail-closed
//! adoption checks, and first-unsatisfied-node resume. Every execution path
//! is concrete — a mismatch is a typed hard conflict or custody failure,
//! never a silent marker or a fabricated value.
//!
//! # Adapter integration requirements (the three deployment nodes)
//!
//! This module plans and classifies [`OperationV1::InstallStellarWasm`],
//! [`OperationV1::DeployStellarOft`], and [`OperationV1::DeployEvmOft`]; the
//! exact transaction encoding belongs to the native adapters and is not
//! invented here:
//!
//! - `install_stellar_wasm`: one Stellar host `uploadContractWasm` operation
//!   broadcasting the pinned official OFT WASM; readback is the wasm code
//!   hash verifiable in network host-install state.
//! - `deploy_stellar_oft`: one Soroban transaction running `createContractV2`
//!   from the plan deploy account with the plan salt and pinned wasm, then
//!   the OFT `init` with the exact ordered arguments `(token_contract,
//!   shared_decimals=6, "LockUnlock", endpoint, delegate)`. The official
//!   initializer couples the initial owner and the endpoint delegate to the
//!   `delegate` argument. Readback settles the node only when the contract at
//!   the derived address carries the pinned wasm code hash, exposes
//!   owner/delegate equal to the planned owner-delegate, and exposes the
//!   planned lockbox token contract with shared decimals 6.
//! - `deploy_evm_oft`: one plain EVM `CREATE` from the concrete deployer EOA
//!   at the reserved live nonce with init code equal to the wrapped
//!   constructor `(name, symbol, endpoint, owner_delegate)`; the derived
//!   address is `keccak256(rlp([deployer, nonce]))[12..]`. Readback settles
//!   the node only when the runtime code at the derived address hashes to the
//!   pinned digest and owner/delegate/name/symbol/decimals/endpoint match.
//!
//! Callers gather [`DeploymentObservationsV1`] through the chain adapters,
//! evaluate [`deployment_node_plan`], then drive [`require_resumable`]:
//! execution resumes from the first unsatisfied node and never redeploys
//! over a differing address/code/owner/asset.

use crate::{
    domain::{
        ChainIdentityV1, DesiredRouteV1, OperationV1, RouteStateV1, SCHEMA_VERSION, SHARED_DECIMALS,
    },
    environment,
    error::{Error, Result},
    evm::{DeployEvmOftBindingV1, EvmChain},
    wrap::WrapPlanV1,
};

/// Deterministic Stellar OFT deployment: derivation inputs, the exact
/// derived `C...` contract address, and the artifact-lock WASM binding.
/// The address must equal the wrap plan exactly; the plan's install node
/// must carry the pinned WASM digest.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct StellarDeploymentV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub network_passphrase: String,
    pub deployer: String,
    /// Hex-encoded 32-byte `create_contract_v2` salt from the wrap plan.
    pub salt: String,
    /// Derived OFT contract address; verified equal to the plan.
    pub derived_address: String,
    /// Pinned Stellar OFT WASM digest the plan installs.
    pub wasm_sha256: String,
    pub artifact_lock_sha256: String,
}

/// Deterministic EVM OFT deployment: the `{deployer, nonce, derived
/// address, init code hash}` binding plus the wrapper constructor
/// arguments. `runtime_code_hash` stays `None` until an exact readback
/// verifies deployed runtime bytecode against the pinned digest.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct EvmDeploymentV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub chain_id: u64,
    pub deployer: String,
    pub nonce: u64,
    pub derived_address: String,
    /// keccak256 of the wrapper init code; `None` means the pinned artifact
    /// was never built and verification fails closed.
    pub init_code_hash: Option<String>,
    /// keccak256 of the deployed runtime bytecode; `None` until readback.
    pub runtime_code_hash: Option<String>,
    pub name: String,
    pub symbol: String,
    pub endpoint: String,
    pub owner_delegate: String,
    pub artifact_lock_sha256: String,
}

/// Route-wide deployment proof binding both peers to the artifact lock.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct DeploymentProofV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub desired_sha256: String,
    pub artifact_lock_sha256: String,
    pub stellar: StellarDeploymentV1,
    pub evm: EvmDeploymentV1,
}

/// Fail-closed verdict of an adoption check against recorded route state.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Eq, PartialEq)]
pub struct AdoptionVerdictV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub desired_sha256: String,
    pub environment: crate::domain::Environment,
    pub artifact_lock_sha256: String,
    pub stellar_oft: String,
    pub evm_oft: String,
    /// True only when route state already records both exact deployments;
    /// a rerun is then a no-op.
    pub already_satisfied: bool,
}

/// Reconstructs and verifies the Stellar deployment vector exactly: the
/// route-bound salt, the derived contract address, and the artifact-lock
/// WASM digest. Any drift is a hard conflict; the plan's install node must
/// match the pinned lock.
fn stellar_deployment(desired: &DesiredRouteV1, plan: &WrapPlanV1) -> Result<StellarDeploymentV1> {
    let salt = decode_salt(&plan.stellar_salt)?;
    if crate::wrap::stellar_salt(&plan.route_id, &plan.asset.asset_id) != salt {
        return Err(Error::Conflict(
            "wrap plan salt drifts from the route and asset pair".into(),
        ));
    }
    if plan.stellar_deploy_account != desired.stellar_owner {
        return Err(Error::Conflict(
            "wrap plan deploy account differs from the desired stellar owner".into(),
        ));
    }
    let derived = crate::codec::derive_stellar_contract_address(
        &desired.identity.stellar_passphrase,
        &plan.stellar_deploy_account,
        &salt,
    )?;
    if derived != plan.stellar_oft {
        return Err(Error::Conflict(format!(
            "derived stellar OFT {derived} differs from the wrap plan's {}",
            plan.stellar_oft
        )));
    }
    let deployment_operation = plan_deploy_stellar_operation(plan)?;
    let OperationV1::DeployStellarOft {
        deployer,
        salt: operation_salt,
        wasm_sha256: operation_wasm,
        token,
        shared_decimals,
        endpoint,
        delegate,
        expected_address,
    } = &deployment_operation
    else {
        unreachable!()
    };
    if deployer != &plan.stellar_deploy_account
        || operation_salt != &plan.stellar_salt
        || token != &plan.stellar_token_contract
        || *shared_decimals != SHARED_DECIMALS
        || endpoint != &desired.identity.stellar_endpoint
        || delegate != &desired.stellar_delegate
        || expected_address != &derived
    {
        return Err(Error::Conflict(
            "Stellar deployment operation drifts from the wrap plan constructor binding".into(),
        ));
    }
    let wasm_sha256 = install_wasm_sha256(plan)?;
    if wasm_sha256 != crate::artifacts::embedded_lock()?.stellar.oft_wasm_sha256 {
        return Err(Error::Custody(
            "wrap plan installs a stellar WASM that diverges from the artifact lock".into(),
        ));
    }
    if operation_wasm != &wasm_sha256 {
        return Err(Error::Conflict(
            "Stellar deployment operation wasm differs from the install node".into(),
        ));
    }
    Ok(StellarDeploymentV1 {
        schema_name: "stellar_deployment".into(),
        schema_version: SCHEMA_VERSION,
        route_id: plan.route_id.clone(),
        network_passphrase: desired.identity.stellar_passphrase.clone(),
        deployer: plan.stellar_deploy_account.clone(),
        salt: plan.stellar_salt.clone(),
        derived_address: derived,
        wasm_sha256,
        artifact_lock_sha256: crate::artifacts::lock_sha256()?,
    })
}

/// Reconstructs and verifies the EVM deployment vector: plain `CREATE`
/// derivation from the reserved deployer nonce, the wrapped constructor
/// arguments, and the pinned init code hash. The init code hash stays
/// `None` until the wrapper artifact is built and verification fails
/// closed on `None` — a hash is never fabricated.
fn evm_deployment(
    desired: &DesiredRouteV1,
    plan: &WrapPlanV1,
    binding: &DeployEvmOftBindingV1,
) -> Result<EvmDeploymentV1> {
    let bound_deployer = crate::evm::parse_address(&binding.deployer)?;
    let plan_deployer = crate::evm::parse_address(&plan.evm_deployer)?;
    let desired_owner = crate::evm::parse_address(&desired.evm_owner)?;
    if bound_deployer != plan_deployer || bound_deployer != desired_owner {
        return Err(Error::Conflict(
            "evm deployment binding deployer differs from the wrap plan or desired owner".into(),
        ));
    }
    if binding.nonce != plan.evm_nonce {
        return Err(Error::Conflict(
            "evm deployment binding nonce differs from the wrap plan".into(),
        ));
    }
    let (node_deployer, node_nonce) = plan_evm_node(plan)?;
    if node_nonce != plan.evm_nonce || crate::evm::parse_address(node_deployer)? != plan_deployer {
        return Err(Error::Conflict(
            "wrap plan deploy node drifts from the plan vector".into(),
        ));
    }
    let derived = crate::evm::derive_create_address(bound_deployer, binding.nonce);
    if crate::evm::parse_address(&binding.derived_address)? != derived {
        return Err(Error::Conflict(
            "evm binding derived address does not match its own create derivation".into(),
        ));
    }
    if crate::evm::parse_address(&plan.evm_oft)? != derived {
        return Err(Error::Conflict(format!(
            "derived evm OFT {} differs from the wrap plan's {}",
            crate::evm::canonical_address(derived),
            plan.evm_oft
        )));
    }
    let init_code_hash = match binding.init_code_hash.clone() {
        None => {
            return Err(Error::Custody(
                "evm wrapper init code hash is not bound; artifact build must precede deployment verification"
                    .into(),
            ))
        }
        Some(hash) => hash,
    };
    if !init_code_hash
        .trim_start_matches("0x")
        .eq_ignore_ascii_case(
            crate::artifacts::embedded_lock()?
                .evm
                .creation_bytecode_keccak256
                .trim_start_matches("0x"),
        )
    {
        return Err(Error::Custody(
            "evm wrapper init code hash diverges from the artifact lock".into(),
        ));
    }
    let deployment_operation = plan_deploy_evm_operation(plan)?;
    let OperationV1::DeployEvmOft {
        deployer,
        nonce,
        creation_bytecode_keccak256,
        name,
        symbol,
        endpoint,
        owner_delegate,
        expected_address,
    } = &deployment_operation
    else {
        unreachable!()
    };
    if crate::evm::parse_address(deployer)? != bound_deployer
        || *nonce != binding.nonce
        || !creation_bytecode_keccak256
            .trim_start_matches("0x")
            .eq_ignore_ascii_case(init_code_hash.trim_start_matches("0x"))
        || name != &binding.name
        || symbol != &binding.symbol
        || !endpoint.eq_ignore_ascii_case(&binding.endpoint)
        || crate::evm::parse_address(owner_delegate)?
            != crate::evm::parse_address(&binding.owner_delegate)?
        || crate::evm::parse_address(expected_address)? != derived
    {
        return Err(Error::Conflict(
            "EVM deployment operation drifts from its nonce, artifact, or constructor binding"
                .into(),
        ));
    }
    if binding.name != plan.name {
        return Err(Error::Conflict(
            "evm binding name differs from the wrap plan".into(),
        ));
    }
    if binding.symbol != plan.symbol {
        return Err(Error::Conflict(
            "evm binding symbol differs from the wrap plan".into(),
        ));
    }
    if !binding
        .endpoint
        .eq_ignore_ascii_case(&desired.identity.evm_endpoint)
    {
        return Err(Error::Conflict(
            "evm binding endpoint differs from the desired route identity".into(),
        ));
    }
    if crate::evm::parse_address(&binding.owner_delegate)?
        != crate::evm::parse_address(&desired.evm_delegate)?
    {
        return Err(Error::Conflict(
            "evm binding owner delegate differs from the desired route identity".into(),
        ));
    }
    Ok(EvmDeploymentV1 {
        schema_name: "evm_deployment".into(),
        schema_version: SCHEMA_VERSION,
        route_id: plan.route_id.clone(),
        chain_id: desired.identity.evm_chain_id,
        deployer: binding.deployer.clone(),
        nonce: binding.nonce,
        derived_address: binding.derived_address.clone(),
        init_code_hash: Some(init_code_hash.trim_start_matches("0x").to_ascii_lowercase()),
        runtime_code_hash: None,
        name: binding.name.clone(),
        symbol: binding.symbol.clone(),
        endpoint: binding.endpoint.clone(),
        owner_delegate: binding.owner_delegate.clone(),
        artifact_lock_sha256: crate::artifacts::lock_sha256()?,
    })
}

/// Binds the deterministic route-wide deployment proof. The plan must bind
/// the exact desired digest and route; both peers are re-derived and
/// verified against the plan and the pinned artifact lock.
pub fn deployment_proof(
    desired: &DesiredRouteV1,
    desired_sha256: &str,
    plan: &WrapPlanV1,
    binding: &DeployEvmOftBindingV1,
) -> Result<DeploymentProofV1> {
    if plan.route_id != desired.route_id {
        return Err(Error::Conflict(
            "wrap plan route differs from the desired route".into(),
        ));
    }
    if plan.desired_sha256 != desired_sha256 {
        return Err(Error::Conflict(
            "wrap plan binds a different desired digest".into(),
        ));
    }
    let stellar = stellar_deployment(desired, plan)?;
    let evm = evm_deployment(desired, plan, binding)?;
    Ok(DeploymentProofV1 {
        schema_name: "deployment_proof".into(),
        schema_version: SCHEMA_VERSION,
        route_id: plan.route_id.clone(),
        desired_sha256: desired_sha256.to_string(),
        artifact_lock_sha256: crate::artifacts::lock_sha256()?,
        stellar,
        evm,
    })
}

/// Requires the observed deployed runtime code to hash exactly to the
/// pinned digest. Empty code (an EOA or missing contract at the derived
/// address) is a chain refusal, never an invented hash.
pub fn verify_runtime_code_hash(
    observed_code: &[u8],
    expected_runtime_hash: &str,
) -> Result<String> {
    if observed_code.is_empty() {
        return Err(Error::Chain(
            "deployment readback: deployed account has no code".into(),
        ));
    }
    let hash = hex::encode(crate::evm::keccak256_of(observed_code));
    if hash != expected_runtime_hash {
        return Err(Error::Custody(format!(
            "deployment readback: runtime code hash {hash} diverges from the pinned artifact digest"
        )));
    }
    Ok(hash)
}

/// Reads the deployed runtime code through the chain adapter and binds the
/// verified hash into the deployment. The expected digest is the
/// artifact-lock runtime bytecode hash; divergence or missing code fails
/// closed and leaves `runtime_code_hash` unset.
pub fn apply_runtime_readback(
    chain: &dyn EvmChain,
    deployment: &mut EvmDeploymentV1,
    expected_runtime_hash: &str,
) -> Result<String> {
    let address = crate::evm::parse_address(&deployment.derived_address)?;
    let code = crate::block_on_result(chain.code(address))?;
    let hash = verify_runtime_code_hash(&code, expected_runtime_hash)?;
    deployment.runtime_code_hash = Some(hash.clone());
    Ok(hash)
}

/// Fail-closed adoption check. Adoption is a local state binding and is
/// available on recognized environments; it requires exact runtime readback from
/// [`apply_runtime_readback`]; recorded route state must match the proof
/// exactly or the adoption is a hard conflict, never a silent overwrite.
/// When route state already records both exact deployments the verdict is
/// `already_satisfied` and a rerun is a no-op.
pub fn adoption_verdict(
    identity: &ChainIdentityV1,
    plan: &WrapPlanV1,
    proof: &DeploymentProofV1,
    state: &RouteStateV1,
) -> Result<AdoptionVerdictV1> {
    environment::classify(identity)?;
    if plan.route_id != state.route_id {
        return Err(Error::Conflict(
            "wrap plan route differs from the route state".into(),
        ));
    }
    if plan.desired_sha256 != state.desired_sha256 {
        return Err(Error::Conflict(
            "route state binds a different desired digest than the wrap plan".into(),
        ));
    }
    if proof.evm.runtime_code_hash.is_none() {
        return Err(Error::Custody(
            "adoption requires exact deployment readback; evm runtime code hash is unverified"
                .into(),
        ));
    }
    if let Some(opening) = &state.opening_custody {
        if opening.artifact_lock_sha256 != proof.artifact_lock_sha256 {
            return Err(Error::Custody(
                "adoption artifact lock differs from opening custody".into(),
            ));
        }
    }
    let recorded_stellar = state.contracts.get("stellar_oft");
    let recorded_evm = state.contracts.get("evm_oft");
    if let Some(recorded) = recorded_stellar {
        if *recorded != plan.stellar_oft {
            return Err(Error::Conflict(format!(
                "route state records stellar OFT {recorded} but the plan derives {}",
                plan.stellar_oft
            )));
        }
    }
    if let Some(recorded) = recorded_evm {
        if !recorded.eq_ignore_ascii_case(&proof.evm.derived_address) {
            return Err(Error::Conflict(format!(
                "route state records evm OFT {recorded} but the plan derives {}",
                proof.evm.derived_address
            )));
        }
    }
    Ok(AdoptionVerdictV1 {
        schema_name: "adoption_verdict".into(),
        schema_version: SCHEMA_VERSION,
        route_id: plan.route_id.clone(),
        desired_sha256: plan.desired_sha256.clone(),
        environment: identity.environment,
        artifact_lock_sha256: proof.artifact_lock_sha256.clone(),
        stellar_oft: plan.stellar_oft.clone(),
        evm_oft: proof.evm.derived_address.clone(),
        already_satisfied: recorded_stellar.is_some() && recorded_evm.is_some(),
    })
}

/// The `install_stellar_wasm` node's pinned WASM digest.
fn install_wasm_sha256(plan: &WrapPlanV1) -> Result<String> {
    plan.operations
        .iter()
        .find_map(|operation| match operation {
            OperationV1::InstallStellarWasm { wasm_sha256 } => Some(wasm_sha256.clone()),
            _ => None,
        })
        .ok_or_else(|| Error::Conflict("wrap plan lacks the install_stellar_wasm node".into()))
}

/// The `deploy_evm_oft` node's deployer and reserved nonce.
fn plan_evm_node(plan: &WrapPlanV1) -> Result<(&str, u64)> {
    plan.operations
        .iter()
        .find_map(|operation| match operation {
            OperationV1::DeployEvmOft {
                deployer, nonce, ..
            } => Some((deployer.as_str(), *nonce)),
            _ => None,
        })
        .ok_or_else(|| Error::Conflict("wrap plan lacks the deploy_evm_oft node".into()))
}

fn decode_salt(hex_salt: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(hex_salt)
        .map_err(|error| Error::InvalidInput(format!("wrap plan salt is not hex: {error}")))?;
    bytes
        .try_into()
        .map_err(|_| Error::InvalidInput("wrap plan salt must be 32 bytes".into()))
}

// ---------------------------------------------------------------------------
// Resumable deployment node plan (frozen-plan Step 6)
// ---------------------------------------------------------------------------

/// Stellar deployment-chain observations the native adapter produces for one
/// resume evaluation. Every field is adapter-derived; `None` means the fact
/// could not be observed (typically an absent deployment). A deployment
/// observation is all-or-nothing: when code is present every bound fact must
/// also be read back, and a missing fact is a hard conflict, never a silent
/// assumption.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StellarDeploymentObservationsV1 {
    /// Wasm code hash the adapter observed installed in network host-install
    /// state; the pinned digest when the pinned wasm is confirmed present.
    pub installed_wasm_sha256: Option<String>,
    /// Contract address the adapter actually read; must equal the derived OFT
    /// address or the readback is a hard conflict.
    pub oft_address: Option<String>,
    /// Code hash of the contract observed at the read address; `None` means
    /// no deployment is present.
    pub oft_code_sha256: Option<String>,
    pub owner: Option<String>,
    pub delegate: Option<String>,
    /// Lockbox token contract the deployed OFT is bound to.
    pub token_contract: Option<String>,
    pub decimals: Option<u8>,
}

/// EVM deployment-chain observations the native adapter produces for one
/// resume evaluation. `runtime_code_sha256` `None` means no deployment is
/// present at the derived address.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EvmDeploymentObservationsV1 {
    /// Live pending nonce of the deployer account. When the deployment is
    /// absent this must equal the plan's reserved nonce; any divergence means
    /// unrelated nonce consumption invalidated the plan before signing.
    pub deployer_nonce: u64,
    /// Contract address the adapter actually read; must equal the derived OFT
    /// address or the readback is a hard conflict.
    pub oft_address: Option<String>,
    /// Runtime code hash of the contract observed at the read address; `None`
    /// means no deployment is present.
    pub runtime_code_sha256: Option<String>,
    pub owner: Option<String>,
    pub delegate: Option<String>,
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub decimals: Option<u8>,
    pub endpoint: Option<String>,
}

/// Combined deployment-chain observations for one resume evaluation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeploymentObservationsV1 {
    pub stellar: StellarDeploymentObservationsV1,
    pub evm: EvmDeploymentObservationsV1,
}

fn stellar_address_value(value: stellar_baselib::xdr::ScVal) -> Result<String> {
    use stellar_baselib::{
        address::{Address, AddressTrait as _},
        xdr::ScVal,
    };
    let ScVal::Address(address) = value else {
        return Err(Error::Chain(
            "Stellar view returned a non-address value".into(),
        ));
    };
    Address::from_sc_address(&address)
        .map(|address| address.to_string())
        .map_err(|error| Error::Chain(format!("invalid Stellar view address: {error}")))
}

fn evm_selector(signature: &str) -> Vec<u8> {
    crate::evm::keccak256_of(signature.as_bytes())[..4].to_vec()
}

fn evm_address_word(address: &str) -> Result<Vec<u8>> {
    let address = crate::evm::parse_address(address)?;
    let mut word = vec![0u8; 32];
    word[12..].copy_from_slice(address.as_slice());
    Ok(word)
}

fn decode_evm_address(value: &[u8], label: &str) -> Result<String> {
    if value.len() < 32 {
        return Err(Error::Chain(format!("{label} returned a short EVM word")));
    }
    Ok(crate::evm::canonical_address(
        alloy::primitives::Address::from_slice(&value[value.len() - 20..]),
    ))
}

fn decode_evm_u8(value: &[u8], label: &str) -> Result<u8> {
    if value.len() != 32 || value[..31].iter().any(|byte| *byte != 0) {
        return Err(Error::Chain(format!("{label} returned a non-u8 EVM word")));
    }
    Ok(value[31])
}

fn decode_evm_string(value: &[u8], label: &str) -> Result<String> {
    if value.len() < 64 {
        return Err(Error::Chain(format!(
            "{label} returned malformed dynamic data"
        )));
    }
    let offset = usize::from_be_bytes(
        value[24..32]
            .try_into()
            .map_err(|_| Error::Chain(format!("{label} offset is malformed")))?,
    );
    if offset.checked_add(32).is_none_or(|end| end > value.len()) {
        return Err(Error::Chain(format!("{label} offset is out of bounds")));
    }
    let length = usize::from_be_bytes(
        value[offset + 24..offset + 32]
            .try_into()
            .map_err(|_| Error::Chain(format!("{label} length is malformed")))?,
    );
    let start = offset + 32;
    let end = start
        .checked_add(length)
        .filter(|end| *end <= value.len())
        .ok_or_else(|| Error::Chain(format!("{label} value is out of bounds")))?;
    String::from_utf8(value[start..end].to_vec())
        .map_err(|_| Error::Chain(format!("{label} is not UTF-8")))
}

/// Reads every deployment fact required by the resumable node classifier.
pub fn observe_deployments(
    stellar: &dyn crate::stellar::StellarChain,
    evm: &dyn EvmChain,
    desired: &DesiredRouteV1,
    plan: &WrapPlanV1,
) -> Result<DeploymentObservationsV1> {
    use stellar_baselib::xdr::{Limits, ScVal, WriteXdr as _};

    let stellar_code = stellar.contract_code_hash_optional(&plan.stellar_oft)?;
    let (stellar_owner, stellar_delegate, token_contract, stellar_decimals) = if stellar_code
        .is_some()
    {
        let source = &plan.stellar_deploy_account;
        let owner =
            stellar_address_value(stellar.invoke_view(&plan.stellar_oft, "owner", &[], source)?)?;
        let token =
            stellar_address_value(stellar.invoke_view(&plan.stellar_oft, "token", &[], source)?)?;
        let decimals =
            match stellar.invoke_view(&plan.stellar_oft, "shared_decimals", &[], source)? {
                ScVal::U32(value) => u8::try_from(value)
                    .map_err(|_| Error::Chain("Stellar shared decimals exceed u8".into()))?,
                _ => {
                    return Err(Error::Chain(
                        "Stellar shared_decimals returned a non-u32 value".into(),
                    ))
                }
            };
        let oapp_arg = crate::layerzero::stellar_address(&plan.stellar_oft)?
            .to_xdr(Limits::none())
            .map(hex::encode)
            .map_err(|error| {
                Error::Chain(format!("Stellar delegate arg encoding failed: {error}"))
            })?;
        let delegate = stellar_address_value(stellar.invoke_view(
            &desired.identity.stellar_endpoint,
            "delegate",
            &[oapp_arg],
            source,
        )?)?;
        (Some(owner), Some(delegate), Some(token), Some(decimals))
    } else {
        (None, None, None, None)
    };

    let evm_address = crate::evm::parse_address(&plan.evm_oft)?;
    let evm_code = crate::block_on_result(evm.code(evm_address))?;
    let (evm_runtime, evm_owner, evm_delegate, evm_name, evm_symbol, evm_decimals, evm_endpoint) =
        if evm_code.is_empty() {
            (None, None, None, None, None, None, None)
        } else {
            let call = |to, signature: &str, mut calldata: Vec<u8>| {
                let mut input = evm_selector(signature);
                input.append(&mut calldata);
                crate::block_on_result(evm.call(to, input))
            };
            let owner = decode_evm_address(&call(evm_address, "owner()", vec![])?, "owner")?;
            let endpoint =
                decode_evm_address(&call(evm_address, "endpoint()", vec![])?, "endpoint")?;
            let delegate = decode_evm_address(
                &call(
                    crate::evm::parse_address(&desired.identity.evm_endpoint)?,
                    "delegates(address)",
                    evm_address_word(&plan.evm_oft)?,
                )?,
                "delegate",
            )?;
            let name = decode_evm_string(&call(evm_address, "name()", vec![])?, "name")?;
            let symbol = decode_evm_string(&call(evm_address, "symbol()", vec![])?, "symbol")?;
            let decimals = decode_evm_u8(&call(evm_address, "decimals()", vec![])?, "decimals")?;
            (
                Some(hex::encode(crate::evm::keccak256_of(&evm_code))),
                Some(owner),
                Some(delegate),
                Some(name),
                Some(symbol),
                Some(decimals),
                Some(endpoint),
            )
        };
    Ok(DeploymentObservationsV1 {
        stellar: StellarDeploymentObservationsV1 {
            installed_wasm_sha256: stellar
                .wasm_installed(&crate::artifacts::embedded_lock()?.stellar.oft_wasm_sha256)?
                .then(|| crate::artifacts::embedded_lock().map(|lock| lock.stellar.oft_wasm_sha256))
                .transpose()?,
            oft_address: stellar_code.as_ref().map(|_| plan.stellar_oft.clone()),
            oft_code_sha256: stellar_code,
            owner: stellar_owner,
            delegate: stellar_delegate,
            token_contract,
            decimals: stellar_decimals,
        },
        evm: EvmDeploymentObservationsV1 {
            deployer_nonce: crate::block_on_result(
                evm.account_nonce(crate::evm::parse_address(&plan.evm_deployer)?),
            )?,
            oft_address: evm_runtime.as_ref().map(|_| plan.evm_oft.clone()),
            runtime_code_sha256: evm_runtime,
            owner: evm_owner,
            delegate: evm_delegate,
            name: evm_name,
            symbol: evm_symbol,
            decimals: evm_decimals,
            endpoint: evm_endpoint,
        },
    })
}

/// Status of one deployment node. Only exact readback is `Satisfied`;
/// anything else keeps the route unadopted. A `Conflicting` node is a hard
/// conflict: the plan never redeploys over it.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentNodeStatus {
    /// Chain readback exactly matches the node's binding; nothing to run.
    Satisfied,
    /// No admissible deployment evidence exists (target absent); the node
    /// must run.
    Pending,
    /// Evidence exists at the target but differs from the binding; a typed
    /// hard conflict, never an overwrite.
    Conflicting,
}

/// One deployment node of the wrap plan with its exact readback status.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeploymentNodeV1 {
    /// Stable node kind (`install_stellar_wasm`, `deploy_stellar_oft`,
    /// `deploy_evm_oft`).
    pub kind: String,
    /// The exact closed-enum operation from the wrap plan.
    pub operation: OperationV1,
    pub status: DeploymentNodeStatus,
    /// Typed conflict reason when `status` is `Conflicting`.
    #[serde(default)]
    pub conflict: Option<String>,
}

/// Complete resumable deployment node plan. The embedded [`DeploymentProofV1`]
/// re-verifies every deterministic vector (Stellar salt/address/wasm digest,
/// EVM deployer/nonce/derived address/init code hash, constructor arguments,
/// artifact-lock binding) before any node is classified.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeploymentNodePlanV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub route_id: String,
    pub desired_sha256: String,
    pub artifact_lock_sha256: String,
    pub stellar_oft: String,
    pub evm_oft: String,
    /// Deployment nodes in plan order: install wasm, deploy stellar OFT,
    /// deploy EVM OFT.
    pub nodes: Vec<DeploymentNodeV1>,
    /// Index of the first node that must run; `None` when no node is pending.
    pub first_unsatisfied: Option<usize>,
    /// Every typed conflict reason; non-empty iff a node is `Conflicting`.
    pub conflicts: Vec<String>,
    /// True only when every node is exactly satisfied (a rerun is an
    /// idempotent adoption no-op).
    pub converged: bool,
}

/// Compares digest strings case-insensitively, tolerating an optional `0x`
/// prefix so adapter output never trips on rendering.
fn hashes_equal(left: &str, right: &str) -> bool {
    let normalize = |value: &str| value.trim_start_matches("0x").to_ascii_lowercase();
    normalize(left) == normalize(right)
}

/// The wrap plan's own `install_stellar_wasm` operation; absent nodes are a
/// hard conflict.
fn plan_install_operation(plan: &WrapPlanV1) -> Result<OperationV1> {
    plan.operations
        .iter()
        .find_map(|operation| match operation {
            OperationV1::InstallStellarWasm { .. } => Some(operation.clone()),
            _ => None,
        })
        .ok_or_else(|| Error::Conflict("wrap plan lacks the install_stellar_wasm node".into()))
}

/// The wrap plan's own `deploy_stellar_oft` operation; absent nodes are a
/// hard conflict.
fn plan_deploy_stellar_operation(plan: &WrapPlanV1) -> Result<OperationV1> {
    plan.operations
        .iter()
        .find_map(|operation| match operation {
            OperationV1::DeployStellarOft { .. } => Some(operation.clone()),
            _ => None,
        })
        .ok_or_else(|| Error::Conflict("wrap plan lacks the deploy_stellar_oft node".into()))
}

/// The wrap plan's own `deploy_evm_oft` operation; absent nodes are a hard
/// conflict.
fn plan_deploy_evm_operation(plan: &WrapPlanV1) -> Result<OperationV1> {
    plan.operations
        .iter()
        .find_map(|operation| match operation {
            OperationV1::DeployEvmOft { .. } => Some(operation.clone()),
            _ => None,
        })
        .ok_or_else(|| Error::Conflict("wrap plan lacks the deploy_evm_oft node".into()))
}

fn node_install_wasm(
    plan: &WrapPlanV1,
    proof: &DeploymentProofV1,
    observed: &StellarDeploymentObservationsV1,
) -> Result<DeploymentNodeV1> {
    let operation = plan_install_operation(plan)?;
    let expected = &proof.stellar.wasm_sha256;
    let (status, conflict) = match &observed.installed_wasm_sha256 {
        Some(hash) if hashes_equal(hash, expected) => (DeploymentNodeStatus::Satisfied, None),
        Some(hash) => (
            DeploymentNodeStatus::Conflicting,
            Some(format!(
                "installed stellar wasm hash {hash} differs from the pinned digest {expected}"
            )),
        ),
        None => {
            // The install is provable through the deployed instance: a live
            // OFT carrying the pinned code implies the wasm is installed.
            if observed
                .oft_code_sha256
                .as_deref()
                .is_some_and(|hash| hashes_equal(hash, expected))
            {
                (DeploymentNodeStatus::Satisfied, None)
            } else {
                (DeploymentNodeStatus::Pending, None)
            }
        }
    };
    Ok(DeploymentNodeV1 {
        kind: crate::stellar::operation_label(&operation),
        operation,
        status,
        conflict,
    })
}

fn node_deploy_stellar_oft(
    desired: &DesiredRouteV1,
    plan: &WrapPlanV1,
    proof: &DeploymentProofV1,
    observed: &StellarDeploymentObservationsV1,
) -> Result<DeploymentNodeV1> {
    let operation = plan_deploy_stellar_operation(plan)?;
    let derived = &proof.stellar.derived_address;
    let expected_wasm = &proof.stellar.wasm_sha256;
    let owner_delegate = &desired.stellar_delegate;
    let token = &plan.stellar_token_contract;
    let (status, conflict) = match &observed.oft_code_sha256 {
        None => (DeploymentNodeStatus::Pending, None),
        Some(code_hash) => {
            let mut failures = Vec::new();
            if observed.oft_address.as_deref() != Some(derived.as_str()) {
                failures.push(format!(
                    "readback address {:?} differs from the derived OFT {derived}",
                    observed.oft_address
                ));
            }
            if !hashes_equal(code_hash, expected_wasm) {
                failures.push(format!(
                    "deployed code hash {code_hash} differs from the pinned wasm digest {expected_wasm}"
                ));
            }
            if observed.owner.as_deref() != Some(owner_delegate.as_str())
                || observed.delegate.as_deref() != Some(owner_delegate.as_str())
            {
                failures.push(format!(
                    "owner/delegate readback ({:?}/{:?}) must both equal the planned owner-delegate {owner_delegate}",
                    observed.owner, observed.delegate
                ));
            }
            if observed.token_contract.as_deref() != Some(token.as_str()) {
                failures.push(format!(
                    "deployed lockbox token {:?} differs from the planned asset contract {token}",
                    observed.token_contract
                ));
            }
            if observed.decimals != Some(SHARED_DECIMALS) {
                failures.push(format!(
                    "deployed shared decimals must be {SHARED_DECIMALS}"
                ));
            }
            match failures.is_empty() {
                true => (DeploymentNodeStatus::Satisfied, None),
                false => (DeploymentNodeStatus::Conflicting, Some(failures.join("; "))),
            }
        }
    };
    Ok(DeploymentNodeV1 {
        kind: crate::stellar::operation_label(&operation),
        operation,
        status,
        conflict,
    })
}

fn node_deploy_evm_oft(
    desired: &DesiredRouteV1,
    plan: &WrapPlanV1,
    proof: &DeploymentProofV1,
    observed: &EvmDeploymentObservationsV1,
) -> Result<DeploymentNodeV1> {
    let operation = plan_deploy_evm_operation(plan)?;
    let derived = &proof.evm.derived_address;
    let expected_runtime = &crate::artifacts::embedded_lock()?
        .evm
        .runtime_bytecode_keccak256;
    let owner_delegate = crate::evm::parse_address(&desired.evm_delegate)?;
    let endpoint = crate::evm::parse_address(&desired.identity.evm_endpoint)?;
    let (status, conflict) = match &observed.runtime_code_sha256 {
        None => {
            if observed.deployer_nonce != proof.evm.nonce {
                (
                    DeploymentNodeStatus::Conflicting,
                    Some(format!(
                        "live deployer nonce {} no longer equals the reserved nonce {}; unrelated nonce consumption invalidated the plan before signing",
                        observed.deployer_nonce, proof.evm.nonce
                    )),
                )
            } else {
                (DeploymentNodeStatus::Pending, None)
            }
        }
        Some(code_hash) => {
            let mut failures = Vec::new();
            let observed_address = observed
                .oft_address
                .as_deref()
                .and_then(|value| crate::evm::parse_address(value).ok());
            if observed_address != Some(crate::evm::parse_address(derived)?) {
                failures.push(format!(
                    "readback address {:?} differs from the derived OFT {derived}",
                    observed.oft_address
                ));
            }
            if !hashes_equal(code_hash, expected_runtime) {
                failures.push(format!(
                    "deployed runtime code hash {code_hash} differs from the pinned digest {expected_runtime}"
                ));
            }
            let observed_owner = observed
                .owner
                .as_deref()
                .and_then(|value| crate::evm::parse_address(value).ok());
            let observed_delegate = observed
                .delegate
                .as_deref()
                .and_then(|value| crate::evm::parse_address(value).ok());
            if observed_owner != Some(owner_delegate) || observed_delegate != Some(owner_delegate) {
                failures.push(format!(
                    "owner/delegate readback ({:?}/{:?}) must both equal the planned owner-delegate {}",
                    observed.owner,
                    observed.delegate,
                    crate::evm::canonical_address(owner_delegate)
                ));
            }
            if observed.name.as_deref() != Some(plan.name.as_str()) {
                failures.push(format!(
                    "deployed token name {:?} differs from the planned wrapper name {}",
                    observed.name, plan.name
                ));
            }
            if observed.symbol.as_deref() != Some(plan.symbol.as_str()) {
                failures.push(format!(
                    "deployed token symbol {:?} differs from the planned wrapper symbol {}",
                    observed.symbol, plan.symbol
                ));
            }
            if observed.decimals != Some(SHARED_DECIMALS) {
                failures.push(format!(
                    "deployed shared decimals must be {SHARED_DECIMALS}"
                ));
            }
            let observed_endpoint = observed
                .endpoint
                .as_deref()
                .and_then(|value| crate::evm::parse_address(value).ok());
            if observed_endpoint != Some(endpoint) {
                failures.push(format!(
                    "deployed endpoint {:?} differs from the route identity endpoint {}",
                    observed.endpoint,
                    crate::evm::canonical_address(endpoint)
                ));
            }
            match failures.is_empty() {
                true => (DeploymentNodeStatus::Satisfied, None),
                false => (DeploymentNodeStatus::Conflicting, Some(failures.join("; "))),
            }
        }
    };
    Ok(DeploymentNodeV1 {
        kind: crate::stellar::operation_label(&operation),
        operation,
        status,
        conflict,
    })
}

/// Evaluates the wrap plan's deployment nodes against chain observations and
/// builds the complete resumable node plan. The proof is rebuilt first, so
/// every deterministic vector (Stellar salt/address/wasm digest, EVM
/// deployer/nonce/derived address/init code hash, constructor arguments,
/// artifact-lock binding) is re-verified exactly before any node is
/// classified. Callers pass [`require_resumable`] the result; only nodes from
/// the returned first-unsatisfied index onward are executed, and a
/// `Conflicting` node refuses the resume before anything runs.
pub fn deployment_node_plan(
    desired: &DesiredRouteV1,
    desired_sha256: &str,
    plan: &WrapPlanV1,
    binding: &DeployEvmOftBindingV1,
    observations: &DeploymentObservationsV1,
) -> Result<DeploymentNodePlanV1> {
    let proof = deployment_proof(desired, desired_sha256, plan, binding)?;
    let nodes = vec![
        node_install_wasm(plan, &proof, &observations.stellar)?,
        node_deploy_stellar_oft(desired, plan, &proof, &observations.stellar)?,
        node_deploy_evm_oft(desired, plan, &proof, &observations.evm)?,
    ];
    let first_unsatisfied = nodes
        .iter()
        .position(|node| node.status == DeploymentNodeStatus::Pending);
    let conflicts: Vec<String> = nodes
        .iter()
        .filter_map(|node| node.conflict.clone())
        .collect();
    let converged = conflicts.is_empty() && first_unsatisfied.is_none();
    Ok(DeploymentNodePlanV1 {
        schema_name: "deployment_node_plan".into(),
        schema_version: SCHEMA_VERSION,
        route_id: proof.route_id.clone(),
        desired_sha256: proof.desired_sha256.clone(),
        artifact_lock_sha256: proof.artifact_lock_sha256.clone(),
        stellar_oft: proof.stellar.derived_address.clone(),
        evm_oft: proof.evm.derived_address.clone(),
        nodes,
        first_unsatisfied,
        conflicts,
        converged,
    })
}

/// Resume driver for the deployment node plan. Refuses with a typed conflict
/// when any node is `Conflicting` — a differing code/owner/asset/address at a
/// derived target is never redeployed over. Otherwise returns the index of
/// the first unsatisfied node to resume from; `None` means every deployment
/// node is already satisfied and a rerun is an exact adoption no-op.
pub fn require_resumable(plan: &DeploymentNodePlanV1) -> Result<Option<usize>> {
    if let Some((index, node)) = plan
        .nodes
        .iter()
        .enumerate()
        .find(|(_, node)| node.status == DeploymentNodeStatus::Conflicting)
    {
        return Err(Error::Conflict(format!(
            "deployment node {} (node {index}) is conflicted: {}",
            node.kind,
            node.conflict.as_deref().unwrap_or("readback is unverified")
        )));
    }
    Ok(plan.first_unsatisfied)
}
