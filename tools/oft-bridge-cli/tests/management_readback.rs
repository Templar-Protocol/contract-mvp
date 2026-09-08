//! Focused fake-adapter tests for the management readback boundary:
//! `route::apply_management_readback` and `route::read_evm_blocked_library`.
//! Every test proves the exact authoritative read source and value, and that
//! any mismatch leaves `RouteStateV1` unchanged. No live networks, no
//! validation commands. Option-typed Soroban views are scripted as
//! `ScVal::Vec(None|one)` matching the production decoder.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::{Arc, Mutex};

use alloy::primitives::{keccak256, Address};
use stellar_baselib::xdr::{
    AccountId, ContractId, Hash, Int128Parts, PublicKey, ScAddress, ScBytes, ScMap, ScMapEntry,
    ScSymbol, ScVal, ScVec, StringM, Uint256, VecM,
};
use stellar_strkey::Strkey;
use templar_oft_bridge_cli::domain::{
    AssetKind, AssetPolicyV1, ChainIdentityV1, Environment, OperationV1, RouteStateV1,
    SCHEMA_VERSION,
};
use templar_oft_bridge_cli::environment::{
    SEPOLIA_EID, SEPOLIA_ENDPOINT, STELLAR_TESTNET_EID, STELLAR_TESTNET_ENDPOINT,
    STELLAR_TESTNET_PASSPHRASE,
};
use templar_oft_bridge_cli::error::{Error, Result};
use templar_oft_bridge_cli::evm::EvmChain;
use templar_oft_bridge_cli::route::{
    apply_live_readback, apply_management_readback, read_evm_blocked_library,
};
use templar_oft_bridge_cli::stellar::{
    StellarChain, StellarSimulationV1, StellarTransactionStatusV1,
};

const STELLAR_OWNER: &str = "GCLQ3APIE5AS4XJUTRP5AF7ZMQAXDNGIRMIF3MIWQPF6ZPFJVNJDCN5E";
const NEW_STELLAR_OWNER: &str = "GDFQVQCYYB7GKCGSCUSIQYXTPLV5YJ3XWDMWGQMDNM4EAXAL7LITIBQ7";
const OTHER_STELLAR: &str = STELLAR_OWNER;
const STELLAR_OFT: &str = "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV";
const EVM_OWNER: &str = "0x1111111111111111111111111111111111111111";
const NEW_EVM_OWNER: &str = "0x2222222222222222222222222222222222222222";
const EVM_OFT: &str = "0x3333333333333333333333333333333333333333";
const EVM_DELEGATE: &str = "0x4444444444444444444444444444444444444444";
const BLOCKED_LIBRARY: &str = "0x5555555555555555555555555555555555555555";
const PAUSER_ROLE: &str = "PAUSER_ROLE";
const DEFAULT_ADMIN_ROLE: &str = "DEFAULT_ADMIN_ROLE";

/// Scripted Stellar adapter. Views are keyed by `(contract, function)` so any
/// read from the wrong authoritative contract fails loudly with a typed
/// `Chain` error instead of silently returning a value.
#[derive(Clone)]
struct FakeStellar {
    views: BTreeMap<(String, String), ScVal>,
    latest_ledger: u32,
    live_until: u32,
}

impl FakeStellar {
    fn new(views: BTreeMap<(String, String), ScVal>) -> Self {
        Self {
            views,
            latest_ledger: 0,
            live_until: 0,
        }
    }

    fn seed(&mut self, contract: &str, function: &str, value: ScVal) {
        self.views.insert((contract.into(), function.into()), value);
    }
}

impl StellarChain for FakeStellar {
    fn network_passphrase(&self) -> Result<String> {
        Ok(STELLAR_TESTNET_PASSPHRASE.into())
    }
    fn contract_instance_live_until(&self, _contract: &str) -> Result<u32> {
        Ok(self.live_until)
    }
    fn endpoint_eid(&self, _endpoint: &str, _source: &str) -> Result<u32> {
        Ok(STELLAR_TESTNET_EID)
    }
    fn account_sequence(&self, _account: &str) -> Result<String> {
        Ok("41".into())
    }
    fn invoke_view(
        &self,
        contract: &str,
        function: &str,
        _args_xdr_hex: &[String],
        _source: &str,
    ) -> Result<stellar_baselib::xdr::ScVal> {
        self.views
            .get(&(contract.into(), function.into()))
            .cloned()
            .ok_or_else(|| {
                Error::Chain(format!("unexpected stellar view {function} on {contract}"))
            })
    }
    fn token_balance(&self, _token: &str, _address: &str, _source: &str) -> Result<String> {
        Ok("0".into())
    }
    fn account_signers(&self, _account: &str) -> Result<std::collections::BTreeMap<String, u32>> {
        Ok(BTreeMap::new())
    }
    fn account_threshold(&self, _account: &str, _level: &str) -> Result<u32> {
        Ok(1)
    }
    fn latest_ledger(&self) -> Result<u32> {
        Ok(self.latest_ledger)
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
        Err(Error::Chain(
            "unexpected stellar simulate on the readback path".into(),
        ))
    }
    fn submit_transaction(&self, _signed_envelope_xdr: &str) -> Result<String> {
        Err(Error::Chain(
            "unexpected stellar submit on the readback path".into(),
        ))
    }
    fn transaction_status(&self, _transaction_hash: &str) -> Result<StellarTransactionStatusV1> {
        Err(Error::Chain(
            "unexpected stellar status poll on the readback path".into(),
        ))
    }
}

/// Scripted EVM adapter. Responses are keyed by the 4-byte selector so a call
/// with unexpected calldata fails loudly; every call is recorded for
/// authoritative-source assertions.
#[derive(Clone)]
struct FakeEvm {
    responses: BTreeMap<Vec<u8>, Vec<u8>>,
    calls: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
}

impl FakeEvm {
    fn new(responses: BTreeMap<Vec<u8>, Vec<u8>>) -> Self {
        Self {
            responses,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> Vec<(String, Vec<u8>)> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl EvmChain for FakeEvm {
    async fn chain_id(&self) -> Result<u64> {
        Ok(11_155_111)
    }
    async fn code(&self, _address: Address) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }
    async fn call(&self, to: Address, calldata: Vec<u8>) -> Result<Vec<u8>> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((format!("{to:#x}"), calldata.clone()));
        let selector = calldata.get(..4).unwrap_or_default();
        self.responses
            .get(selector)
            .cloned()
            .ok_or_else(|| Error::Chain("unexpected evm call on fake adapter".into()))
    }
    async fn endpoint_eid(&self, _endpoint: &str) -> Result<u32> {
        Ok(SEPOLIA_EID)
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
    ) -> Result<templar_oft_bridge_cli::evm::EvmSimulationV1> {
        Err(Error::Chain(
            "unexpected evm estimate on the readback path".into(),
        ))
    }
    async fn send_raw_transaction(&self, _encoded: &[u8]) -> Result<String> {
        Err(Error::Chain(
            "unexpected evm send on the readback path".into(),
        ))
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

fn route_state() -> RouteStateV1 {
    RouteStateV1 {
        schema_name: "route_state".into(),
        schema_version: SCHEMA_VERSION,
        route_id: "route-management-readback".into(),
        desired_sha256: "desired-digest".into(),
        identity: ChainIdentityV1 {
            environment: Environment::StellarTestnetSepolia,
            stellar_passphrase: STELLAR_TESTNET_PASSPHRASE.into(),
            stellar_eid: STELLAR_TESTNET_EID,
            stellar_endpoint: STELLAR_TESTNET_ENDPOINT.into(),
            stellar_endpoint_code_hash: "1".repeat(64),
            evm_chain_id: 11_155_111,
            evm_eid: SEPOLIA_EID,
            evm_endpoint: SEPOLIA_ENDPOINT.into(),
            evm_endpoint_code_hash: "2".repeat(64),
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
            ("stellar_oft".into(), STELLAR_OFT.into()),
            ("evm_owner".into(), EVM_OWNER.into()),
            ("evm_oft".into(), EVM_OFT.into()),
        ]),
        requested_config: BTreeMap::new(),
        effective_config: BTreeMap::new(),
    }
}

fn stellar_address(value: &str) -> Result<ScVal> {
    let address = match Strkey::from_str(value) {
        Ok(Strkey::PublicKeyEd25519(key)) => {
            ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(key.0))))
        }
        Ok(Strkey::Contract(contract)) => ScAddress::Contract(ContractId(Hash(contract.0))),
        _ => return Err(Error::InvalidInput(format!("{value} is not a strkey"))),
    };
    Ok(ScVal::Address(address))
}

fn symbol(value: &str) -> Result<ScVal> {
    Ok(ScVal::Symbol(ScSymbol(
        StringM::try_from(value.as_bytes().to_vec())
            .map_err(|error| Error::InvalidInput(format!("invalid symbol: {error}")))?,
    )))
}

/// Soroban `Option` readback: `ScVal::Vec` containing exactly one value.
fn option_value(value: ScVal) -> ScVal {
    ScVal::Vec(Some(ScVec(
        VecM::try_from(vec![value]).expect("one-element vec"),
    )))
}

/// Soroban `Option::None` readback.
fn option_none() -> ScVal {
    ScVal::Vec(None)
}

fn bytes_hex(bytes: &[u8]) -> ScVal {
    ScVal::Bytes(ScBytes(bytes.to_vec().try_into().expect("bytes vec")))
}

fn evm_word(address: &str) -> Result<Vec<u8>> {
    let address = Address::from_str(address)
        .map_err(|error| Error::InvalidInput(format!("invalid evm address: {error}")))?;
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(address.as_slice());
    Ok(word.to_vec())
}

fn selector(signature: &str) -> Vec<u8> {
    keccak256(signature.as_bytes())[..4].to_vec()
}

fn assert_unchanged(state: &RouteStateV1, snapshot: &RouteStateV1) {
    assert_eq!(
        state, snapshot,
        "mismatch must leave RouteStateV1 unchanged"
    );
}

fn conflict(error: Error) -> String {
    match error {
        Error::Conflict(message) => message,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

// ---- Stellar owner: begin / accept / cancel -------------------------------

#[test]
fn begin_owner_transfer_records_exact_pending_owner_and_ttl() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "pending_owner",
        option_value(stellar_address(NEW_STELLAR_OWNER)?),
    );
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::BeginStellarOwnershipTransfer {
            new_owner: NEW_STELLAR_OWNER.into(),
            ttl: 4000,
        },
    )?;
    assert_eq!(
        state.effective_config["stellar:pending_owner"],
        serde_json::Value::String(NEW_STELLAR_OWNER.into())
    );
    assert_eq!(
        state.effective_config["stellar:pending_owner_ttl"],
        serde_json::Value::String("4000".into())
    );
    Ok(())
}

#[test]
fn begin_owner_transfer_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "pending_owner",
        option_value(stellar_address(OTHER_STELLAR).unwrap()),
    );
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::BeginStellarOwnershipTransfer {
            new_owner: NEW_STELLAR_OWNER.into(),
            ttl: 4000,
        },
    )
    .expect_err("differing pending owner must refuse");
    assert!(conflict(error).contains("pending Stellar owner"));
    assert_unchanged(&state, &snapshot);
}

#[test]
fn accept_owner_commits_new_owner_and_clears_pending() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "owner",
        option_value(stellar_address(NEW_STELLAR_OWNER)?),
    );
    let mut state = route_state();
    state.effective_config.insert(
        "stellar:pending_owner".into(),
        serde_json::Value::String(NEW_STELLAR_OWNER.into()),
    );
    state.effective_config.insert(
        "stellar:pending_owner_ttl".into(),
        serde_json::Value::String("4000".into()),
    );
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::AcceptStellarOwnership,
    )?;
    assert_eq!(state.contracts["stellar_owner"], NEW_STELLAR_OWNER);
    assert!(!state.effective_config.contains_key("stellar:pending_owner"));
    assert!(!state
        .effective_config
        .contains_key("stellar:pending_owner_ttl"));
    Ok(())
}

#[test]
fn accept_owner_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "owner",
        option_value(stellar_address(OTHER_STELLAR).unwrap()),
    );
    let mut state = route_state();
    state.effective_config.insert(
        "stellar:pending_owner".into(),
        serde_json::Value::String(NEW_STELLAR_OWNER.into()),
    );
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::AcceptStellarOwnership,
    )
    .expect_err("owner readback differing from pending must refuse");
    assert!(conflict(error).contains("Stellar owner"));
    assert_unchanged(&state, &snapshot);
}

#[test]
fn cancel_owner_transfer_clears_recorded_pending() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "pending_owner", option_none());
    let mut state = route_state();
    state.effective_config.insert(
        "stellar:pending_owner".into(),
        serde_json::Value::String(NEW_STELLAR_OWNER.into()),
    );
    state.effective_config.insert(
        "stellar:pending_owner_ttl".into(),
        serde_json::Value::String("4000".into()),
    );
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::CancelStellarOwnershipTransfer,
    )?;
    assert!(!state.effective_config.contains_key("stellar:pending_owner"));
    assert!(!state
        .effective_config
        .contains_key("stellar:pending_owner_ttl"));
    Ok(())
}

#[test]
fn cancel_owner_transfer_with_live_pending_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "pending_owner",
        option_value(stellar_address(NEW_STELLAR_OWNER).unwrap()),
    );
    let mut state = route_state();
    state.effective_config.insert(
        "stellar:pending_owner".into(),
        serde_json::Value::String(NEW_STELLAR_OWNER.into()),
    );
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::CancelStellarOwnershipTransfer,
    )
    .expect_err("live pending owner must refuse cancellation");
    assert!(conflict(error).contains("still exists"));
    assert_unchanged(&state, &snapshot);
}

// ---- Delegates -------------------------------------------------------------

#[test]
fn set_stellar_delegate_records_exact_endpoint_delegate() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_TESTNET_ENDPOINT,
        "delegate",
        option_value(stellar_address(NEW_STELLAR_OWNER)?),
    );
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetStellarDelegate {
            delegate: NEW_STELLAR_OWNER.into(),
        },
    )?;
    assert_eq!(state.contracts["stellar_delegate"], NEW_STELLAR_OWNER);
    Ok(())
}

#[test]
fn set_stellar_delegate_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_TESTNET_ENDPOINT,
        "delegate",
        option_value(stellar_address(OTHER_STELLAR).unwrap()),
    );
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetStellarDelegate {
            delegate: NEW_STELLAR_OWNER.into(),
        },
    )
    .expect_err("differing delegate must refuse");
    assert!(conflict(error).contains("Stellar delegate"));
    assert_unchanged(&state, &snapshot);
}

#[test]
fn set_evm_delegate_records_exact_endpoint_delegate() -> Result<()> {
    let evm = FakeEvm::new(BTreeMap::from([(
        selector("delegates(address)"),
        evm_word(EVM_DELEGATE)?,
    )]));
    let mut state = route_state();
    apply_management_readback(
        &FakeStellar::new(BTreeMap::new()),
        &evm,
        &mut state,
        &OperationV1::SetEvmDelegate {
            delegate: EVM_DELEGATE.into(),
        },
    )?;
    assert_eq!(state.contracts["evm_delegate"], EVM_DELEGATE);
    let calls = evm.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].0.eq_ignore_ascii_case(SEPOLIA_ENDPOINT));
    assert_eq!(calls[0].1[..4], selector("delegates(address)")[..]);
    Ok(())
}

#[test]
fn set_evm_delegate_mismatch_leaves_state_unchanged() {
    let evm = FakeEvm::new(BTreeMap::from([(
        selector("delegates(address)"),
        evm_word(NEW_EVM_OWNER).unwrap(),
    )]));
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &FakeStellar::new(BTreeMap::new()),
        &evm,
        &mut state,
        &OperationV1::SetEvmDelegate {
            delegate: EVM_DELEGATE.into(),
        },
    )
    .expect_err("differing EVM delegate must refuse");
    assert!(conflict(error).contains("EVM delegate"));
    assert_unchanged(&state, &snapshot);
}

// ---- EVM owner -------------------------------------------------------------

#[test]
fn transfer_evm_owner_records_exact_oft_owner() -> Result<()> {
    let evm = FakeEvm::new(BTreeMap::from([(
        selector("owner()"),
        evm_word(NEW_EVM_OWNER)?,
    )]));
    let mut state = route_state();
    apply_management_readback(
        &FakeStellar::new(BTreeMap::new()),
        &evm,
        &mut state,
        &OperationV1::TransferEvmOwnership {
            new_owner: NEW_EVM_OWNER.into(),
        },
    )?;
    assert_eq!(state.contracts["evm_owner"], NEW_EVM_OWNER);
    let calls = evm.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, EVM_OFT);
    assert_eq!(calls[0].1[..4], selector("owner()")[..]);
    Ok(())
}

#[test]
fn transfer_evm_owner_mismatch_leaves_state_unchanged() {
    let evm = FakeEvm::new(BTreeMap::from([(
        selector("owner()"),
        evm_word(EVM_DELEGATE).unwrap(),
    )]));
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &FakeStellar::new(BTreeMap::new()),
        &evm,
        &mut state,
        &OperationV1::TransferEvmOwnership {
            new_owner: NEW_EVM_OWNER.into(),
        },
    )
    .expect_err("differing EVM owner must refuse");
    assert!(conflict(error).contains("EVM owner"));
    assert_unchanged(&state, &snapshot);
}

// ---- Fees ------------------------------------------------------------------

#[test]
fn default_fee_exact_readback_records_bps() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "default_fee_bps",
        option_value(ScVal::U32(250)),
    );
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetDefaultFee { bps: 250 },
    )?;
    assert_eq!(
        state.effective_config["fee_bps:stellar_default"],
        serde_json::Value::String("250".into())
    );
    Ok(())
}

#[test]
fn default_fee_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "default_fee_bps",
        option_value(ScVal::U32(300)),
    );
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetDefaultFee { bps: 250 },
    )
    .expect_err("differing default fee must refuse");
    assert!(conflict(error).contains("default fee"));
    assert_unchanged(&state, &snapshot);
}

#[test]
fn destination_fee_exact_readback_records_bps_and_to_evm_alias() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "fee_bps", ScVal::U32(180));
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetDestinationFee {
            remote_eid: SEPOLIA_EID,
            bps: 180,
        },
    )?;
    assert_eq!(
        state.effective_config[&format!("fee_bps:stellar:{SEPOLIA_EID}")],
        serde_json::Value::String("180".into())
    );
    assert_eq!(
        state.effective_config["fee_bps:stellar_to_evm"],
        serde_json::Value::String("180".into())
    );
    Ok(())
}

#[test]
fn destination_fee_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "fee_bps", ScVal::U32(200));
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetDestinationFee {
            remote_eid: SEPOLIA_EID,
            bps: 180,
        },
    )
    .expect_err("differing destination fee must refuse");
    assert!(conflict(error).contains("destination fee"));
    assert_unchanged(&state, &snapshot);
}

#[test]
fn fee_recipient_exact_readback_records_address() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "fee_deposit_address",
        option_value(stellar_address(NEW_STELLAR_OWNER)?),
    );
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetFeeRecipient {
            recipient: NEW_STELLAR_OWNER.into(),
        },
    )?;
    assert_eq!(
        state.effective_config["fee_deposit_address:stellar"],
        serde_json::Value::String(NEW_STELLAR_OWNER.into())
    );
    Ok(())
}

#[test]
fn fee_recipient_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "fee_deposit_address",
        option_value(stellar_address(OTHER_STELLAR).unwrap()),
    );
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetFeeRecipient {
            recipient: NEW_STELLAR_OWNER.into(),
        },
    )
    .expect_err("differing fee recipient must refuse");
    assert!(conflict(error).contains("fee recipient"));
    assert_unchanged(&state, &snapshot);
}

// ---- Message inspector -----------------------------------------------------

#[test]
fn message_inspector_set_and_clear_readback() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "msg_inspector",
        option_value(stellar_address(NEW_STELLAR_OWNER)?),
    );
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetMessageInspector {
            inspector: Some(NEW_STELLAR_OWNER.into()),
        },
    )?;
    assert_eq!(
        state.effective_config["message_inspector:stellar"],
        serde_json::Value::String(NEW_STELLAR_OWNER.into())
    );
    stellar.seed(STELLAR_OFT, "msg_inspector", option_none());
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetMessageInspector { inspector: None },
    )?;
    assert_eq!(
        state.effective_config["message_inspector:stellar"],
        serde_json::Value::Null
    );
    Ok(())
}

#[test]
fn message_inspector_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "msg_inspector",
        option_value(stellar_address(OTHER_STELLAR).unwrap()),
    );
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetMessageInspector {
            inspector: Some(NEW_STELLAR_OWNER.into()),
        },
    )
    .expect_err("differing inspector must refuse");
    assert!(conflict(error).contains("message inspector"));
    assert_unchanged(&state, &snapshot);
}

// ---- Pause -----------------------------------------------------------------

#[test]
fn pause_and_unpause_exact_readback_records_state() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "is_paused", ScVal::Bool(true));
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::PauseEmergency,
    )?;
    assert_eq!(
        state.effective_config["stellar:is_paused"],
        serde_json::Value::Bool(true)
    );
    stellar.seed(STELLAR_OFT, "is_paused", ScVal::Bool(false));
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::UnpauseEmergency,
    )?;
    assert_eq!(
        state.effective_config["stellar:is_paused"],
        serde_json::Value::Bool(false)
    );
    Ok(())
}

#[test]
fn pause_readback_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "is_paused", ScVal::Bool(false));
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::PauseEmergency,
    )
    .expect_err("unpaused readback must refuse pause");
    assert!(conflict(error).contains("pause state"));
    assert_unchanged(&state, &snapshot);
}

// ---- TTL: config / freeze / actual instance expiration ---------------------

fn ttl_map(threshold: u32, extend_to: u32) -> Result<ScVal> {
    Ok(ScVal::Map(Some(ScMap(
        VecM::try_from(vec![
            ScMapEntry {
                key: ScVal::Symbol(ScSymbol(
                    StringM::try_from(b"threshold".to_vec()).expect("symbol"),
                )),
                val: ScVal::U32(threshold),
            },
            ScMapEntry {
                key: ScVal::Symbol(ScSymbol(
                    StringM::try_from(b"extend_to".to_vec()).expect("symbol"),
                )),
                val: ScVal::U32(extend_to),
            },
        ])
        .expect("map"),
    ))))
}

/// Soroban `ttl_configs` tuple: two option-wrapped inner config maps.
fn ttl_configs(instance: ScVal, persistent: ScVal) -> ScVal {
    ScVal::Vec(Some(ScVec(
        VecM::try_from(vec![option_value(instance), option_value(persistent)])
            .expect("two-element vec"),
    )))
}

#[test]
fn ttl_config_exact_readback_records_every_field() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "ttl_configs",
        ttl_configs(ttl_map(10, 20)?, ttl_map(30, 40)?),
    );
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetTtlConfig {
            instance_threshold: 10,
            instance_extend_to: 20,
            persistent_threshold: 30,
            persistent_extend_to: 40,
        },
    )?;
    for (key, value) in [
        ("ttl:instance_threshold", "10"),
        ("ttl:instance_extend_to", "20"),
        ("ttl:persistent_threshold", "30"),
        ("ttl:persistent_extend_to", "40"),
    ] {
        assert_eq!(
            state.effective_config[key],
            serde_json::Value::String(value.into())
        );
    }
    Ok(())
}

#[test]
fn ttl_config_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "ttl_configs",
        ttl_configs(ttl_map(10, 21).unwrap(), ttl_map(30, 40).unwrap()),
    );
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetTtlConfig {
            instance_threshold: 10,
            instance_extend_to: 20,
            persistent_threshold: 30,
            persistent_extend_to: 40,
        },
    )
    .expect_err("differing extend_to must refuse");
    assert!(conflict(error).contains("TTL config"));
    assert_unchanged(&state, &snapshot);
}

#[test]
fn freeze_ttl_exact_readback_records_frozen() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "is_ttl_configs_frozen", ScVal::Bool(true));
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::FreezeTtlConfig {
            acknowledgement: "freeze".into(),
        },
    )?;
    assert_eq!(
        state.effective_config["ttl:is_frozen"],
        serde_json::Value::Bool(true)
    );
    Ok(())
}

#[test]
fn freeze_ttl_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "is_ttl_configs_frozen", ScVal::Bool(false));
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::FreezeTtlConfig {
            acknowledgement: "freeze".into(),
        },
    )
    .expect_err("unfrozen readback must refuse freeze");
    assert!(conflict(error).contains("TTL freeze"));
    assert_unchanged(&state, &snapshot);
}

#[test]
fn extend_instance_ttl_records_current_ledger_and_live_until() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.latest_ledger = 100;
    stellar.live_until = 200;
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::ExtendInstanceTtl { ledgers: 100 },
    )?;
    assert_eq!(
        state.effective_config["ttl:current_ledger"],
        serde_json::Value::String("100".into())
    );
    assert_eq!(
        state.effective_config["ttl:instance_live_until"],
        serde_json::Value::String("200".into())
    );
    Ok(())
}

#[test]
fn extend_instance_ttl_not_extended_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.latest_ledger = 100;
    stellar.live_until = 100;
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::ExtendInstanceTtl { ledgers: 100 },
    )
    .expect_err("expired instance TTL must refuse");
    assert!(conflict(error).contains("was not extended"));
    assert_unchanged(&state, &snapshot);
}

// ---- Roles: grant / revoke / admin / remove-admin --------------------------

#[test]
fn grant_role_records_address_after_exact_has_role_readback() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "has_role", option_value(ScVal::U32(1)));
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::GrantRole {
            role: PAUSER_ROLE.into(),
            address: NEW_STELLAR_OWNER.into(),
        },
    )?;
    assert_eq!(
        state.contracts[&format!("stellar_role:{PAUSER_ROLE}")],
        NEW_STELLAR_OWNER
    );
    Ok(())
}

#[test]
fn grant_role_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "has_role", option_none());
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::GrantRole {
            role: PAUSER_ROLE.into(),
            address: NEW_STELLAR_OWNER.into(),
        },
    )
    .expect_err("absent role grant readback must refuse");
    assert!(conflict(error).contains("role grant"));
    assert_unchanged(&state, &snapshot);
}

#[test]
fn revoke_role_removes_the_recorded_address() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "has_role", option_none());
    let mut state = route_state();
    state.contracts.insert(
        format!("stellar_role:{PAUSER_ROLE}"),
        NEW_STELLAR_OWNER.into(),
    );
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::RevokeRole {
            role: PAUSER_ROLE.into(),
            address: NEW_STELLAR_OWNER.into(),
        },
    )?;
    assert!(!state
        .contracts
        .contains_key(&format!("stellar_role:{PAUSER_ROLE}")));
    Ok(())
}

#[test]
fn revoke_role_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "has_role", option_value(ScVal::U32(1)));
    let mut state = route_state();
    state.contracts.insert(
        format!("stellar_role:{PAUSER_ROLE}"),
        NEW_STELLAR_OWNER.into(),
    );
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::RevokeRole {
            role: PAUSER_ROLE.into(),
            address: NEW_STELLAR_OWNER.into(),
        },
    )
    .expect_err("live role readback must refuse revocation");
    assert!(conflict(error).contains("role grant"));
    assert_unchanged(&state, &snapshot);
}

#[test]
fn set_role_admin_records_exact_admin_after_symbol_readback() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "get_role_admin",
        option_value(symbol(DEFAULT_ADMIN_ROLE)?),
    );
    let mut state = route_state();
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetRoleAdmin {
            role: PAUSER_ROLE.into(),
            admin_role: DEFAULT_ADMIN_ROLE.into(),
        },
    )?;
    assert_eq!(
        state.effective_config[&format!("stellar_role_admin:{PAUSER_ROLE}")],
        serde_json::Value::String(DEFAULT_ADMIN_ROLE.into())
    );
    Ok(())
}

#[test]
fn set_role_admin_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "get_role_admin",
        option_value(symbol("OPERATOR_ROLE").unwrap()),
    );
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetRoleAdmin {
            role: PAUSER_ROLE.into(),
            admin_role: DEFAULT_ADMIN_ROLE.into(),
        },
    )
    .expect_err("differing role admin must refuse");
    assert!(conflict(error).contains("role admin"));
    assert_unchanged(&state, &snapshot);
}

#[test]
fn remove_role_admin_clears_recorded_admin_after_void_readback() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "get_role_admin", option_none());
    let mut state = route_state();
    state.effective_config.insert(
        format!("stellar_role_admin:{PAUSER_ROLE}"),
        serde_json::Value::String(DEFAULT_ADMIN_ROLE.into()),
    );
    apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::RemoveRoleAdmin {
            role: PAUSER_ROLE.into(),
            admin_role: DEFAULT_ADMIN_ROLE.into(),
        },
    )?;
    assert!(!state
        .effective_config
        .contains_key(&format!("stellar_role_admin:{PAUSER_ROLE}")));
    Ok(())
}

#[test]
fn remove_role_admin_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "get_role_admin",
        option_value(symbol(DEFAULT_ADMIN_ROLE).unwrap()),
    );
    let mut state = route_state();
    state.effective_config.insert(
        format!("stellar_role_admin:{PAUSER_ROLE}"),
        serde_json::Value::String(DEFAULT_ADMIN_ROLE.into()),
    );
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::RemoveRoleAdmin {
            role: PAUSER_ROLE.into(),
            admin_role: DEFAULT_ADMIN_ROLE.into(),
        },
    )
    .expect_err("live role admin must refuse removal");
    assert!(conflict(error).contains("role admin still exists"));
    assert_unchanged(&state, &snapshot);
}

// ---- Rate-limit delegation into apply_live_readback ------------------------

fn rate_limit_map(limit: u128, window_seconds: u64, mode: &str) -> Result<ScVal> {
    let hi = (limit >> 64) as i64;
    let lo = (limit & u64::MAX as u128) as u64;
    Ok(ScVal::Map(Some(ScMap(
        VecM::try_from(vec![
            ScMapEntry {
                key: ScVal::Symbol(ScSymbol(
                    StringM::try_from(b"limit".to_vec()).expect("symbol"),
                )),
                val: ScVal::I128(Int128Parts { lo, hi }),
            },
            ScMapEntry {
                key: ScVal::Symbol(ScSymbol(
                    StringM::try_from(b"window_seconds".to_vec()).expect("symbol"),
                )),
                val: ScVal::U64(window_seconds),
            },
            ScMapEntry {
                key: ScVal::Symbol(ScSymbol(
                    StringM::try_from(b"mode".to_vec()).expect("symbol"),
                )),
                val: ScVal::Vec(Some(ScVec(
                    VecM::try_from(vec![ScVal::Symbol(ScSymbol(
                        StringM::try_from(mode.as_bytes().to_vec()).expect("symbol"),
                    ))])
                    .expect("one-element vec"),
                ))),
            },
        ])
        .expect("map"),
    ))))
}

#[test]
fn rate_limit_exact_readback_records_config_for_both_directions() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "rate_limit_config",
        rate_limit_map(5_000_000, 3600, "Net")?,
    );
    let mut state = route_state();
    for (operation, prefix) in [
        (
            OperationV1::SetOutboundRateLimit {
                remote_eid: SEPOLIA_EID,
                limit_raw: 5_000_000,
                window_seconds: 3600,
                mode: "net".into(),
            },
            "outbound_rate_limit",
        ),
        (
            OperationV1::SetInboundRateLimit {
                remote_eid: SEPOLIA_EID,
                limit_raw: 5_000_000,
                window_seconds: 3600,
                mode: "net".into(),
            },
            "inbound_rate_limit",
        ),
    ] {
        apply_management_readback(
            &stellar,
            &FakeEvm::new(BTreeMap::new()),
            &mut state,
            &operation,
        )?;
        assert_eq!(
            state.effective_config[&format!("{prefix}:stellar:{SEPOLIA_EID}:limit_raw")],
            serde_json::Value::String("5000000".into())
        );
        assert_eq!(
            state.effective_config[&format!("{prefix}:stellar:{SEPOLIA_EID}:window_seconds")],
            serde_json::Value::String("3600".into())
        );
        assert_eq!(
            state.effective_config[&format!("{prefix}:stellar:{SEPOLIA_EID}:mode")],
            serde_json::Value::String("net".into())
        );
    }
    Ok(())
}

#[test]
fn rate_limit_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(
        STELLAR_OFT,
        "rate_limit_config",
        rate_limit_map(5_000_000, 1200, "Net").unwrap(),
    );
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_management_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetOutboundRateLimit {
            remote_eid: SEPOLIA_EID,
            limit_raw: 5_000_000,
            window_seconds: 3600,
            mode: "net".into(),
        },
    )
    .expect_err("differing window must refuse");
    assert!(conflict(error).contains("rate-limit"));
    assert_unchanged(&state, &snapshot);
}

// ---- Enforced options readback (route-config surface) ----------------------

#[test]
fn stellar_enforced_options_exact_readback_records_options() -> Result<()> {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "enforced_options", bytes_hex(&[0x00, 0x01]));
    let mut state = route_state();
    apply_live_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetStellarReceiveOptions {
            remote_eid: SEPOLIA_EID,
            message_type: 1,
            options: "0x0001".into(),
        },
    )?;
    assert_eq!(
        state.effective_config[&format!("receive_options:stellar:{SEPOLIA_EID}:1")],
        serde_json::Value::String("0x0001".into())
    );
    Ok(())
}

#[test]
fn stellar_enforced_options_mismatch_leaves_state_unchanged() {
    let mut stellar = FakeStellar::new(BTreeMap::new());
    stellar.seed(STELLAR_OFT, "enforced_options", bytes_hex(&[0x00, 0x02]));
    let mut state = route_state();
    let snapshot = state.clone();
    let error = apply_live_readback(
        &stellar,
        &FakeEvm::new(BTreeMap::new()),
        &mut state,
        &OperationV1::SetStellarReceiveOptions {
            remote_eid: SEPOLIA_EID,
            message_type: 1,
            options: "0x0001".into(),
        },
    )
    .expect_err("differing options must refuse");
    assert!(conflict(error).contains("options"));
    assert_unchanged(&state, &snapshot);
}

// ---- EVM blocked library ---------------------------------------------------

#[test]
fn blocked_library_returns_the_nonzero_endpoint_constant() -> Result<()> {
    let evm = FakeEvm::new(BTreeMap::from([(
        selector("blockedLibrary()"),
        evm_word(BLOCKED_LIBRARY)?,
    )]));
    let address = read_evm_blocked_library(&evm, SEPOLIA_ENDPOINT)?;
    assert_eq!(address, BLOCKED_LIBRARY);
    let calls = evm.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].0.eq_ignore_ascii_case(SEPOLIA_ENDPOINT));
    assert_eq!(calls[0].1[..4], selector("blockedLibrary()")[..]);
    Ok(())
}

#[test]
fn blocked_library_zero_word_is_refused() {
    let evm = FakeEvm::new(BTreeMap::from([(
        selector("blockedLibrary()"),
        vec![0u8; 32],
    )]));
    let error = read_evm_blocked_library(&evm, SEPOLIA_ENDPOINT)
        .expect_err("zero blocked library must be refused");
    assert!(conflict(error).contains("blockedLibrary readback is zero"));
}
