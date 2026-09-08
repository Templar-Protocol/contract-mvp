//! Governance boundaries: deterministic proposal and external-signature
//! paths, executed-transaction readback, and the recovery capability matrix.
//! Mainnet hot-key execution remains disabled; reviewed proposals may be
//! signed externally and ingested after exact finalized-transaction readback.

use std::collections::BTreeMap;

use alloy::sol_types::SolCall as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

alloy::sol! {
    interface Safe {
        function getTransactionHash(
            address to,
            uint256 value,
            bytes data,
            uint8 operation,
            uint256 safeTxGas,
            uint256 baseGas,
            uint256 gasPrice,
            address gasToken,
            address refundReceiver,
            uint256 nonce
        ) external view returns (bytes32);
        function isOwner(address owner) external view returns (bool);
    }
}

use crate::domain::{Environment, ExecutablePlanV1, OperationV1, ProposalV1, Vm, SCHEMA_VERSION};
use crate::error::{Error, Result};

/// Canonical policy message for every mainnet mutation path.
pub const PRODUCTION_MUTATION_UNSUPPORTED_V1: &str = "production_mutation_unsupported_v1";

fn require_plan_environment(environment: Environment, plan: &ExecutablePlanV1) -> Result<()> {
    let (stellar_passphrase, evm_chain_id) = match environment {
        Environment::StellarTestnetSepolia => (
            crate::environment::STELLAR_TESTNET_PASSPHRASE,
            11_155_111u64,
        ),
        Environment::StellarMainnetEthereum => {
            (crate::environment::STELLAR_PUBLIC_PASSPHRASE, 1u64)
        }
    };
    if let Some(binding) = &plan.stellar {
        if binding.network_passphrase != stellar_passphrase {
            return Err(Error::Policy(
                "proposal network passphrase differs from the requested environment".into(),
            ));
        }
    }
    if let Some(binding) = &plan.evm {
        if binding.chain_id != evm_chain_id.to_string() {
            return Err(Error::Policy(
                "proposal chain id differs from the requested environment".into(),
            ));
        }
    }
    Ok(())
}

/// Typed verification data returned for a signed proposal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignatureVerificationDataV1 {
    pub route_id: String,
    pub vm: Vm,
    pub sender: String,
    /// Decimal string sequence (Stellar) or nonce (EVM).
    pub sequence_or_nonce: String,
    pub unsigned_payload: String,
    pub unsigned_payload_sha256: String,
    pub plan_sha256: String,
    pub expires_at_unix: u64,
    pub attached_weight: u32,
    pub required_weight: u32,
    pub threshold_satisfied: bool,
}

/// Outcome of transaction preparation when a qualified simulation demands
/// footprint restoration before the original operation can proceed.
#[derive(Debug)]
pub enum PrepareOutcome {
    Ready(ExecutablePlanV1),
    RestorationRequired(OperationV1),
}

/// Derives the journaled footprint-restore sub-operation for an original
/// operation. The original plan must be rebuilt afterwards because
/// restoration consumes a sequence.
pub fn restoration_required(original: &OperationV1) -> Result<OperationV1> {
    Ok(OperationV1::RestoreFootprint {
        original_operation_sha256: crate::canonical_sha256(original)?,
    })
}

/// Maps a typed adapter refusal carrying [`crate::stellar::RESTORATION_REQUIRED`]
pub fn prepare_from_simulation(
    original: &OperationV1,
    simulation_error: Error,
) -> Result<PrepareOutcome> {
    let marker = format!(": {}", crate::stellar::RESTORATION_REQUIRED);
    match &simulation_error {
        Error::Chain(message) if message.ends_with(&marker) => Ok(
            PrepareOutcome::RestorationRequired(restoration_required(original)?),
        ),
        _ => Err(simulation_error),
    }
}

/// Live HTTP adapters for proposal construction. `None` on a mutation path
/// means the command cannot bind live state and fails closed.
pub struct LiveAdapters<'a> {
    pub stellar_url: Option<&'a str>,
    pub evm_url: Option<&'a str>,
}

fn require_url<'a>(url: Option<&'a str>, chain: &str) -> Result<&'a str> {
    url.ok_or_else(|| {
        Error::InvalidInput(format!(
            "live_environment_required: {chain} RPC URL is required to bind a proposal"
        ))
    })
}

/// Builds an executable plan bound to live adapter state: Stellar proposals
/// bind passphrase, source account, sequence, time bounds, the authority
/// topology, and the RPC-simulated exact transaction envelope with its
/// authorization entries; EVM proposals bind chain ID, nonce, typed
/// calldata, a live estimated gas/fee policy, and Safe metadata when the
/// owner is a Safe. Offline tests inject fakes.
pub fn build_executable_plan(
    state: &RouteStateV1,
    operation: &OperationV1,
    stellar: &dyn crate::stellar::StellarChain,
    evm: &dyn crate::evm::EvmChain,
) -> Result<ExecutablePlanV1> {
    validate_operation(operation)?;
    let desired_sha256 = state.desired_sha256.clone();
    match operation_vm(operation) {
        Vm::Stellar => {
            let binding = stellar_plan_binding(state, operation, stellar)?;
            let simulation_sha256 = stellar_simulation_digest(&binding)?;
            executable_plan(
                state,
                operation,
                desired_sha256,
                simulation_sha256,
                Some(binding),
                None,
            )
        }
        Vm::Evm => {
            let binding = evm_plan_binding(state, operation, evm)?;
            let simulation_sha256 = binding.transaction_digest.clone();
            executable_plan(
                state,
                operation,
                desired_sha256,
                simulation_sha256,
                None,
                Some(binding),
            )
        }
    }
}

/// Digest of the constructed transaction's canonical bytes: the decoded
/// envelope XDR of the exact transaction the proposal commits to.
fn stellar_simulation_digest(binding: &crate::domain::StellarPlanBindingV1) -> Result<String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(binding.envelope_xdr.as_bytes())
        .map_err(|_| Error::Chain("adapter envelope XDR is not valid base64".into()))?;
    if bytes.is_empty() {
        return Err(Error::Chain(
            "adapter returned an empty transaction envelope".into(),
        ));
    }
    let digest = hex::encode(Sha256::digest(&bytes));
    if binding.envelope_sha256 != digest {
        return Err(Error::Custody(
            "adapter envelope digest does not match envelope XDR".into(),
        ));
    }
    Ok(digest)
}

/// Shared plan envelope; the simulation digest binds the constructed
/// transaction's canonical bytes.
fn executable_plan(
    state: &RouteStateV1,
    operation: &OperationV1,
    desired_sha256: String,
    simulation_sha256: String,
    stellar_binding: Option<crate::domain::StellarPlanBindingV1>,
    evm_binding: Option<crate::domain::EvmPlanBindingV1>,
) -> Result<ExecutablePlanV1> {
    let now = crate::now_unix()?;
    Ok(ExecutablePlanV1 {
        schema_name: "executable_plan".into(),
        schema_version: SCHEMA_VERSION,
        route_id: state.route_id.clone(),
        desired_sha256,
        operation: operation.clone(),
        artifact_lock_sha256: crate::artifacts::lock_sha256()?,
        simulation_sha256,
        expires_at_unix: now
            .checked_add(900)
            .ok_or_else(|| Error::Custody("proposal expiry overflow".into()))?,
        stellar: stellar_binding,
        evm: evm_binding,
        continuation_sha256: String::new(),
    })
}

fn stellar_plan_binding(
    state: &RouteStateV1,
    operation: &OperationV1,
    stellar: &dyn crate::stellar::StellarChain,
) -> Result<crate::domain::StellarPlanBindingV1> {
    let passphrase = stellar.network_passphrase()?;
    if passphrase != state.identity.stellar_passphrase {
        return Err(Error::Policy(
            "derived environment mismatch: RPC passphrase differs from route identity".into(),
        ));
    }
    let sender = crate::layerzero::stellar_operation_authorizer(state, operation)?.to_string();
    let sequence = stellar.account_sequence(&sender)?;
    let ledger = stellar.latest_ledger()?;
    let weights = stellar.account_signers(&sender)?;
    let level = threshold_level(operation);
    let threshold = stellar.account_threshold(&sender, level)?;
    let available: u64 = weights.values().map(|weight| u64::from(*weight)).sum();
    if available < u64::from(threshold) {
        return Err(Error::Policy(format!(
            "insufficient signer weight {available} for threshold {threshold} ({level})"
        )));
    }
    let min_ledger = ledger;
    let max_ledger = ledger.saturating_add(1_000);
    // RPC-backed simulation of the exact typed transaction this proposal
    // commits to; the adapter owns construction, simulation, assembly, and
    // auth-class qualification and refuses rather than fabricating.
    let simulation = stellar
        .simulate_transaction(state, operation, &sender, &sequence, min_ledger, max_ledger)?;
    Ok(crate::domain::StellarPlanBindingV1 {
        network_passphrase: passphrase,
        source_account: sender,
        sequence,
        min_ledger,
        max_ledger,
        auth_entries: simulation.auth_entries,
        envelope_xdr: simulation.envelope_xdr,
        envelope_sha256: simulation.envelope_sha256,
        simulation_ledger: simulation.simulation_ledger,
        signer_weights: weights,
        required_threshold_weight: threshold,
        threshold_level: level.to_string(),
    })
}

fn evm_plan_binding(
    state: &RouteStateV1,
    operation: &OperationV1,
    evm: &dyn crate::evm::EvmChain,
) -> Result<crate::domain::EvmPlanBindingV1> {
    use std::str::FromStr as _;
    let chain_id = crate::block_on_result(evm.chain_id())?;
    if chain_id != state.identity.evm_chain_id {
        return Err(Error::Policy(
            "derived environment mismatch: RPC chain id differs from route identity".into(),
        ));
    }
    let owner = crate::layerzero::evm_operation_authorizer(state, operation)?.to_string();
    let address = crate::evm::parse_address(&owner)?;
    let nonce = crate::block_on_result(evm.account_nonce(address))?;
    let creation = matches!(operation, OperationV1::DeployEvmOft { .. });
    let calldata = if creation {
        evm.deployment_init_code(operation)?
    } else {
        crate::layerzero::encode_calldata_for_route(state, operation)?
    };
    if let OperationV1::DeployEvmOft {
        nonce: reserved_nonce,
        ..
    } = operation
    {
        if nonce != *reserved_nonce {
            return Err(Error::Conflict(format!(
                "live EVM deployer nonce {nonce} differs from reserved nonce {reserved_nonce}"
            )));
        }
    }
    let target = if creation {
        "create".to_string()
    } else {
        match operation {
            OperationV1::CommitVerification {
                vm: Vm::Evm,
                message,
            } => message.current_receive_library.clone(),
            OperationV1::SetEvmSendLibrary { .. }
            | OperationV1::SetEvmReceiveLibrary { .. }
            | OperationV1::RemoveEvmReceiveLibraryTimeout { .. }
            | OperationV1::SetEvmUlnConfig { .. }
            | OperationV1::SetEvmExecutorConfig { .. }
            | OperationV1::ContainOutbound { .. }
            | OperationV1::RestoreOutbound { .. }
            | OperationV1::ExecuteReceive { vm: Vm::Evm, .. } => {
                state.identity.evm_endpoint.clone()
            }
            _ => state.contracts.get("evm_oft").cloned().unwrap_or_default(),
        }
    };
    let value = match operation {
        OperationV1::SendLeg {
            vm: Vm::Evm,
            intent,
        } => alloy::primitives::U256::from_str(&intent.native_fee_raw).map_err(|error| {
            Error::InvalidInput(format!("invalid EVM send native fee: {error}"))
        })?,
        _ => alloy::primitives::U256::ZERO,
    };
    let estimate = if creation {
        crate::block_on_result(evm.estimate_creation(address, value, calldata.clone()))?
    } else {
        crate::block_on_result(evm.estimate_transaction(
            address,
            crate::evm::parse_address(&target)?,
            value,
            calldata.clone(),
        ))?
    };
    let safe_binding = match crate::block_on_result(evm.safe_state(address))? {
        Some(_) if creation => {
            return Err(Error::Policy(
                "deploy_evm_oft requires a plain EOA; Safe CREATE is unsupported".into(),
            ))
        }
        Some((threshold, safe_nonce)) => {
            let safe = crate::domain::SafeTransactionV1 {
                to: target.clone(),
                value: value.to_string(),
                data: hex::encode(&calldata),
                operation: 0,
                safe_tx_gas: "0".into(),
                base_gas: "0".into(),
                gas_price: "0".into(),
                gas_token: "0x0000000000000000000000000000000000000000".into(),
                refund_receiver: "0x0000000000000000000000000000000000000000".into(),
                nonce: safe_nonce.clone(),
                threshold,
                safe_tx_hash: safe_tx_hash(evm, address, &target, &calldata, &safe_nonce)?,
            };
            Some(safe)
        }
        None => None,
    };
    let binding = crate::domain::EvmPlanBindingV1 {
        chain_id: chain_id.to_string(),
        sender: crate::evm::canonical_address(address),
        target,
        value: value.to_string(),
        nonce: nonce.to_string(),
        calldata: format!("0x{}", hex::encode(&calldata)),
        gas_limit: estimate.gas_limit.to_string(),
        max_fee_per_gas_wei: estimate.max_fee_per_gas_wei,
        max_priority_fee_per_gas_wei: estimate.max_priority_fee_per_gas_wei,
        transaction_digest: String::new(),
        safe: safe_binding,
    };
    let transaction_digest =
        hex::encode(crate::evm::keccak256_of(&crate::canonical_bytes(&binding)?));
    Ok(crate::domain::EvmPlanBindingV1 {
        transaction_digest,
        ..binding
    })
}

/// Computes the Safe transaction hash through the live Safe contract's own
/// `getTransactionHash` view over the exact bound fields. The digest is the
/// contract's answer, never a locally guessed EIP-712 encoding; a failing
/// view call fails closed.
fn safe_tx_hash(
    evm: &dyn crate::evm::EvmChain,
    safe: alloy::primitives::Address,
    to: &str,
    data: &[u8],
    nonce: &str,
) -> Result<String> {
    let zero = alloy::primitives::U256::ZERO;
    let nonce = nonce
        .parse::<alloy::primitives::U256>()
        .map_err(|_| Error::Chain("safe nonce is not a decimal string".to_string()))?;
    let calldata = Safe::getTransactionHashCall {
        to: crate::evm::parse_address(to)?,
        value: zero,
        data: data.to_vec().into(),
        operation: 0,
        safeTxGas: zero,
        baseGas: zero,
        gasPrice: zero,
        gasToken: alloy::primitives::Address::ZERO,
        refundReceiver: alloy::primitives::Address::ZERO,
        nonce,
    }
    .abi_encode();
    let result = crate::block_on_result(evm.call(safe, calldata))?;
    if result.len() != 32 {
        return Err(Error::Chain(
            "safe getTransactionHash returned a non-word result".into(),
        ));
    }
    Ok(format!("0x{}", hex::encode(&result)))
}

/// Recovery scenario the matrix classifies.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryScenario {
    /// Outbound is contained; restore is required.
    OutboundContained,
    /// Packet never reached the destination.
    PacketUndelivered,
    /// Packet delivered but not acknowledged.
    PacketDeliveredUnacknowledged,
    /// Packet acknowledged but value not settled.
    PacketAcknowledgedUnsettled,
}

/// Mechanism a recovery would use.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMechanism {
    /// Restore outbound libraries from the containment snapshot.
    OperatorRestore,
    /// Resend the packet through the operator.
    OperatorResend,
    /// Refund the packet value through the operator.
    OperatorRefund,
    /// Withdraw stuck value through the owner after the timelock.
    OwnerTimelockWithdraw,
}

/// One cell of the recovery capability matrix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryCapabilityV1 {
    /// No v1 recovery path exists for this cell.
    Unavailable { reason: String },
    /// Recovery is authorized through the mechanism.
    Authorized {
        mechanism: RecoveryMechanism,
        requires_source_account: bool,
    },
}

/// Pure recovery capability matrix over the intervening VM and scenario.
pub fn recovery_capability(vm: Vm, scenario: RecoveryScenario) -> RecoveryCapabilityV1 {
    match scenario {
        RecoveryScenario::OutboundContained => RecoveryCapabilityV1::Authorized {
            mechanism: RecoveryMechanism::OperatorRestore,
            requires_source_account: true,
        },
        RecoveryScenario::PacketUndelivered => RecoveryCapabilityV1::Authorized {
            mechanism: RecoveryMechanism::OperatorResend,
            requires_source_account: true,
        },
        RecoveryScenario::PacketDeliveredUnacknowledged => RecoveryCapabilityV1::Unavailable {
            reason: "await_native_acknowledgement_v1".to_string(),
        },
        RecoveryScenario::PacketAcknowledgedUnsettled => match vm {
            Vm::Stellar => RecoveryCapabilityV1::Authorized {
                mechanism: RecoveryMechanism::OperatorRefund,
                requires_source_account: true,
            },
            Vm::Evm => RecoveryCapabilityV1::Authorized {
                mechanism: RecoveryMechanism::OwnerTimelockWithdraw,
                requires_source_account: false,
            },
        },
    }
}

fn validate_operation(operation: &OperationV1) -> Result<()> {
    match operation {
        OperationV1::SetStellarUlnConfig {
            config_sha256,
            config,
            ..
        }
        | OperationV1::SetEvmUlnConfig {
            config_sha256,
            config,
            ..
        } => {
            let typed: crate::layerzero::UlnConfigType3V1 = serde_json::from_value(config.clone())
                .map_err(|error| {
                    Error::InvalidInput(format!("invalid typed ULN config: {error}"))
                })?;
            typed.validate()?;
            if &crate::canonical_sha256(config)? != config_sha256 {
                return Err(Error::Custody("ULN config digest mismatch".into()));
            }
        }
        OperationV1::SetStellarExecutorConfig {
            config_sha256,
            config,
            ..
        }
        | OperationV1::SetEvmExecutorConfig {
            config_sha256,
            config,
            ..
        } => {
            let typed: crate::layerzero::ExecutorConfigType3V1 =
                serde_json::from_value(config.clone()).map_err(|error| {
                    Error::InvalidInput(format!("invalid typed executor config: {error}"))
                })?;
            typed.validate()?;
            if &crate::canonical_sha256(config)? != config_sha256 {
                return Err(Error::Custody("executor config digest mismatch".into()));
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_plan(plan: &ExecutablePlanV1) -> Result<()> {
    if plan.schema_name != "executable_plan" || plan.schema_version != SCHEMA_VERSION {
        return Err(Error::InvalidInput(
            "unsupported executable plan schema".into(),
        ));
    }
    if plan.route_id.trim().is_empty() {
        return Err(Error::InvalidInput(
            "plan route_id must not be empty".into(),
        ));
    }
    validate_operation(&plan.operation)?;
    if plan.simulation_sha256.trim().is_empty() {
        return Err(Error::InvalidInput(
            "plan must bind a simulation digest".into(),
        ));
    }
    if plan.stellar.is_some() == plan.evm.is_some() {
        return Err(Error::InvalidInput(
            "plan must bind exactly one chain".into(),
        ));
    }
    if let Some(stellar) = plan.stellar.as_ref() {
        if stellar.envelope_xdr.trim().is_empty() {
            return Err(Error::InvalidInput(
                "stellar plan must bind the constructed envelope".into(),
            ));
        }
    }
    if let Some(evm) = plan.evm.as_ref() {
        let sender = crate::evm::parse_address(&evm.sender)?;
        if evm.sender != crate::evm::canonical_address(sender) {
            return Err(Error::InvalidInput(
                "evm plan sender must be a canonical lowercase address".into(),
            ));
        }
        let gas_policy = [
            ("gas_limit", &evm.gas_limit),
            ("max_fee_per_gas_wei", &evm.max_fee_per_gas_wei),
            (
                "max_priority_fee_per_gas_wei",
                &evm.max_priority_fee_per_gas_wei,
            ),
        ];
        for (field, value) in gas_policy {
            if value.trim().is_empty() || value.parse::<u128>().is_err() {
                return Err(Error::InvalidInput(format!(
                    "evm plan {field} must be a live decimal estimate"
                )));
            }
        }
        if let Some(safe) = evm.safe.as_ref() {
            let hash = safe.safe_tx_hash.strip_prefix("0x").unwrap_or_default();
            if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(Error::InvalidInput(
                    "safe transaction hash must be the live adapter digest".into(),
                ));
            }
        }
    }
    Ok(())
}

fn require_fresh_plan(plan: &ExecutablePlanV1) -> Result<()> {
    let now = crate::now_unix()?;
    if plan.expires_at_unix <= now {
        return Err(Error::Conflict("proposal has expired".into()));
    }
    Ok(())
}

/// VM an operation mutates. Dual-sided containment follows the outbound
/// direction; cross-VM recovery operations carry their explicit `vm`.
pub fn operation_vm(operation: &crate::domain::OperationV1) -> Vm {
    use crate::domain::OperationV1;
    match operation {
        OperationV1::DeployEvmOft { .. }
        | OperationV1::TransferEvmOwnership { .. }
        | OperationV1::SetEvmDelegate { .. }
        | OperationV1::SetEvmPeer { .. }
        | OperationV1::SetEvmSendLibrary { .. }
        | OperationV1::SetEvmReceiveLibrary { .. }
        | OperationV1::RemoveEvmReceiveLibraryTimeout { .. }
        | OperationV1::SetEvmUlnConfig { .. }
        | OperationV1::SetEvmExecutorConfig { .. }
        | OperationV1::SetEvmReceiveOptions { .. } => Vm::Evm,
        OperationV1::CommitVerification { vm, .. } | OperationV1::ExecuteReceive { vm, .. } => *vm,
        OperationV1::ContainOutbound { snapshot } | OperationV1::RestoreOutbound { snapshot } => {
            match snapshot.direction {
                crate::domain::Direction::StellarToEvm => Vm::Stellar,
                crate::domain::Direction::EvmToStellar => Vm::Evm,
            }
        }
        OperationV1::InstallStellarWasm { .. }
        | OperationV1::DeployStellarOft { .. }
        | OperationV1::BeginStellarOwnershipTransfer { .. }
        | OperationV1::AcceptStellarOwnership
        | OperationV1::CancelStellarOwnershipTransfer
        | OperationV1::SetStellarDelegate { .. }
        | OperationV1::SetStellarPeer { .. }
        | OperationV1::SetStellarSendLibrary { .. }
        | OperationV1::SetStellarReceiveLibrary { .. }
        | OperationV1::RemoveStellarReceiveLibraryTimeout { .. }
        | OperationV1::SetStellarUlnConfig { .. }
        | OperationV1::SetStellarExecutorConfig { .. }
        | OperationV1::SetStellarReceiveOptions { .. }
        | OperationV1::SetDefaultFee { .. }
        | OperationV1::SetDestinationFee { .. }
        | OperationV1::SetFeeRecipient { .. }
        | OperationV1::SetMessageInspector { .. }
        | OperationV1::SetInboundRateLimit { .. }
        | OperationV1::SetOutboundRateLimit { .. }
        | OperationV1::PauseEmergency
        | OperationV1::UnpauseEmergency
        | OperationV1::SetTtlConfig { .. }
        | OperationV1::FreezeTtlConfig { .. }
        | OperationV1::ExtendInstanceTtl { .. }
        | OperationV1::GrantRole { .. }
        | OperationV1::RevokeRole { .. }
        | OperationV1::SetRoleAdmin { .. }
        | OperationV1::RemoveRoleAdmin { .. }
        | OperationV1::RestoreFootprint { .. } => Vm::Stellar,
        OperationV1::SendLeg { vm, .. } => *vm,
    }
}

/// Required Stellar threshold level for an operation.
pub fn threshold_level(operation: &crate::domain::OperationV1) -> &'static str {
    use crate::domain::OperationV1;
    match operation {
        OperationV1::BeginStellarOwnershipTransfer { .. }
        | OperationV1::AcceptStellarOwnership
        | OperationV1::CancelStellarOwnershipTransfer => "high",
        _ => "medium",
    }
}

/// Builds a deterministic proposal for external authorization.
pub fn build_proposal(environment: Environment, plan: ExecutablePlanV1) -> Result<ProposalV1> {
    validate_plan(&plan)?;
    require_plan_environment(environment, &plan)?;
    Ok(ProposalV1 {
        schema_name: "proposal".to_string(),
        schema_version: SCHEMA_VERSION,
        plan,
        signatures: BTreeMap::new(),
    })
}

/// Verifies and attaches one externally produced Stellar signature.
pub fn attach_signature(
    environment: Environment,
    proposal: &ProposalV1,
    signer: &str,
    signature: &str,
) -> Result<ProposalV1> {
    validate_plan(&proposal.plan)?;
    require_plan_environment(environment, &proposal.plan)?;
    require_fresh_plan(&proposal.plan)?;
    verify_stellar_signature(proposal, signer, signature)?;
    if let Some(existing) = proposal.signatures.get(signer) {
        if existing == signature {
            return Ok(proposal.clone());
        }
        return Err(Error::Conflict(format!(
            "proposal already has a different signature for {signer}"
        )));
    }
    let mut attached = proposal.clone();
    attached
        .signatures
        .insert(signer.to_string(), signature.to_string());
    Ok(attached)
}

fn verify_stellar_signature(proposal: &ProposalV1, signer: &str, signature: &str) -> Result<u32> {
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
    use stellar_baselib::{
        keypair::KeypairBehavior as _,
        transaction::{Transaction, TransactionBehavior as _},
    };

    let binding = proposal
        .plan
        .stellar
        .as_ref()
        .ok_or_else(|| Error::Policy("Stellar signatures require a Stellar proposal".into()))?;
    let weight = *binding
        .signer_weights
        .get(signer)
        .ok_or_else(|| Error::Policy(format!("{signer} is not an authorized Stellar signer")))?;
    if weight == 0 {
        return Err(Error::Policy(format!(
            "{signer} has zero Stellar signer weight"
        )));
    }
    let signature = hex::decode(signature.trim_start_matches("0x"))
        .or_else(|_| BASE64_STANDARD.decode(signature))
        .map_err(|_| Error::InvalidInput("Stellar signature must be hex or base64".into()))?;
    if signature.len() != 64 {
        return Err(Error::InvalidInput(
            "Stellar signature must be exactly 64 bytes".into(),
        ));
    }
    let keypair = stellar_baselib::keypair::Keypair::from_public_key(signer)
        .map_err(|error| Error::InvalidInput(format!("invalid Stellar signer: {error}")))?;
    let transaction = std::panic::catch_unwind(|| {
        Transaction::from_xdr_envelope(&binding.envelope_xdr, &binding.network_passphrase)
    })
    .map_err(|_| Error::InvalidInput("proposal contains invalid Stellar envelope XDR".into()))?;
    if !keypair.verify(&transaction.hash(), &signature) {
        return Err(Error::Policy(format!(
            "invalid Stellar signature for {signer}"
        )));
    }
    Ok(weight)
}

/// Derives typed out-of-band signing material from a proposal's chain
/// binding. The unsigned payload is the Stellar envelope XDR when present,
/// otherwise the EVM transaction digest.
pub fn signature_verification_data(
    environment: Environment,
    proposal: &ProposalV1,
) -> Result<SignatureVerificationDataV1> {
    validate_plan(&proposal.plan)?;
    require_plan_environment(environment, &proposal.plan)?;
    require_fresh_plan(&proposal.plan)?;
    let (vm, sender, sequence_or_nonce, unsigned_payload) =
        match (proposal.plan.stellar.as_ref(), proposal.plan.evm.as_ref()) {
            (Some(binding), None) => (
                Vm::Stellar,
                binding.source_account.clone(),
                binding.sequence.clone(),
                binding.envelope_xdr.clone(),
            ),
            (None, Some(binding)) => (
                Vm::Evm,
                String::new(),
                binding.nonce.clone(),
                binding.transaction_digest.clone(),
            ),
            _ => {
                return Err(Error::InvalidInput(
                    "plan must bind exactly one chain".into(),
                ))
            }
        };
    if unsigned_payload.trim().is_empty() {
        return Err(Error::Chain(
            "proposal has no unsigned payload yet; construct the envelope via the qualified adapter".into(),
        ));
    }
    let (attached_weight, required_weight) = if let Some(binding) = &proposal.plan.stellar {
        let attached_weight =
            proposal
                .signatures
                .iter()
                .try_fold(0u32, |total, (signer, signature)| {
                    verify_stellar_signature(proposal, signer, signature).and_then(|weight| {
                        total
                            .checked_add(weight)
                            .ok_or_else(|| Error::Custody("Stellar signer weight overflow".into()))
                    })
                })?;
        (attached_weight, binding.required_threshold_weight)
    } else {
        (0, 0)
    };
    let unsigned_payload_sha256 = hex::encode(Sha256::digest(unsigned_payload.as_bytes()));
    let plan_sha256 = crate::canonical_sha256(&proposal.plan)?;
    Ok(SignatureVerificationDataV1 {
        route_id: proposal.plan.route_id.clone(),
        vm,
        sender,
        sequence_or_nonce,
        unsigned_payload,
        unsigned_payload_sha256,
        attached_weight,
        required_weight,
        threshold_satisfied: required_weight > 0 && attached_weight >= required_weight,
        plan_sha256,
        expires_at_unix: proposal.plan.expires_at_unix,
    })
}

/// Checked native adapter trait for governance paths.
pub trait GovernancePolicyAdapter {
    fn plan_recovery(
        &self,
        environment: Environment,
        vm: Vm,
        scenario: RecoveryScenario,
    ) -> Result<RecoveryCapabilityV1>;
    fn build_proposal(
        &self,
        environment: Environment,
        plan: ExecutablePlanV1,
    ) -> Result<ProposalV1>;
    fn attach_signature(
        &self,
        environment: Environment,
        proposal: &ProposalV1,
        signer: &str,
        signature: &str,
    ) -> Result<ProposalV1>;
    fn signature_verification_data(
        &self,
        environment: Environment,
        proposal: &ProposalV1,
    ) -> Result<SignatureVerificationDataV1>;
}

/// Checked adapter implementation of [GovernancePolicyAdapter].
#[derive(Debug, Default)]
pub struct CheckedGovernancePolicy;

impl GovernancePolicyAdapter for CheckedGovernancePolicy {
    fn plan_recovery(
        &self,
        _environment: Environment,
        vm: Vm,
        scenario: RecoveryScenario,
    ) -> Result<RecoveryCapabilityV1> {
        Ok(recovery_capability(vm, scenario))
    }

    fn build_proposal(
        &self,
        environment: Environment,
        plan: ExecutablePlanV1,
    ) -> Result<ProposalV1> {
        build_proposal(environment, plan)
    }

    fn attach_signature(
        &self,
        environment: Environment,
        proposal: &ProposalV1,
        signer: &str,
        signature: &str,
    ) -> Result<ProposalV1> {
        attach_signature(environment, proposal, signer, signature)
    }

    fn signature_verification_data(
        &self,
        environment: Environment,
        proposal: &ProposalV1,
    ) -> Result<SignatureVerificationDataV1> {
        signature_verification_data(environment, proposal)
    }
}

use crate::domain::{OperationDraftV1, RouteStateV1};
use crate::output::CommandData;
use crate::state::{read_json, RouteStore};
use std::path::Path;

fn route_environment(state_path: &Path) -> Result<(RouteStateV1, RouteStore)> {
    let store = RouteStore::open(state_path)?;
    let state = store.load_state()?;
    Ok((state, store))
}

/// `proposal create`: re-derives a fresh plan from a draft. The
/// draft's operation bytes are never trusted; only its identity.
pub fn create_proposal(
    state_path: &Path,
    draft_path: &Path,
    out: &Path,
    stellar_rpc: Option<&str>,
    evm_rpc: Option<&str>,
) -> Result<CommandData> {
    let (state, store) = route_environment(state_path)?;
    crate::environment::classify(&state.identity)?;
    let draft: OperationDraftV1 = read_json(draft_path)?;
    if draft.route_id != state.route_id || draft.desired_sha256 != state.desired_sha256 {
        return Err(Error::Conflict(
            "draft does not bind to this route state".into(),
        ));
    }
    let plan = with_live_adapters(state_path, stellar_rpc, evm_rpc, |stellar, evm| {
        crate::environment::verify_live_endpoint_code(&state.identity, stellar, evm)?;
        build_executable_plan(&state, &draft.operation, stellar, evm)
    })?;
    let operation_sha256 = crate::canonical_sha256(&draft.operation)?;
    let proposal = build_proposal(state.identity.environment, plan)?;
    let relative = Path::new("proposals").join(
        out.file_name()
            .ok_or_else(|| Error::InvalidInput("proposal output must name a file".into()))?,
    );
    store.write_proposal(&relative, &operation_sha256, &proposal)?;
    Ok(CommandData {
        result: serde_json::to_value(&proposal)?,
        artifact: None,
    })
}

/// `--proposal-out`: serialize an operation for external authorization.
pub fn proposal_for_operation(
    state_path: &Path,
    operation: &crate::domain::OperationV1,
    out: &Path,
    stellar_rpc: Option<&str>,
    evm_rpc: Option<&str>,
) -> Result<CommandData> {
    let (state, store) = route_environment(state_path)?;
    crate::environment::classify(&state.identity)?;
    let plan = with_live_adapters(state_path, stellar_rpc, evm_rpc, |stellar, evm| {
        crate::environment::verify_live_endpoint_code(&state.identity, stellar, evm)?;
        build_executable_plan(&state, operation, stellar, evm)
    })?;
    let proposal = build_proposal(state.identity.environment, plan)?;
    let operation_sha256 = crate::canonical_sha256(operation)?;
    let relative = Path::new("proposals").join(
        out.file_name()
            .ok_or_else(|| Error::InvalidInput("proposal output must name a file".into()))?,
    );
    let artifact = store.write_proposal(&relative, &operation_sha256, &proposal)?;
    Ok(CommandData {
        result: serde_json::to_value(&proposal)?,
        artifact: Some(artifact),
    })
}

/// Builds the concrete HTTP adapter pair used to bind a proposal. Both RPC
/// URLs are mandatory: chain-identity checks read both sides.
fn with_live_adapters<T>(
    state_path: &Path,
    stellar_rpc: Option<&str>,
    evm_rpc: Option<&str>,
    build: impl FnOnce(&dyn crate::stellar::StellarChain, &dyn crate::evm::EvmChain) -> Result<T>,
) -> Result<T> {
    let stellar = crate::stellar::HttpStellarChain::new(require_url(stellar_rpc, "Stellar")?)?
        .with_artifact_root(state_path);
    let evm =
        crate::evm::HttpEvmChain::new(require_url(evm_rpc, "EVM")?)?.with_artifact_root(state_path);
    build(&stellar, &evm)
}

/// `proposal ingest`: verifies the finalized on-chain transaction against the
/// exact closed proposal before optionally appending authoritative evidence.
pub fn ingest_proposal(
    state_path: &Path,
    proposal_path: &Path,
    executed_tx: &str,
    stellar_rpc: Option<&str>,
    evm_rpc: Option<&str>,
    write: bool,
) -> Result<CommandData> {
    let state = RouteStore::open(state_path)?.load_state()?;
    crate::environment::classify(&state.identity)?;
    let stellar = stellar_rpc
        .map(crate::stellar::HttpStellarChain::new)
        .transpose()?;
    let evm = evm_rpc.map(crate::evm::HttpEvmChain::new).transpose()?;
    ingest_proposal_with_adapters(
        state_path,
        proposal_path,
        executed_tx,
        stellar
            .as_ref()
            .map(|chain| chain as &dyn crate::stellar::StellarChain),
        evm.as_ref().map(|chain| chain as &dyn crate::evm::EvmChain),
        write,
    )
}

pub fn ingest_proposal_with_adapters(
    state_path: &Path,
    proposal_path: &Path,
    executed_tx: &str,
    stellar: Option<&dyn crate::stellar::StellarChain>,
    evm: Option<&dyn crate::evm::EvmChain>,
    write: bool,
) -> Result<CommandData> {
    let (initial_state, store) = route_environment(state_path)?;
    let _write_lock = if write { Some(store.lock()?) } else { None };
    let state = if write {
        store.load_state()?
    } else {
        initial_state
    };
    crate::environment::classify(&state.identity)?;
    let proposal: ProposalV1 = read_json(proposal_path)?;
    validate_plan(&proposal.plan)?;
    require_plan_environment(state.identity.environment, &proposal.plan)?;
    if proposal.plan.route_id != state.route_id
        || proposal.plan.desired_sha256 != state.desired_sha256
    {
        return Err(Error::Conflict(
            "proposal does not bind to this route state".into(),
        ));
    }
    // Expiry controls review/signing. Once the exact transaction has executed,
    // custody accounting must still be able to ingest it indefinitely.
    let ingested_after_review_deadline = crate::now_unix()? > proposal.plan.expires_at_unix;
    if let Some(opening) = &state.opening_custody {
        if proposal.plan.artifact_lock_sha256 != opening.artifact_lock_sha256 {
            return Err(Error::Conflict(
                "proposal artifact lock differs from opening custody".into(),
            ));
        }
    }

    let mut evidence = match (&proposal.plan.stellar, &proposal.plan.evm) {
        (Some(binding), None) => {
            let chain = stellar.ok_or_else(|| {
                Error::InvalidInput("Stellar RPC URL is required to ingest this proposal".into())
            })?;
            let status = chain.transaction_status(executed_tx)?;
            if status.status != "success" || status.ledger.is_none() {
                return Err(Error::Chain(format!(
                    "Stellar transaction is not finalized successful: {}",
                    status.status
                )));
            }
            let included_ledger = status.ledger.ok_or_else(|| {
                Error::Chain("Stellar transaction response omitted inclusion ledger".into())
            })?;
            let latest_ledger = chain.latest_ledger()?;
            let canonical = chain.transaction_status(executed_tx)?;
            if latest_ledger <= included_ledger
                || canonical.status != "success"
                || canonical.ledger != Some(included_ledger)
            {
                return Err(Error::Chain(
                    "Stellar transaction has not reached canonical successor-ledger finality"
                        .into(),
                ));
            }
            let executed_envelope = status.envelope_xdr.as_deref().ok_or_else(|| {
                Error::Chain("Stellar transaction response omitted envelope XDR".into())
            })?;
            if crate::stellar::envelope_transaction_hash(
                executed_envelope,
                &binding.network_passphrase,
            )? != crate::stellar::envelope_transaction_hash(
                &binding.envelope_xdr,
                &binding.network_passphrase,
            )? {
                return Err(Error::Conflict(
                    "executed Stellar transaction differs from proposal".into(),
                ));
            }
            if write {
                if let OperationV1::SendLeg { intent, .. } = &proposal.plan.operation {
                    crate::canary::record_stellar_source_message(
                        &store,
                        intent,
                        &canonical,
                        executed_tx,
                    )?;
                }
            }
            serde_json::json!({
                "chain": "stellar",
                "transaction_hash": executed_tx,
                "ledger": status.ledger,
                "envelope_xdr": executed_envelope,
            })
        }
        (None, Some(binding)) => {
            let chain = evm.ok_or_else(|| {
                Error::InvalidInput("EVM RPC URL is required to ingest this proposal".into())
            })?;
            let transaction = crate::block_on_result(chain.transaction_by_hash(executed_tx))?
                .ok_or_else(|| Error::Chain("EVM transaction was not found".into()))?;
            let safe_address =
                crate::layerzero::evm_operation_authorizer(&state, &proposal.plan.operation)?;
            verify_evm_transaction(binding, &transaction, safe_address)?;
            let receipt = crate::block_on_result(chain.transaction_receipt(executed_tx))?
                .ok_or_else(|| Error::Chain("EVM receipt was not found".into()))?;
            if receipt.succeeded != Some(true) || receipt.block_number.is_none() {
                return Err(Error::Chain(
                    "EVM transaction is not finalized successful".into(),
                ));
            }
            let included_block = receipt
                .block_number
                .ok_or_else(|| Error::Chain("EVM receipt omitted inclusion block".into()))?;
            let included_hash = receipt
                .raw
                .get("blockHash")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| Error::Chain("EVM receipt omitted inclusion block hash".into()))?;
            let latest_block = crate::block_on_result(chain.latest_block())?;
            let canonical = crate::block_on_result(chain.transaction_receipt(executed_tx))?
                .ok_or_else(|| Error::Chain("EVM receipt disappeared before finality".into()))?;
            let canonical_hash = canonical
                .raw
                .get("blockHash")
                .and_then(serde_json::Value::as_str);
            if latest_block <= included_block
                || canonical.succeeded != Some(true)
                || canonical.block_number != Some(included_block)
                || canonical_hash.is_none_or(|hash| !hash.eq_ignore_ascii_case(included_hash))
            {
                return Err(Error::Chain(
                    "EVM transaction has not reached canonical successor-block finality".into(),
                ));
            }
            if write {
                if let OperationV1::SendLeg { intent, .. } = &proposal.plan.operation {
                    crate::canary::record_evm_source_message(&store, intent, &canonical)?;
                }
            }
            serde_json::json!({
                "chain": "evm",
                "transaction_hash": executed_tx,
                "transaction": transaction,
                "receipt": receipt,
            })
        }
        _ => {
            return Err(Error::InvalidInput(
                "proposal must bind exactly one chain".into(),
            ))
        }
    };
    evidence
        .as_object_mut()
        .ok_or_else(|| Error::Custody("proposal evidence is not an object".into()))?
        .insert(
            "ingested_after_review_deadline".into(),
            serde_json::Value::Bool(ingested_after_review_deadline),
        );

    if write {
        let stellar = stellar.ok_or_else(|| {
            Error::InvalidInput("both chain adapters are required for proposal readback".into())
        })?;
        let evm = evm.ok_or_else(|| {
            Error::InvalidInput("both chain adapters are required for proposal readback".into())
        })?;
        let mut observed = store.load_state()?;
        match crate::route::apply_live_readback(
            stellar,
            evm,
            &mut observed,
            &proposal.plan.operation,
        ) {
            Ok(()) => {}
            Err(Error::InvalidInput(_)) => {
                match crate::route::apply_management_readback(
                    stellar,
                    evm,
                    &mut observed,
                    &proposal.plan.operation,
                ) {
                    Ok(()) | Err(Error::InvalidInput(_)) => {}
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        }
        store.save_state(&observed)?;
        store.append_operation(
            crate::state::OperationEventV1 {
                operation_id: crate::canonical_sha256(&proposal.plan.operation)?,
                state: crate::state::OperationState::Confirmed,
                detail: evidence.clone(),
            },
            None,
        )?;
    }
    Ok(CommandData {
        result: serde_json::json!({"verified": true, "written": write, "evidence": evidence}),
        artifact: None,
    })
}

fn verify_evm_transaction(
    binding: &crate::domain::EvmPlanBindingV1,
    transaction: &serde_json::Value,
    expected_authorizer: &str,
) -> Result<()> {
    let field = |name: &str| {
        transaction
            .get(name)
            .ok_or_else(|| Error::Chain(format!("EVM transaction omitted {name}")))
    };
    let actual_to = field("to")?
        .as_str()
        .ok_or_else(|| Error::Chain("EVM transaction target is not a string".into()))?;
    let actual_input = transaction
        .get("input")
        .or_else(|| transaction.get("data"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Chain("EVM transaction omitted input".into()))?;
    let bound_sender = crate::evm::parse_address(&binding.sender)?;
    let expected_authorizer = crate::evm::parse_address(expected_authorizer)?;
    if bound_sender != expected_authorizer {
        return Err(Error::Conflict(
            "EVM plan sender differs from the expected authorizer".into(),
        ));
    }
    if let Some(safe) = binding.safe.as_ref() {
        if crate::evm::parse_address(actual_to)? != expected_authorizer {
            return Err(Error::Conflict(
                "executed Safe transaction targets a different Safe".into(),
            ));
        }
        return verify_safe_execution(safe, actual_input);
    }
    let actual_chain_id = json_uint(field("chainId")?)?;
    let actual_nonce = json_uint(field("nonce")?)?;
    let actual_value = json_uint(field("value")?)?;
    let actual_from = field("from")?
        .as_str()
        .ok_or_else(|| Error::Chain("EVM transaction sender is not a string".into()))?;
    let actual_from = crate::evm::parse_address(actual_from)?;
    let expected_to = &binding.target;
    if actual_chain_id != binding.chain_id
        || actual_nonce != binding.nonce
        || actual_value != binding.value
        || actual_from != bound_sender
        || !actual_to.eq_ignore_ascii_case(expected_to)
        || !actual_input.eq_ignore_ascii_case(&binding.calldata)
    {
        return Err(Error::Conflict(
            "executed EVM transaction differs from proposal".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod evm_transaction_binding_tests {
    use super::verify_evm_transaction;
    use crate::domain::EvmPlanBindingV1;
    use crate::error::Error;

    const SENDER: &str = "0x1111111111111111111111111111111111111111";
    const TARGET: &str = "0x2222222222222222222222222222222222222222";

    fn binding() -> EvmPlanBindingV1 {
        EvmPlanBindingV1 {
            chain_id: "11155111".into(),
            sender: SENDER.into(),
            target: TARGET.into(),
            value: "17".into(),
            nonce: "9".into(),
            calldata: "0x1234".into(),
            gas_limit: "50000".into(),
            max_fee_per_gas_wei: "2000000000".into(),
            max_priority_fee_per_gas_wei: "1000000000".into(),
            transaction_digest: "plan-digest".into(),
            safe: None,
        }
    }

    fn transaction(from: &str) -> serde_json::Value {
        serde_json::json!({
            "from": from,
            "to": TARGET,
            "chainId": "0xaa36a7",
            "nonce": "0x9",
            "value": "0x11",
            "input": "0x1234",
        })
    }

    #[test]
    fn accepts_direct_transaction_from_bound_sender() {
        verify_evm_transaction(&binding(), &transaction(SENDER), SENDER)
            .expect("matching recovered sender");
    }

    #[test]
    fn refuses_direct_transaction_from_different_sender() {
        let error = verify_evm_transaction(
            &binding(),
            &transaction("0x3333333333333333333333333333333333333333"),
            SENDER,
        )
        .expect_err("different recovered sender must refuse");
        assert!(matches!(error, Error::Conflict(_)));
        assert!(error
            .to_string()
            .contains("executed EVM transaction differs from proposal"));
    }

    #[test]
    fn refuses_direct_transaction_without_sender() {
        let mut transaction = transaction(SENDER);
        transaction.as_object_mut().expect("object").remove("from");
        let error = verify_evm_transaction(&binding(), &transaction, SENDER)
            .expect_err("missing recovered sender must refuse");
        assert!(matches!(error, Error::Chain(_)));
        assert!(error.to_string().contains("omitted from"));
    }

    #[test]
    fn refuses_plan_sender_that_differs_from_route_authorizer() {
        let error = verify_evm_transaction(
            &binding(),
            &transaction(SENDER),
            "0x3333333333333333333333333333333333333333",
        )
        .expect_err("plan sender must remain bound to route authority");
        assert!(matches!(error, Error::Conflict(_)));
        assert!(error.to_string().contains("expected authorizer"));
    }
}

fn verify_safe_execution(safe: &crate::domain::SafeTransactionV1, input: &str) -> Result<()> {
    let input = hex::decode(input.trim_start_matches("0x"))
        .map_err(|_| Error::Chain("Safe execution calldata is not hex".into()))?;
    let selector = &crate::evm::keccak256_of(
        b"execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)",
    )[..4];
    if input.len() < 4 + 10 * 32 || input[..4] != *selector {
        return Err(Error::Conflict(
            "executed Safe transaction has the wrong selector or head".into(),
        ));
    }
    let words = &input[4..4 + 10 * 32];
    let word = |index: usize| &words[index * 32..(index + 1) * 32];
    let address = |index: usize| {
        crate::evm::canonical_address(alloy::primitives::Address::from_slice(&word(index)[12..]))
    };
    let uint = |index: usize| alloy::primitives::U256::from_be_slice(word(index)).to_string();
    let offset = |index: usize| -> Result<usize> {
        if word(index)[..24].iter().any(|byte| *byte != 0) {
            return Err(Error::Chain("Safe dynamic offset exceeds usize".into()));
        }
        Ok(usize::from_be_bytes(word(index)[24..].try_into().map_err(
            |_| Error::Chain("Safe dynamic offset is malformed".into()),
        )?))
    };
    let dynamic = |offset: usize| -> Result<&[u8]> {
        let start = 4usize
            .checked_add(offset)
            .ok_or_else(|| Error::Chain("Safe dynamic offset overflow".into()))?;
        let length_end = start
            .checked_add(32)
            .filter(|end| *end <= input.len())
            .ok_or_else(|| Error::Chain("Safe dynamic length is out of bounds".into()))?;
        if input[start..start + 24].iter().any(|byte| *byte != 0) {
            return Err(Error::Chain("Safe dynamic length exceeds usize".into()));
        }
        let length = usize::from_be_bytes(
            input[start + 24..length_end]
                .try_into()
                .map_err(|_| Error::Chain("Safe dynamic length is malformed".into()))?,
        );
        let value_start = length_end;
        let value_end = value_start
            .checked_add(length)
            .filter(|end| *end <= input.len())
            .ok_or_else(|| Error::Chain("Safe dynamic value is out of bounds".into()))?;
        Ok(&input[value_start..value_end])
    };
    let data = dynamic(offset(2)?)?;
    let signatures = dynamic(offset(9)?)?;
    if !address(0).eq_ignore_ascii_case(&safe.to)
        || uint(1) != safe.value
        || !hex::encode(data).eq_ignore_ascii_case(safe.data.trim_start_matches("0x"))
        || word(3)[..31].iter().any(|byte| *byte != 0)
        || word(3)[31] != safe.operation
        || uint(4) != safe.safe_tx_gas
        || uint(5) != safe.base_gas
        || uint(6) != safe.gas_price
        || !address(7).eq_ignore_ascii_case(&safe.gas_token)
        || !address(8).eq_ignore_ascii_case(&safe.refund_receiver)
        || signatures.is_empty()
    {
        return Err(Error::Conflict(
            "executed Safe transaction differs from proposal".into(),
        ));
    }
    Ok(())
}

fn json_uint(value: &serde_json::Value) -> Result<String> {
    use alloy::primitives::U256;
    use std::str::FromStr as _;

    if let Some(value) = value.as_u64() {
        return Ok(value.to_string());
    }
    let value = value
        .as_str()
        .ok_or_else(|| Error::Chain("EVM integer field is not a string or integer".into()))?;
    let parsed = if let Some(hex) = value.strip_prefix("0x") {
        U256::from_str_radix(hex, 16)
    } else {
        U256::from_str(value)
    }
    .map_err(|error| Error::Chain(format!("invalid EVM integer field: {error}")))?;
    Ok(parsed.to_string())
}

/// `proposal stellar-signature attach`: verifies an external signature and
/// writes a new proposal file.
pub fn attach_signature_command(
    state_path: &Path,
    proposal_path: &Path,
    public_key: &str,
    signature: &str,
    out: &Path,
) -> Result<CommandData> {
    let (state, _store) = route_environment(state_path)?;
    crate::environment::classify(&state.identity)?;
    let proposal: ProposalV1 = read_json(proposal_path)?;
    let attached = attach_signature(state.identity.environment, &proposal, public_key, signature)?;
    crate::state::write_create_new_json(out, &attached)?;
    Ok(CommandData {
        result: serde_json::to_value(&attached)?,
        artifact: None,
    })
}

/// `proposal stellar-signature verify`: read-only typed verification data.
pub fn verify_stellar_proposal(state_path: &Path, proposal_path: &Path) -> Result<CommandData> {
    let (state, _store) = route_environment(state_path)?;
    crate::environment::classify(&state.identity)?;
    let proposal: ProposalV1 = read_json(proposal_path)?;
    let data = signature_verification_data(state.identity.environment, &proposal)?;
    Ok(CommandData {
        result: serde_json::to_value(&data)?,
        artifact: None,
    })
}

#[derive(Deserialize)]
struct SafeExecutionEvidenceV1 {
    #[serde(flatten)]
    transaction: crate::domain::SafeTransactionV1,
    confirmations: Vec<SafeConfirmationV1>,
}

#[derive(Deserialize)]
struct SafeConfirmationV1 {
    owner: String,
    signature: String,
}

/// `proposal safe verify`: checks the exact Safe payload, current threshold
/// and nonce, EOA signatures, and live Safe owner membership.
pub fn verify_safe_proposal(
    state_path: &Path,
    proposal_path: &Path,
    safe_tx: &Path,
    evm_rpc: Option<&str>,
) -> Result<CommandData> {
    use crate::evm::EvmChain as _;
    use alloy::sol_types::SolCall as _;
    use std::str::FromStr as _;

    let (state, _store) = route_environment(state_path)?;
    crate::environment::classify(&state.identity)?;
    let proposal: ProposalV1 = read_json(proposal_path)?;
    validate_plan(&proposal.plan)?;
    require_plan_environment(state.identity.environment, &proposal.plan)?;
    require_fresh_plan(&proposal.plan)?;
    if proposal.plan.route_id != state.route_id
        || proposal.plan.desired_sha256 != state.desired_sha256
    {
        return Err(Error::Conflict(
            "proposal does not bind to this route state".into(),
        ));
    }
    let binding = proposal
        .plan
        .evm
        .as_ref()
        .and_then(|binding| binding.safe.as_ref())
        .ok_or_else(|| Error::Policy("proposal is not controlled by an EVM Safe".into()))?;
    let evidence: SafeExecutionEvidenceV1 = read_json(safe_tx)?;
    if &evidence.transaction != binding {
        return Err(Error::Conflict(
            "Safe transaction file differs from proposal".into(),
        ));
    }
    let safe = crate::evm::parse_address(
        state
            .contracts
            .get("evm_owner")
            .ok_or_else(|| Error::Custody("route has no recorded EVM owner".into()))?,
    )?;
    let evm = crate::evm::HttpEvmChain::new(require_url(evm_rpc, "EVM")?)?;
    let live = crate::block_on_result(evm.safe_state(safe))?
        .ok_or_else(|| Error::Policy("recorded EVM owner is not a Safe".into()))?;
    if live.0 != binding.threshold || live.1 != binding.nonce {
        return Err(Error::Conflict(
            "live Safe threshold or nonce differs from proposal".into(),
        ));
    }
    let digest = alloy::primitives::B256::from_str(&binding.safe_tx_hash)
        .map_err(|error| Error::InvalidInput(format!("invalid Safe transaction hash: {error}")))?;
    let mut owners = std::collections::BTreeSet::new();
    for confirmation in &evidence.confirmations {
        let claimed = crate::evm::parse_address(&confirmation.owner)?;
        let signature = alloy::primitives::Signature::from_str(&confirmation.signature)
            .map_err(|error| Error::InvalidInput(format!("invalid Safe signature: {error}")))?;
        let recovered = signature
            .recover_address_from_prehash(&digest)
            .map_err(|error| Error::Policy(format!("Safe signature recovery failed: {error}")))?;
        if recovered != claimed {
            return Err(Error::Policy(format!(
                "Safe signature does not belong to {}",
                confirmation.owner
            )));
        }
        let result = crate::block_on_result(
            evm.call(safe, Safe::isOwnerCall { owner: claimed }.abi_encode()),
        )?;
        if result.len() != 32 || result[31] != 1 {
            return Err(Error::Policy(format!(
                "{} is not a live Safe owner",
                confirmation.owner
            )));
        }
        owners.insert(claimed);
    }
    if owners.len() < binding.threshold as usize {
        return Err(Error::Policy(format!(
            "Safe threshold not met: {} of {} owners",
            owners.len(),
            binding.threshold
        )));
    }
    Ok(CommandData {
        result: serde_json::json!({
            "verified": true,
            "safe": safe.to_string(),
            "safe_tx_hash": binding.safe_tx_hash,
            "confirmed_owners": owners.len(),
            "required_threshold": binding.threshold,
        }),
        artifact: None,
    })
}
