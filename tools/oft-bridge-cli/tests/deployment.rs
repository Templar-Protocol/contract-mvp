//! Focused deployment/adoption lifecycle tests: deterministic proofs bind
//! both peers exactly, artifact-lock binding fails closed on any drift,
//! readback requires the pinned digest, recorded state must match exactly,
//! mainnet mutation is refused, and resume proceeds from the first
//! unsatisfied node with hard conflicts on differing code/owner/asset and
//! nonce/address drift. No placeholder or fabricated values on any reachable
//! path.

use std::{collections::BTreeMap, path::PathBuf};

use alloy::primitives::{Address, B256};
use templar_oft_bridge_cli::{
    artifacts::embedded_lock,
    deployment::{
        adoption_verdict, apply_runtime_readback, deployment_node_plan, deployment_proof,
        require_resumable, DeploymentNodeStatus, DeploymentObservationsV1,
        EvmDeploymentObservationsV1, StellarDeploymentObservationsV1,
    },
    domain::{
        AssetKind, AssetPolicyV1, ChainIdentityV1, DesiredRouteV1, Environment, OperationV1,
        RouteStateV1, SCHEMA_VERSION,
    },
    error::{Error, Result},
    evm::{parse_address, DeployEvmOftBindingV1, EvmChain, EvmSimulationV1},
    wrap::plan_wrap,
};

const TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";
const MAINNET_PASSPHRASE: &str = "Public Global Stellar Network ; September 2015";
const OPERATOR: &str = "GCLQ3APIE5AS4XJUTRP5AF7ZMQAXDNGIRMIF3MIWQPF6ZPFJVNJDCN5E";
const EVM_OWNER: &str = "0xc61B17BF20b4B16bb70C1942CD8D9eBDe6726386";
const DESIRED_DIGEST: &str = "desired-hash";
const RUNTIME_CODE: &[u8] = b"disposable-oft-runtime-bytecode-fixture";

fn testnet_identity() -> ChainIdentityV1 {
    ChainIdentityV1 {
        environment: Environment::StellarTestnetSepolia,
        stellar_passphrase: TESTNET_PASSPHRASE.into(),
        stellar_eid: 40600,
        stellar_endpoint: "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV".into(),
        stellar_endpoint_code_hash: "0".repeat(64),
        evm_chain_id: 11_155_111,
        evm_eid: 40161,
        evm_endpoint: "0x6EDCE65403992e310A62460808c4b910D972f10f".into(),
        evm_endpoint_code_hash: "0".repeat(64),
    }
}

fn desired(identity: ChainIdentityV1) -> DesiredRouteV1 {
    DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: SCHEMA_VERSION,
        route_id: "route-deployment-test".into(),
        identity,
        asset: AssetPolicyV1 {
            kind: AssetKind::NativeSac,
            asset_id: "native".into(),
            local_decimals: 7,
            issuer_custodian_evidence_sha256: None,
            destination_acceptance_evidence_sha256: None,
            custody_risk_acceptance_sha256: None,
            forbidden_classic_issuer: None,
            evidence: BTreeMap::new(),
        },
        stellar_owner: OPERATOR.into(),
        stellar_delegate: OPERATOR.into(),
        evm_owner: EVM_OWNER.into(),
        evm_delegate: EVM_OWNER.into(),
        config: BTreeMap::new(),
    }
}

fn init_hash_from_lock() -> B256 {
    let lock = embedded_lock().expect("embedded artifact lock");
    B256::from_slice(
        &hex::decode(&lock.evm.creation_bytecode_keccak256).expect("lock creation hash is hex"),
    )
}

fn bind(
    desired: &DesiredRouteV1,
    plan: &templar_oft_bridge_cli::wrap::WrapPlanV1,
    init_code_hash: Option<B256>,
) -> Result<DeployEvmOftBindingV1> {
    DeployEvmOftBindingV1::bind(
        parse_address(&desired.evm_owner)?,
        plan.evm_nonce,
        init_code_hash,
        plan.name.clone(),
        plan.symbol.clone(),
        parse_address(&desired.identity.evm_endpoint)?,
        parse_address(&desired.evm_delegate)?,
    )
}

fn plan(desired: &DesiredRouteV1) -> Result<templar_oft_bridge_cli::wrap::WrapPlanV1> {
    plan_wrap(desired, DESIRED_DIGEST, "Wrapped XLM", "wXLM", 7, false)
}

fn route_state(desired: &DesiredRouteV1, contracts: BTreeMap<String, String>) -> RouteStateV1 {
    RouteStateV1 {
        schema_name: "route_state".into(),
        schema_version: SCHEMA_VERSION,
        route_id: desired.route_id.clone(),
        desired_sha256: DESIRED_DIGEST.into(),
        identity: desired.identity.clone(),
        asset: desired.asset.clone(),
        opening_custody: None,
        operations_log: PathBuf::from("operations.jsonl"),
        messages_log: PathBuf::from("messages.jsonl"),
        lock_file: PathBuf::from(".lock"),
        contracts,
        requested_config: desired.config.clone(),
        effective_config: BTreeMap::new(),
    }
}

struct FakeEvm {
    code: Vec<u8>,
}

#[async_trait::async_trait]
impl EvmChain for FakeEvm {
    async fn chain_id(&self) -> Result<u64> {
        Ok(11_155_111)
    }
    async fn code(&self, _address: Address) -> Result<Vec<u8>> {
        Ok(self.code.clone())
    }
    async fn call(&self, _to: Address, _calldata: Vec<u8>) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }
    async fn endpoint_eid(&self, _endpoint: &str) -> Result<u32> {
        Ok(40_161)
    }
    async fn account_nonce(&self, _address: Address) -> Result<u64> {
        Ok(7)
    }
    async fn safe_state(&self, _safe: Address) -> Result<Option<(u32, String)>> {
        Ok(None)
    }
    async fn estimate_transaction(
        &self,
        _from: Address,
        _to: Address,
        _value: alloy::primitives::U256,
        _calldata: Vec<u8>,
    ) -> Result<EvmSimulationV1> {
        Ok(EvmSimulationV1 {
            gas_limit: 0,
            max_fee_per_gas_wei: "0".into(),
            max_priority_fee_per_gas_wei: "0".into(),
        })
    }
    async fn send_raw_transaction(&self, _encoded: &[u8]) -> Result<String> {
        Ok("0xtransaction".into())
    }
    async fn transaction_receipt(
        &self,
        _transaction_hash: &str,
    ) -> Result<Option<templar_oft_bridge_cli::evm::EvmReceiptV1>> {
        Ok(None)
    }
    async fn transaction_by_hash(
        &self,
        _transaction_hash: &str,
    ) -> Result<Option<serde_json::Value>> {
        Ok(None)
    }
}

#[test]
fn deployment_proof_binds_both_peers_and_the_artifact_lock() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let proof = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)?;
    assert_eq!(proof.route_id, desired.route_id);
    assert_eq!(proof.desired_sha256, DESIRED_DIGEST);
    assert_eq!(proof.stellar.derived_address, plan.stellar_oft);
    assert_eq!(
        proof.stellar.wasm_sha256,
        embedded_lock()?.stellar.oft_wasm_sha256
    );
    assert_eq!(proof.evm.derived_address, plan.evm_oft);
    assert_eq!(
        proof.evm.init_code_hash.as_deref(),
        Some(embedded_lock()?.evm.creation_bytecode_keccak256.as_str())
    );
    assert_eq!(proof.evm.runtime_code_hash, None);
    assert_eq!(
        proof.artifact_lock_sha256,
        templar_oft_bridge_cli::artifacts::lock_sha256()?
    );
    Ok(())
}

#[test]
fn full_lifecycle_adopts_only_after_exact_readback() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut proven = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)?;

    // No readback yet: adoption must fail closed.
    let fresh = route_state(&desired, BTreeMap::new());
    let error = adoption_verdict(&desired.identity, &plan, &proven, &fresh)
        .expect_err("adoption without readback must fail");
    assert!(matches!(error, Error::Custody(_)));

    // Exact readback binds the runtime hash.
    let expected = hex::encode(templar_oft_bridge_cli::evm::keccak256_of(RUNTIME_CODE));
    let hash = apply_runtime_readback(
        &FakeEvm {
            code: RUNTIME_CODE.to_vec(),
        },
        &mut proven.evm,
        &expected,
    )?;
    assert_eq!(hash, expected);
    assert_eq!(
        proven.evm.runtime_code_hash.as_deref(),
        Some(expected.as_str())
    );

    // Fresh state: adoption is required but not yet satisfied.
    let verdict = adoption_verdict(&desired.identity, &plan, &proven, &fresh)?;
    assert!(!verdict.already_satisfied);
    assert_eq!(verdict.evm_oft, plan.evm_oft);
    assert_eq!(verdict.artifact_lock_sha256, proven.artifact_lock_sha256);

    // Rerun over state that already records both exact deployments is a no-op.
    let recorded = route_state(
        &desired,
        BTreeMap::from([
            ("stellar_oft".into(), plan.stellar_oft.clone()),
            ("evm_oft".into(), plan.evm_oft.clone()),
        ]),
    );
    let rerun = adoption_verdict(&desired.identity, &plan, &proven, &recorded)?;
    assert!(rerun.already_satisfied);
    Ok(())
}

#[test]
fn evm_binding_without_init_code_hash_fails_closed() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, None)?;
    let error = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)
        .expect_err("unbuilt wrapper must fail closed");
    assert!(matches!(error, Error::Custody(_)));
    Ok(())
}

#[test]
fn evm_init_code_hash_drift_is_custody_failure() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(B256::ZERO))?;
    let error = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)
        .expect_err("init code drift must fail");
    assert!(matches!(error, Error::Custody(_)));
    Ok(())
}

#[test]
fn evm_nonce_drift_is_hard_conflict() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let mut binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    binding.nonce = plan.evm_nonce + 1;
    let error = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)
        .expect_err("nonce drift must be a conflict");
    assert!(matches!(error, Error::Conflict(_)));
    Ok(())
}

#[test]
fn evm_derived_address_drift_is_hard_conflict() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let mut binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    binding.derived_address = "0x0000000000000000000000000000000000000001".into();
    let error = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)
        .expect_err("derived address drift must be a conflict");
    assert!(matches!(error, Error::Conflict(_)));
    Ok(())
}

#[test]
fn evm_constructor_arguments_must_match_the_plan() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let mut binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    binding.name = "Other Wrapper".into();
    let error = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)
        .expect_err("wrapper name drift must be a conflict");
    assert!(matches!(error, Error::Conflict(_)));
    Ok(())
}

#[test]
fn stellar_salt_drift_is_hard_conflict() -> Result<()> {
    let desired = desired(testnet_identity());
    let mut plan = plan(&desired)?;
    plan.stellar_salt = hex::encode([0u8; 32]);
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let error = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)
        .expect_err("salt drift must be a conflict");
    assert!(matches!(error, Error::Conflict(_)));
    Ok(())
}

#[test]
fn stellar_address_drift_is_hard_conflict() -> Result<()> {
    let desired = desired(testnet_identity());
    let mut plan = plan(&desired)?;
    plan.stellar_oft = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".into();
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let error = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)
        .expect_err("derived stellar drift must be a conflict");
    assert!(matches!(error, Error::Conflict(_)));
    Ok(())
}

#[test]
fn stellar_wasm_install_must_match_the_artifact_lock() -> Result<()> {
    let desired = desired(testnet_identity());
    let mut plan = plan(&desired)?;
    plan.operations[0] = OperationV1::InstallStellarWasm {
        wasm_sha256: "0".repeat(64),
    };
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let error = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)
        .expect_err("wasm drift must be a custody failure");
    assert!(matches!(error, Error::Custody(_)));
    Ok(())
}

#[test]
fn runtime_readback_refuses_code_hash_drift() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut proven = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)?;
    let error = apply_runtime_readback(
        &FakeEvm {
            code: RUNTIME_CODE.to_vec(),
        },
        &mut proven.evm,
        &"0".repeat(64),
    )
    .expect_err("hash drift must fail closed");
    assert!(matches!(error, Error::Custody(_)));
    assert_eq!(proven.evm.runtime_code_hash, None);
    Ok(())
}

#[test]
fn runtime_readback_refuses_missing_code() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut proven = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)?;
    let expected = hex::encode(templar_oft_bridge_cli::evm::keccak256_of(RUNTIME_CODE));
    let error = apply_runtime_readback(&FakeEvm { code: Vec::new() }, &mut proven.evm, &expected)
        .expect_err("absent code must be a chain refusal");
    assert!(matches!(error, Error::Chain(_)));
    Ok(())
}

#[test]
fn adoption_rejects_state_with_a_differing_stellar_deployment() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut proven = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)?;
    let expected = hex::encode(templar_oft_bridge_cli::evm::keccak256_of(RUNTIME_CODE));
    apply_runtime_readback(
        &FakeEvm {
            code: RUNTIME_CODE.to_vec(),
        },
        &mut proven.evm,
        &expected,
    )?;
    let drifted = route_state(
        &desired,
        BTreeMap::from([(
            "stellar_oft".into(),
            "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC".into(),
        )]),
    );
    let error = adoption_verdict(&desired.identity, &plan, &proven, &drifted)
        .expect_err("recorded stellar drift must be a conflict");
    assert!(matches!(error, Error::Conflict(_)));
    Ok(())
}

#[test]
fn adoption_rejects_opening_custody_lock_drift() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut proven = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)?;
    let expected = hex::encode(templar_oft_bridge_cli::evm::keccak256_of(RUNTIME_CODE));
    apply_runtime_readback(
        &FakeEvm {
            code: RUNTIME_CODE.to_vec(),
        },
        &mut proven.evm,
        &expected,
    )?;
    let mut state = route_state(&desired, BTreeMap::new());
    state.opening_custody = Some(templar_oft_bridge_cli::domain::OpeningCustodyV1 {
        schema_name: "opening_custody".into(),
        schema_version: SCHEMA_VERSION,
        stellar_ledger: 1,
        stellar_ledger_hash: "0".repeat(64),
        stellar_ledger_time_unix: 1,
        lockbox_raw: 0,
        evm_block: 1,
        evm_block_hash: "0".repeat(64),
        evm_supply_raw: 0,
        artifact_lock_sha256: "0".repeat(64),
        effective_config_sha256: "0".repeat(64),
        zero_packet_history_proven: false,
        history_evidence_sha256: None,
    });
    let error = adoption_verdict(&desired.identity, &plan, &proven, &state)
        .expect_err("opening custody lock drift must be a custody failure");
    assert!(matches!(error, Error::Custody(_)));
    Ok(())
}

#[test]
fn mainnet_adoption_requires_exact_runtime_readback() -> Result<()> {
    let mainnet_identity = ChainIdentityV1 {
        environment: Environment::StellarMainnetEthereum,
        stellar_passphrase: MAINNET_PASSPHRASE.into(),
        stellar_eid: 30600,
        stellar_endpoint: "CCQLLRE5JBAWYCW3KTWOIWLMFDUOKROQVZNSALQMGOSXNW3ERUOWTZGK".into(),
        stellar_endpoint_code_hash: "0".repeat(64),
        evm_chain_id: 1,
        evm_eid: 30101,
        evm_endpoint: templar_oft_bridge_cli::environment::ETHEREUM_ENDPOINT.into(),
        evm_endpoint_code_hash: "0".repeat(64),
    };
    let desired = desired(mainnet_identity);
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut proven = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)?;
    let state = route_state(&desired, BTreeMap::new());
    let error = adoption_verdict(&desired.identity, &plan, &proven, &state)
        .expect_err("mainnet adoption without readback must fail closed");
    assert!(matches!(error, Error::Custody(_)));

    let expected = hex::encode(templar_oft_bridge_cli::evm::keccak256_of(RUNTIME_CODE));
    apply_runtime_readback(
        &FakeEvm {
            code: RUNTIME_CODE.to_vec(),
        },
        &mut proven.evm,
        &expected,
    )?;
    let verdict = adoption_verdict(&desired.identity, &plan, &proven, &state)?;
    assert_eq!(verdict.environment, Environment::StellarMainnetEthereum);
    assert!(!verdict.already_satisfied);
    Ok(())
}

const OTHER_STELLAR_OFT: &str = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC";
const OTHER_OPERATOR: &str = "GBJQUQZ5O5J7RQ7IZBHDIQ2CVFH3QQIAGZQKQNNVAXFKQNAD6IRWSC";

fn exact_observations(
    desired: &DesiredRouteV1,
    plan: &templar_oft_bridge_cli::wrap::WrapPlanV1,
) -> Result<DeploymentObservationsV1> {
    let lock = embedded_lock()?;
    Ok(DeploymentObservationsV1 {
        stellar: StellarDeploymentObservationsV1 {
            installed_wasm_sha256: Some(lock.stellar.oft_wasm_sha256.clone()),
            oft_address: Some(plan.stellar_oft.clone()),
            oft_code_sha256: Some(lock.stellar.oft_wasm_sha256.clone()),
            owner: Some(desired.stellar_delegate.clone()),
            delegate: Some(desired.stellar_delegate.clone()),
            token_contract: Some(plan.stellar_token_contract.clone()),
            decimals: Some(6),
        },
        evm: EvmDeploymentObservationsV1 {
            deployer_nonce: plan.evm_nonce,
            oft_address: Some(plan.evm_oft.clone()),
            runtime_code_sha256: Some(lock.evm.runtime_bytecode_keccak256.clone()),
            owner: Some(desired.evm_delegate.clone()),
            delegate: Some(desired.evm_delegate.clone()),
            name: Some(plan.name.clone()),
            symbol: Some(plan.symbol.clone()),
            decimals: Some(6),
            endpoint: Some(desired.identity.evm_endpoint.clone()),
        },
    })
}

fn empty_observations(
    plan: &templar_oft_bridge_cli::wrap::WrapPlanV1,
) -> Result<DeploymentObservationsV1> {
    Ok(DeploymentObservationsV1 {
        stellar: StellarDeploymentObservationsV1 {
            installed_wasm_sha256: None,
            oft_address: None,
            oft_code_sha256: None,
            owner: None,
            delegate: None,
            token_contract: None,
            decimals: None,
        },
        evm: EvmDeploymentObservationsV1 {
            deployer_nonce: plan.evm_nonce,
            oft_address: None,
            runtime_code_sha256: None,
            owner: None,
            delegate: None,
            name: None,
            symbol: None,
            decimals: None,
            endpoint: None,
        },
    })
}

#[test]
fn resume_from_first_unsatisfied_node_after_partial_failure() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    // Install succeeded; neither OFT is deployed yet.
    let mut observations = empty_observations(&plan)?;
    observations.stellar.installed_wasm_sha256 = Some(embedded_lock()?.stellar.oft_wasm_sha256);
    let node_plan = deployment_node_plan(&desired, DESIRED_DIGEST, &plan, &binding, &observations)?;
    assert_eq!(node_plan.nodes[0].status, DeploymentNodeStatus::Satisfied);
    assert_eq!(node_plan.nodes[1].status, DeploymentNodeStatus::Pending);
    assert_eq!(node_plan.nodes[2].status, DeploymentNodeStatus::Pending);
    assert_eq!(node_plan.first_unsatisfied, Some(1));
    assert!(node_plan.conflicts.is_empty());
    assert!(!node_plan.converged);
    assert_eq!(require_resumable(&node_plan)?, Some(1));
    assert_eq!(node_plan.nodes[1].operation, plan.operations[1]);
    Ok(())
}

#[test]
fn resume_skips_satisfied_deployments_and_targets_only_evm() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    // Stellar side fully deployed; EVM absent with the reservation intact.
    let mut observations = exact_observations(&desired, &plan)?;
    observations.evm = EvmDeploymentObservationsV1 {
        deployer_nonce: plan.evm_nonce,
        oft_address: None,
        runtime_code_sha256: None,
        owner: None,
        delegate: None,
        name: None,
        symbol: None,
        decimals: None,
        endpoint: None,
    };
    let node_plan = deployment_node_plan(&desired, DESIRED_DIGEST, &plan, &binding, &observations)?;
    assert_eq!(node_plan.nodes[0].status, DeploymentNodeStatus::Satisfied);
    assert_eq!(node_plan.nodes[1].status, DeploymentNodeStatus::Satisfied);
    assert_eq!(node_plan.nodes[2].status, DeploymentNodeStatus::Pending);
    assert_eq!(node_plan.first_unsatisfied, Some(2));
    assert_eq!(require_resumable(&node_plan)?, Some(2));
    assert_eq!(node_plan.nodes[2].operation, plan.operations[2]);
    Ok(())
}

#[test]
fn exact_adoption_is_an_idempotent_noop_with_bound_nonce_and_address() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let observations = exact_observations(&desired, &plan)?;
    let node_plan = deployment_node_plan(&desired, DESIRED_DIGEST, &plan, &binding, &observations)?;
    assert!(node_plan.converged);
    assert_eq!(node_plan.first_unsatisfied, None);
    assert!(node_plan.conflicts.is_empty());
    assert_eq!(require_resumable(&node_plan)?, None);
    // Every node carries the exact plan operation and binds the derived peer.
    assert_eq!(node_plan.stellar_oft, plan.stellar_oft);
    assert_eq!(node_plan.evm_oft, plan.evm_oft);
    assert_eq!(
        node_plan.nodes[0].operation,
        OperationV1::InstallStellarWasm {
            wasm_sha256: embedded_lock()?.stellar.oft_wasm_sha256,
        }
    );
    assert_eq!(node_plan.nodes[2].operation, plan.operations[2]);
    // A rerun over recorded exact deployments is an adoption no-op.
    let mut proven = deployment_proof(&desired, DESIRED_DIGEST, &plan, &binding)?;
    let expected = hex::encode(templar_oft_bridge_cli::evm::keccak256_of(RUNTIME_CODE));
    apply_runtime_readback(
        &FakeEvm {
            code: RUNTIME_CODE.to_vec(),
        },
        &mut proven.evm,
        &expected,
    )?;
    let recorded = route_state(
        &desired,
        BTreeMap::from([
            ("stellar_oft".into(), plan.stellar_oft.clone()),
            ("evm_oft".into(), plan.evm_oft.clone()),
        ]),
    );
    let verdict = adoption_verdict(&desired.identity, &plan, &proven, &recorded)?;
    assert!(verdict.already_satisfied);
    Ok(())
}

#[test]
fn differing_stellar_code_hash_is_hard_conflict() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut observations = exact_observations(&desired, &plan)?;
    observations.stellar.oft_code_sha256 = Some("0".repeat(64));
    let node_plan = deployment_node_plan(&desired, DESIRED_DIGEST, &plan, &binding, &observations)?;
    assert_eq!(node_plan.nodes[1].status, DeploymentNodeStatus::Conflicting);
    assert!(!node_plan.converged);
    let error = require_resumable(&node_plan).expect_err("differing code must refuse resume");
    assert!(matches!(error, Error::Conflict(_)));
    assert!(node_plan
        .conflicts
        .iter()
        .any(|reason| reason.contains("code hash")));
    Ok(())
}

#[test]
fn differing_stellar_owner_is_hard_conflict() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut observations = exact_observations(&desired, &plan)?;
    observations.stellar.owner = Some(OTHER_OPERATOR.into());
    let node_plan = deployment_node_plan(&desired, DESIRED_DIGEST, &plan, &binding, &observations)?;
    assert_eq!(node_plan.nodes[1].status, DeploymentNodeStatus::Conflicting);
    let error = require_resumable(&node_plan).expect_err("differing owner must refuse resume");
    assert!(matches!(error, Error::Conflict(_)));
    assert!(node_plan
        .conflicts
        .iter()
        .any(|reason| reason.contains("owner/delegate")));
    Ok(())
}

#[test]
fn differing_stellar_token_asset_is_hard_conflict() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut observations = exact_observations(&desired, &plan)?;
    observations.stellar.token_contract = Some(desired.identity.stellar_endpoint.clone());
    let node_plan = deployment_node_plan(&desired, DESIRED_DIGEST, &plan, &binding, &observations)?;
    assert_eq!(node_plan.nodes[1].status, DeploymentNodeStatus::Conflicting);
    let error = require_resumable(&node_plan).expect_err("differing asset must refuse resume");
    assert!(matches!(error, Error::Conflict(_)));
    assert!(node_plan
        .conflicts
        .iter()
        .any(|reason| reason.contains("lockbox token")));
    Ok(())
}

#[test]
fn differing_evm_owner_is_hard_conflict() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut observations = exact_observations(&desired, &plan)?;
    observations.evm.owner = Some("0x0000000000000000000000000000000000000001".into());
    let node_plan = deployment_node_plan(&desired, DESIRED_DIGEST, &plan, &binding, &observations)?;
    assert_eq!(node_plan.nodes[2].status, DeploymentNodeStatus::Conflicting);
    let error = require_resumable(&node_plan).expect_err("differing owner must refuse resume");
    assert!(matches!(error, Error::Conflict(_)));
    assert!(node_plan
        .conflicts
        .iter()
        .any(|reason| reason.contains("owner/delegate")));
    Ok(())
}

#[test]
fn differing_evm_asset_symbol_is_hard_conflict() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut observations = exact_observations(&desired, &plan)?;
    observations.evm.symbol = Some("OTHER".into());
    let node_plan = deployment_node_plan(&desired, DESIRED_DIGEST, &plan, &binding, &observations)?;
    assert_eq!(node_plan.nodes[2].status, DeploymentNodeStatus::Conflicting);
    let error = require_resumable(&node_plan).expect_err("differing asset must refuse resume");
    assert!(matches!(error, Error::Conflict(_)));
    assert!(node_plan
        .conflicts
        .iter()
        .any(|reason| reason.contains("symbol")));
    Ok(())
}

#[test]
fn unrelated_nonce_consumption_invalidates_plan_before_signing() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    // Deployment absent and the reserved nonce was consumed elsewhere.
    let mut observations = empty_observations(&plan)?;
    observations.stellar.installed_wasm_sha256 = Some(embedded_lock()?.stellar.oft_wasm_sha256);
    observations.evm.deployer_nonce = plan.evm_nonce + 1;
    let node_plan = deployment_node_plan(&desired, DESIRED_DIGEST, &plan, &binding, &observations)?;
    assert_eq!(node_plan.nodes[2].status, DeploymentNodeStatus::Conflicting);
    let error = require_resumable(&node_plan).expect_err("nonce drift must refuse resume");
    match error {
        Error::Conflict(message) => {
            assert!(message.contains("nonce"), "unexpected message: {message}")
        }
        other => panic!("unexpected error: {other:?}"),
    }
    Ok(())
}

#[test]
fn readback_at_differing_stellar_address_is_hard_conflict() -> Result<()> {
    let desired = desired(testnet_identity());
    let plan = plan(&desired)?;
    let binding = bind(&desired, &plan, Some(init_hash_from_lock()))?;
    let mut observations = exact_observations(&desired, &plan)?;
    observations.stellar.oft_address = Some(OTHER_STELLAR_OFT.into());
    let node_plan = deployment_node_plan(&desired, DESIRED_DIGEST, &plan, &binding, &observations)?;
    assert_eq!(node_plan.nodes[1].status, DeploymentNodeStatus::Conflicting);
    let error = require_resumable(&node_plan).expect_err("differing address must refuse resume");
    assert!(matches!(error, Error::Conflict(_)));
    assert!(node_plan
        .conflicts
        .iter()
        .any(|reason| reason.contains("readback address")));
    Ok(())
}
