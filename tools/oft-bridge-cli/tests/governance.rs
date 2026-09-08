//! Focused fake-adapter governance tests: live adapter-derived simulation
//! values bind into executable plans, and every mismatch or adapter refusal
//! fails closed with a typed error. No offline marker values are accepted
//! on reachable proposal paths.

use alloy::primitives::{keccak256, Address, U256};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};

use templar_oft_bridge_cli::domain::{
    AssetKind, AssetPolicyV1, ChainIdentityV1, Direction, Environment, LegIntentV1, OperationV1,
    RouteStateV1, Vm, SCHEMA_VERSION,
};
use templar_oft_bridge_cli::error::{Error, Result};
use templar_oft_bridge_cli::evm::{EvmChain, EvmSimulationV1};
use templar_oft_bridge_cli::governance::{
    build_executable_plan, build_proposal, ingest_proposal_with_adapters,
};
use templar_oft_bridge_cli::stellar::{StellarChain, StellarSimulationV1};

const PASSPHRASE: &str = "Test SDF Network ; September 2015";
const STELLAR_OWNER: &str = "GOWNER";
const EVM_OWNER: &str = "0x1111111111111111111111111111111111111111";
const EVM_OFT: &str = "0x2222222222222222222222222222222222222222";
const ENVELOPE_BYTES: &[u8] = b"assembled-broadcast-exact-envelope";
const ENVELOPE_HASH: &str = "97f106f784c9bbe7fb5f1906c1aeb40a9781661312953701ac4fc6e3c207b434";
const GAS_LIMIT: u64 = 184_000;
const MAX_FEE: &str = "3000000001";
const MAX_PRIORITY_FEE: &str = "1000000001";

fn selector(signature: &str) -> Vec<u8> {
    keccak256(signature.as_bytes())[..4].to_vec()
}

fn word_u128(value: u128) -> Vec<u8> {
    let mut word = vec![0u8; 16];
    word.extend_from_slice(&value.to_be_bytes());
    word
}

struct FakeStellar {
    passphrase: String,
    simulation: Option<StellarSimulationV1>,
}

impl FakeStellar {
    fn qualified() -> Self {
        use base64::Engine as _;
        Self {
            passphrase: PASSPHRASE.into(),
            simulation: Some(StellarSimulationV1 {
                envelope_xdr: base64::engine::general_purpose::STANDARD.encode(ENVELOPE_BYTES),
                envelope_sha256: ENVELOPE_HASH.into(),
                auth_entries: vec!["source-account-auth-entry".into()],
                simulation_ledger: 4_310,
            }),
        }
    }
}

impl StellarChain for FakeStellar {
    fn network_passphrase(&self) -> Result<String> {
        Ok(self.passphrase.clone())
    }
    fn endpoint_eid(&self, _endpoint: &str, _source: &str) -> Result<u32> {
        Ok(40600)
    }
    fn account_sequence(&self, _account: &str) -> Result<String> {
        Ok("41".into())
    }
    fn invoke_view(
        &self,
        _contract: &str,
        _function: &str,
        _args_xdr_hex: &[String],
        _source: &str,
    ) -> Result<stellar_baselib::xdr::ScVal> {
        Ok(stellar_baselib::xdr::ScVal::U32(0))
    }
    fn token_balance(&self, _token: &str, _address: &str, _source: &str) -> Result<String> {
        Ok("0".into())
    }
    fn account_signers(&self, _account: &str) -> Result<BTreeMap<String, u32>> {
        Ok(BTreeMap::from([(STELLAR_OWNER.into(), 3)]))
    }
    fn account_threshold(&self, _account: &str, _level: &str) -> Result<u32> {
        Ok(2)
    }
    fn latest_ledger(&self) -> Result<u32> {
        Ok(4_310)
    }
    fn simulate_transaction(
        &self,
        _state: &RouteStateV1,
        _operation: &OperationV1,
        _source: &str,
        _sequence: &str,
        _min_ledger: u32,
        _max_ledger: u32,
    ) -> Result<StellarSimulationV1> {
        self.simulation.clone().ok_or_else(|| {
            Error::Chain("stellar simulation refused by the qualified adapter".into())
        })
    }
    fn submit_transaction(&self, _signed_envelope_xdr: &str) -> Result<String> {
        Ok("stellar-tx".into())
    }
    fn transaction_status(
        &self,
        _transaction_hash: &str,
    ) -> Result<templar_oft_bridge_cli::stellar::StellarTransactionStatusV1> {
        Ok(
            templar_oft_bridge_cli::stellar::StellarTransactionStatusV1 {
                status: "success".into(),
                ledger: Some(4_311),
                envelope_xdr: None,
                contract_events: Vec::new(),
            },
        )
    }
}

struct FakeEvm {
    chain_id: u64,
    estimate: Option<EvmSimulationV1>,
    digest_word: [u8; 32],
    fail_safe_view: bool,
    calls: Mutex<Vec<Vec<u8>>>,
    transaction: Option<serde_json::Value>,
    receipt: Option<templar_oft_bridge_cli::evm::EvmReceiptV1>,
}

impl FakeEvm {
    fn qualified() -> Self {
        Self {
            chain_id: 11_155_111,
            estimate: Some(EvmSimulationV1 {
                gas_limit: GAS_LIMIT,
                max_fee_per_gas_wei: MAX_FEE.into(),
                max_priority_fee_per_gas_wei: MAX_PRIORITY_FEE.into(),
            }),
            digest_word: [0xAB; 32],
            fail_safe_view: false,
            calls: Mutex::new(Vec::new()),
            transaction: None,
            receipt: None,
        }
    }
}

#[async_trait::async_trait]
impl EvmChain for FakeEvm {
    async fn chain_id(&self) -> Result<u64> {
        Ok(self.chain_id)
    }
    async fn code(&self, _address: Address) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }
    async fn call(&self, _to: Address, calldata: Vec<u8>) -> Result<Vec<u8>> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(calldata.clone());
        let prefix = calldata.get(..4).unwrap_or_default();
        if prefix == selector("getThreshold()").as_slice() {
            return Ok(word_u128(2));
        }
        if prefix == selector("nonce()").as_slice() {
            return Ok(word_u128(9));
        }
        if prefix == selector(
            "getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256)",
        )
        .as_slice()
        {
            if self.fail_safe_view {
                return Err(Error::Chain("safe view call reverted".into()));
            }
            return Ok(self.digest_word.to_vec());
        }
        if prefix == selector("peers(uint32)").as_slice() {
            return Ok(vec![0x2b; 32]);
        }
        Err(Error::Chain(
            "unexpected view call on fake EVM adapter".into(),
        ))
    }
    async fn endpoint_eid(&self, _endpoint: &str) -> Result<u32> {
        Ok(40161)
    }
    async fn account_nonce(&self, _address: Address) -> Result<u64> {
        Ok(7)
    }
    async fn safe_state(&self, _safe: Address) -> Result<Option<(u32, String)>> {
        Ok(Some((2, "9".into())))
    }
    async fn estimate_transaction(
        &self,
        _from: Address,
        _to: Address,
        _value: U256,
        _calldata: Vec<u8>,
    ) -> Result<EvmSimulationV1> {
        self.estimate
            .clone()
            .ok_or_else(|| Error::Chain("evm estimation refused by the qualified adapter".into()))
    }
    async fn send_raw_transaction(&self, _encoded: &[u8]) -> Result<String> {
        Ok("0xtransaction".into())
    }
    async fn transaction_receipt(
        &self,
        _transaction_hash: &str,
    ) -> Result<Option<templar_oft_bridge_cli::evm::EvmReceiptV1>> {
        Ok(self.receipt.clone())
    }
    async fn transaction_by_hash(
        &self,
        _transaction_hash: &str,
    ) -> Result<Option<serde_json::Value>> {
        Ok(self.transaction.clone())
    }
}

fn route_state() -> RouteStateV1 {
    RouteStateV1 {
        schema_name: "route_state".into(),
        schema_version: SCHEMA_VERSION,
        route_id: "route-governance".into(),
        desired_sha256: "desired-digest".into(),
        identity: ChainIdentityV1 {
            environment: Environment::StellarTestnetSepolia,
            stellar_passphrase: PASSPHRASE.into(),
            stellar_eid: 40600,
            stellar_endpoint: templar_oft_bridge_cli::environment::STELLAR_TESTNET_ENDPOINT.into(),
            stellar_endpoint_code_hash: "endpoint-code-hash".into(),
            evm_chain_id: 11_155_111,
            evm_eid: 40161,
            evm_endpoint: templar_oft_bridge_cli::environment::SEPOLIA_ENDPOINT.into(),
            evm_endpoint_code_hash: "endpoint-code-hash".into(),
        },
        asset: AssetPolicyV1 {
            kind: AssetKind::IssuedSep41,
            asset_id: "CASSETASSETASSETASSETASSETASSETASSETASSET".into(),
            local_decimals: 7,
            issuer_custodian_evidence_sha256: None,
            destination_acceptance_evidence_sha256: None,
            custody_risk_acceptance_sha256: None,
            forbidden_classic_issuer: None,
            evidence: BTreeMap::new(),
        },
        opening_custody: None,
        operations_log: PathBuf::from("operations.jsonl"),
        messages_log: PathBuf::from("messages.jsonl"),
        lock_file: PathBuf::from(".lock"),
        contracts: BTreeMap::from([
            ("stellar_owner".into(), STELLAR_OWNER.into()),
            ("stellar_oft".into(), "COFT".into()),
            ("evm_owner".into(), EVM_OWNER.into()),
            ("evm_oft".into(), EVM_OFT.into()),
        ]),
        requested_config: BTreeMap::new(),
        effective_config: BTreeMap::new(),
    }
}

fn stellar_operation() -> OperationV1 {
    OperationV1::SetStellarPeer {
        remote_eid: 40161,
        peer: format!("0x{}", "1a".repeat(32)),
    }
}

fn evm_operation() -> OperationV1 {
    OperationV1::SetEvmPeer {
        remote_eid: 40600,
        peer: format!("0x{}", "2b".repeat(32)),
    }
}

#[test]
fn stellar_plan_binds_live_simulation_values() {
    let stellar = FakeStellar::qualified();
    let evm = FakeEvm::qualified();
    let plan =
        build_executable_plan(&route_state(), &stellar_operation(), &stellar, &evm).expect("plan");
    let binding = plan.stellar.as_ref().expect("stellar binding");
    let simulation = stellar.simulation.expect("fixture simulation");
    assert_eq!(binding.envelope_xdr, simulation.envelope_xdr);
    assert_eq!(binding.auth_entries, simulation.auth_entries);
    assert_eq!(binding.simulation_ledger, simulation.simulation_ledger);
    assert_eq!(binding.envelope_sha256, simulation.envelope_sha256);
    assert_eq!(binding.sequence, "41");
    assert_eq!(binding.min_ledger, 4_310);
    assert_eq!(binding.max_ledger, 4_310 + 1_000);
    // The plan digest binds the constructed transaction's canonical bytes.
    assert_eq!(
        plan.simulation_sha256,
        hex::encode(sha2::Sha256::digest(ENVELOPE_BYTES))
    );
    // The qualified plan is proposal-eligible.
    build_proposal(Environment::StellarTestnetSepolia, plan).expect("proposal");
}

#[test]
fn evm_plan_binds_live_estimate_and_safe_digest() {
    let stellar = FakeStellar::qualified();
    let evm = FakeEvm::qualified();
    let plan =
        build_executable_plan(&route_state(), &evm_operation(), &stellar, &evm).expect("plan");
    let binding = plan.evm.as_ref().expect("evm binding");
    assert_eq!(binding.gas_limit, GAS_LIMIT.to_string());
    assert_eq!(binding.max_fee_per_gas_wei, MAX_FEE);
    assert_eq!(binding.max_priority_fee_per_gas_wei, MAX_PRIORITY_FEE);
    assert_eq!(binding.nonce, "7");
    let safe = binding.safe.as_ref().expect("safe binding");
    assert_eq!(safe.nonce, "9");
    assert_eq!(safe.threshold, 2);
    assert_eq!(
        safe.safe_tx_hash,
        format!("0x{}", hex::encode(evm.digest_word))
    );
    // Full 452-byte call pinned against `cast calldata` (Foundry 1.7.1),
    // including dynamic bytes offset/length/padding—not only the selector.
    let calls = evm
        .calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let safe_calldata = calls
        .iter()
        .find(|call| {
            call.starts_with(
                &selector(
                    "getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256)",
                ),
            )
        })
        .expect("Safe digest call");
    assert_eq!(safe_calldata.len(), 452);
    assert_eq!(
        hex::encode(Sha256::digest(safe_calldata)),
        "c43c11408c0d0f7de71aff83055da2f82dde7199568a80567de88d079ec5815d"
    );
    // The digest is computed with its own field empty, then stored into the
    // final binding.
    let mut unsigned = binding.clone();
    unsigned.transaction_digest.clear();
    let expected = hex::encode(keccak256(
        serde_json_canonicalizer::to_vec(&unsigned).expect("canonical bytes"),
    ));
    assert_eq!(binding.transaction_digest, expected);
    assert_eq!(plan.simulation_sha256, expected);
    build_proposal(Environment::StellarTestnetSepolia, plan).expect("proposal");
}

#[test]
fn stellar_passphrase_mismatch_fails_closed() {
    let mut stellar = FakeStellar::qualified();
    stellar.passphrase = "Public Global Stellar Network ; September 2015".into();
    let error = build_executable_plan(
        &route_state(),
        &stellar_operation(),
        &stellar,
        &FakeEvm::qualified(),
    )
    .expect_err("mismatch must fail");
    assert!(matches!(error, Error::Policy(_)));
    assert!(error.to_string().contains("passphrase differs"));
}

#[test]
fn stellar_simulation_refusal_fails_closed() {
    let stellar = FakeStellar {
        passphrase: PASSPHRASE.into(),
        simulation: None,
    };
    let error = build_executable_plan(
        &route_state(),
        &stellar_operation(),
        &stellar,
        &FakeEvm::qualified(),
    )
    .expect_err("refusal must fail");
    assert!(matches!(error, Error::Chain(_)));
}

#[test]
fn evm_chain_id_mismatch_fails_closed() {
    let evm = FakeEvm {
        chain_id: 1,
        ..FakeEvm::qualified()
    };
    let error = build_executable_plan(
        &route_state(),
        &evm_operation(),
        &FakeStellar::qualified(),
        &evm,
    )
    .expect_err("mismatch must fail");
    assert!(matches!(error, Error::Policy(_)));
    assert!(error.to_string().contains("chain id differs"));
}

#[test]
fn evm_estimate_refusal_fails_closed() {
    let evm = FakeEvm {
        estimate: None,
        ..FakeEvm::qualified()
    };
    let error = build_executable_plan(
        &route_state(),
        &evm_operation(),
        &FakeStellar::qualified(),
        &evm,
    )
    .expect_err("refusal must fail");
    assert!(matches!(error, Error::Chain(_)));
}

#[test]
fn safe_digest_view_failure_fails_closed() {
    let evm = FakeEvm {
        fail_safe_view: true,
        ..FakeEvm::qualified()
    };
    let error = build_executable_plan(
        &route_state(),
        &evm_operation(),
        &FakeStellar::qualified(),
        &evm,
    )
    .expect_err("view failure must fail");
    assert!(matches!(error, Error::Chain(_)));
}

#[test]
fn marker_valued_plans_are_rejected() {
    let stellar = FakeStellar::qualified();
    let good_stellar = build_executable_plan(
        &route_state(),
        &stellar_operation(),
        &stellar,
        &FakeEvm::qualified(),
    )
    .expect("plan");

    let mut empty_envelope = good_stellar.clone();
    empty_envelope.stellar.as_mut().unwrap().envelope_xdr = String::new();
    assert!(build_proposal(Environment::StellarTestnetSepolia, empty_envelope).is_err());

    let evm = FakeEvm::qualified();
    let good_evm =
        build_executable_plan(&route_state(), &evm_operation(), &stellar, &evm).expect("plan");

    let mut constant_gas = good_evm.clone();
    constant_gas.evm.as_mut().unwrap().gas_limit = "constant:200000".into();
    assert!(build_proposal(Environment::StellarTestnetSepolia, constant_gas).is_err());

    let mut pending_safe = good_evm.clone();
    pending_safe
        .evm
        .as_mut()
        .unwrap()
        .safe
        .as_mut()
        .unwrap()
        .safe_tx_hash = "pending_qualified_adapter".into();
    assert!(build_proposal(Environment::StellarTestnetSepolia, pending_safe).is_err());

    let mut unbound_digest = good_evm;
    unbound_digest.simulation_sha256 = String::new();
    assert!(build_proposal(Environment::StellarTestnetSepolia, unbound_digest).is_err());
}

#[test]
fn stellar_envelope_digest_mismatch_fails_closed() {
    let mut stellar = FakeStellar::qualified();
    stellar
        .simulation
        .as_mut()
        .expect("qualified simulation")
        .envelope_sha256 = "f".repeat(64);
    let error = build_executable_plan(
        &route_state(),
        &stellar_operation(),
        &stellar,
        &FakeEvm::qualified(),
    )
    .expect_err("adapter digest mismatch must fail");
    assert!(matches!(error, Error::Custody(_)));
}

fn safe_execution_input(safe: &templar_oft_bridge_cli::domain::SafeTransactionV1) -> String {
    use alloy::primitives::{Address, U256};
    use std::str::FromStr as _;

    let mut head = vec![[0u8; 32]; 10];
    let address_word = |value: &str| {
        let mut word = [0u8; 32];
        word[12..].copy_from_slice(Address::from_str(value).unwrap().as_slice());
        word
    };
    let uint_word = |value: &str| U256::from_str(value).unwrap().to_be_bytes::<32>();
    head[0] = address_word(&safe.to);
    head[1] = uint_word(&safe.value);
    head[3][31] = safe.operation;
    head[4] = uint_word(&safe.safe_tx_gas);
    head[5] = uint_word(&safe.base_gas);
    head[6] = uint_word(&safe.gas_price);
    head[7] = address_word(&safe.gas_token);
    head[8] = address_word(&safe.refund_receiver);
    let data = hex::decode(safe.data.trim_start_matches("0x")).unwrap();
    let padded_data = data.len().div_ceil(32) * 32;
    head[2] = U256::from(320).to_be_bytes::<32>();
    head[9] = U256::from(320 + 32 + padded_data).to_be_bytes::<32>();
    let mut encoded = selector("execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)");
    encoded.extend(head.into_iter().flatten());
    encoded.extend(U256::from(data.len()).to_be_bytes::<32>());
    encoded.extend(&data);
    encoded.resize(encoded.len() + padded_data - data.len(), 0);
    encoded.extend(U256::from(65).to_be_bytes::<32>());
    encoded.extend([1u8; 65]);
    format!("0x{}", hex::encode(encoded))
}

#[test]
fn evm_ingest_verifies_exact_finalized_transaction_before_journaling() {
    let temporary = tempfile::tempdir().unwrap();
    let state_path = temporary.path().join("route");
    std::fs::create_dir(&state_path).unwrap();
    let state = route_state();
    templar_oft_bridge_cli::state::write_create_new_json(&state_path.join("route.json"), &state)
        .unwrap();
    std::fs::File::create(state_path.join("operations.jsonl")).unwrap();
    std::fs::File::create(state_path.join("messages.jsonl")).unwrap();
    let store = templar_oft_bridge_cli::state::RouteStore::open(&state_path).unwrap();

    let mut evm = FakeEvm::qualified();
    let plan =
        build_executable_plan(&state, &evm_operation(), &FakeStellar::qualified(), &evm).unwrap();
    let binding = plan.evm.as_ref().unwrap();
    let safe = binding.safe.as_ref().unwrap();
    evm.transaction = Some(serde_json::json!({
        "chainId": format!("0x{:x}", evm.chain_id),
        "nonce": "0x7",
        "to": state.contracts["evm_owner"],
        "value": "0x0",
        "input": safe_execution_input(safe),
    }));
    evm.receipt = Some(templar_oft_bridge_cli::evm::EvmReceiptV1 {
        transaction_hash: "0x1234".into(),
        block_number: Some(100),
        succeeded: Some(true),
        logs: Vec::new(),
        raw: serde_json::json!({"status": "0x1", "blockNumber": "0x64"}),
    });
    let proposal = build_proposal(Environment::StellarTestnetSepolia, plan).unwrap();
    let operation_id = templar_oft_bridge_cli::canonical_sha256(&proposal.plan.operation).unwrap();
    store
        .write_proposal(
            std::path::Path::new("proposals/evm.json"),
            &operation_id,
            &proposal,
        )
        .unwrap();
    let proposal_path = state_path.join("proposals/evm.json");

    let preview = ingest_proposal_with_adapters(
        &state_path,
        &proposal_path,
        "0x1234",
        None,
        Some(&evm),
        false,
    )
    .unwrap();
    assert_eq!(preview.result["written"], false);
    ingest_proposal_with_adapters(
        &state_path,
        &proposal_path,
        "0x1234",
        Some(&FakeStellar::qualified()),
        Some(&evm),
        true,
    )
    .unwrap();

    evm.transaction.as_mut().unwrap()["input"] = serde_json::json!("0x1234");
    let mismatch = ingest_proposal_with_adapters(
        &state_path,
        &proposal_path,
        "0x1234",
        None,
        Some(&evm),
        false,
    )
    .unwrap_err();
    assert!(matches!(mismatch, Error::Conflict(_)));
}

#[test]
fn evm_send_plan_binds_the_native_fee_as_transaction_value() {
    let operation = OperationV1::SendLeg {
        vm: Vm::Evm,
        intent: Box::new(LegIntentV1 {
            schema_name: "leg_intent".into(),
            schema_version: SCHEMA_VERSION,
            route_id: "route-governance".into(),
            desired_sha256: "desired-digest".into(),
            direction: Direction::EvmToStellar,
            amount_raw: "1000000".into(),
            destination_eid: 40_600,
            to: "GDFQVQCYYB7GKCGSCUSIQYXTPLV5YJ3XWDMWGQMDNM4EAXAL7LITIBQ7".into(),
            sender: EVM_OWNER.into(),
            refund_address: EVM_OWNER.into(),
            minimum_received_raw: "999000".into(),
            native_fee_raw: "12345".into(),
            extra_options: "0003".into(),
            maximum_native_fee_raw: "12345".into(),
            config_snapshot_sha256: "a".repeat(64),
            custody_snapshot_sha256: "b".repeat(64),
            peer_snapshot_sha256: "c".repeat(64),
            quote_source_ledger: None,
            quote_source_block: None,
            observed_sequence_nonce: None,
            fee_ceiling: None,
            pre_send_snapshot: None,
            finality_policy: None,
            additional_obligation: None,
            expires_at_unix: u64::MAX,
        }),
    };
    let plan = build_executable_plan(
        &route_state(),
        &operation,
        &FakeStellar::qualified(),
        &FakeEvm::qualified(),
    )
    .unwrap();
    assert_eq!(plan.evm.unwrap().value, "12345");
}
