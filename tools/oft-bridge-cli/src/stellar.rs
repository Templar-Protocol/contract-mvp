//! Stellar-side checked native adapter: secret custody and authorization
//! boundaries. All operations are pure decisions over typed inputs; no live
//! chain mutation happens here.

use std::collections::BTreeSet;
use std::env;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use soroban_client::keypair::{Keypair, KeypairBehavior};
use stellar_strkey::{ed25519::PrivateKey, Strkey};
use zeroize::Zeroizing;

use crate::domain::{Environment, OperationV1};
use crate::error::{Error, Result};

/// Encoded length of a Stellar `S...` secret seed strkey.
pub const SEED_STRKEY_LEN: usize = 56;

/// Named-environment-variable secret provider. The seed is accepted only
/// through the named env var, validated and decoded as an Ed25519 `S...`
/// strkey, held zeroized, and used to derive the `G...` public key.
#[derive(Debug)]
pub struct StellarSecretProviderV1 {
    env_var: String,
    seed: Zeroizing<Vec<u8>>,
    public_key: String,
}

impl StellarSecretProviderV1 {
    /// Reads the named environment variable, checks and decodes the `S...`
    /// seed, and derives the public key. The raw variable and the decoded
    /// seed bytes are zeroized when dropped.
    pub fn from_named_env(env_var: &str) -> Result<Self> {
        if env_var.trim().is_empty() {
            return Err(Error::InvalidInput(
                "secret env var name must not be empty".into(),
            ));
        }
        let raw = Zeroizing::new(env::var(env_var).map_err(|_| {
            Error::InvalidInput(format!("environment variable {env_var} is not set"))
        })?);
        let trimmed = raw.trim();
        if trimmed.len() != SEED_STRKEY_LEN || !trimmed.starts_with('S') {
            return Err(Error::InvalidInput(format!(
                "environment variable {env_var} must hold a {SEED_STRKEY_LEN}-character S... seed strkey"
            )));
        }
        let decoded = PrivateKey::from_str(trimmed)
            .map_err(|error| Error::InvalidInput(format!("invalid S... seed strkey: {error}")))?;
        let keypair = Keypair::from_secret(trimmed)
            .map_err(|error| Error::InvalidInput(format!("seed rejected by keypair: {error}")))?;
        Ok(Self {
            env_var: env_var.to_string(),
            seed: Zeroizing::new(decoded.0.to_vec()),
            public_key: keypair.public_key(),
        })
    }

    /// Name of the environment variable the seed was read from.
    pub fn env_var(&self) -> &str {
        &self.env_var
    }

    /// Derived `G...` public key strkey.
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// Raw decoded 32-byte seed. Zeroized when the provider drops.
    pub fn seed(&self) -> &[u8] {
        &self.seed
    }
}

/// Authorization classes the v1 Stellar adapter distinguishes. Address and
/// contract auth is not a class: it is observed evidence and is refused in v1.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StellarAuthorizationClass {
    /// Explicit OFT role operator entry.
    OftRoleOperator,
    /// Owner-derived AUTHORIZER.
    OwnerDerivedAuthorizer,
    /// OApp/Endpoint delegate.
    OAppDelegate,
}

/// Observed signer evidence for one operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StellarSignerEvidenceV1 {
    /// Signer holds an explicit OFT role operator entry.
    OftRoleOperator { role: String, address: String },
    /// Signer is the owner-derived AUTHORIZER.
    OwnerDerivedAuthorizer { owner: String },
    /// Signer is the OApp/Endpoint delegate.
    OAppDelegate { delegate: String },
    /// Signer authorized through an Address/contract entry; refused in v1.
    AddressContract { address: String },
}

impl StellarSignerEvidenceV1 {
    /// Authorization class of the evidence, if not contract auth.
    pub fn class(&self) -> Option<StellarAuthorizationClass> {
        match self {
            Self::OftRoleOperator { .. } => Some(StellarAuthorizationClass::OftRoleOperator),
            Self::OwnerDerivedAuthorizer { .. } => {
                Some(StellarAuthorizationClass::OwnerDerivedAuthorizer)
            }
            Self::OAppDelegate { .. } => Some(StellarAuthorizationClass::OAppDelegate),
            Self::AddressContract { .. } => None,
        }
    }

    /// Address of the signer implied by the evidence.
    pub fn signer(&self) -> &str {
        match self {
            Self::OftRoleOperator { address, .. }
            | Self::OwnerDerivedAuthorizer { owner: address }
            | Self::OAppDelegate { delegate: address }
            | Self::AddressContract { address } => address,
        }
    }
}

/// Per-operation authorization spec.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StellarAuthorizationSpecV1 {
    pub operation: String,
    pub allowed_classes: BTreeSet<StellarAuthorizationClass>,
    pub require_source_account: bool,
}

/// Typed authorization request boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationRequestV1 {
    pub environment: Environment,
    pub operation: OperationV1,
    pub source_account: String,
    pub evidence: StellarSignerEvidenceV1,
}

/// Typed authorization decision boundary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationDecisionV1 {
    pub operation: String,
    pub signer: String,
    pub class: StellarAuthorizationClass,
    pub source_account_auth: bool,
}

/// Checked native adapter trait for Stellar authorization decisions.
pub trait StellarAuthorizationAdapter {
    fn authorize(&self, request: &AuthorizationRequestV1) -> Result<AuthorizationDecisionV1>;
}

/// Authorization spec for an operation. Config-mutating Stellar operations
/// accept operator and owner-derived AUTHORIZER; message send additionally
/// accepts the OApp/Endpoint delegate. Every op requires source-account auth.
pub fn authorization_spec_v1(operation: &OperationV1) -> StellarAuthorizationSpecV1 {
    let mut allowed_classes = BTreeSet::new();
    allowed_classes.insert(StellarAuthorizationClass::OftRoleOperator);
    allowed_classes.insert(StellarAuthorizationClass::OwnerDerivedAuthorizer);
    if matches!(operation, OperationV1::SendLeg { .. }) {
        allowed_classes.insert(StellarAuthorizationClass::OAppDelegate);
    }
    StellarAuthorizationSpecV1 {
        operation: operation_label(operation),
        allowed_classes,
        require_source_account: true,
    }
}

/// Stable operation label used in decisions and refusals.
pub fn operation_label(operation: &OperationV1) -> String {
    match operation {
        OperationV1::InstallStellarWasm { .. } => "install_stellar_wasm",
        OperationV1::DeployStellarOft { .. } => "deploy_stellar_oft",
        OperationV1::DeployEvmOft { .. } => "deploy_evm_oft",
        OperationV1::BeginStellarOwnershipTransfer { .. } => "begin_stellar_ownership_transfer",
        OperationV1::AcceptStellarOwnership => "accept_stellar_ownership",
        OperationV1::CancelStellarOwnershipTransfer => "cancel_stellar_ownership_transfer",
        OperationV1::TransferEvmOwnership { .. } => "transfer_evm_ownership",
        OperationV1::SetStellarDelegate { .. } => "set_stellar_delegate",
        OperationV1::SetEvmDelegate { .. } => "set_evm_delegate",
        OperationV1::SetStellarPeer { .. } => "set_peer_stellar",
        OperationV1::SetEvmPeer { .. } => "set_peer_evm",
        OperationV1::SetStellarSendLibrary { .. } => "set_send_library_stellar",
        OperationV1::SetStellarReceiveLibrary { .. } => "set_receive_library_stellar",
        OperationV1::RemoveStellarReceiveLibraryTimeout { .. } => {
            "remove_receive_library_timeout_stellar"
        }
        OperationV1::SetEvmSendLibrary { .. } => "set_send_library_evm",
        OperationV1::SetEvmReceiveLibrary { .. } => "set_receive_library_evm",
        OperationV1::RemoveEvmReceiveLibraryTimeout { .. } => "remove_receive_library_timeout_evm",
        OperationV1::SetStellarUlnConfig { .. } => "set_uln_stellar",
        OperationV1::SetEvmUlnConfig { .. } => "set_uln_evm",
        OperationV1::SetStellarExecutorConfig { .. } => "set_executor_stellar",
        OperationV1::SetEvmExecutorConfig { .. } => "set_executor_evm",
        OperationV1::SetStellarReceiveOptions { .. } => "set_options_stellar",
        OperationV1::SetEvmReceiveOptions { .. } => "set_options_evm",
        OperationV1::SetDefaultFee { .. } => "set_default_fee",
        OperationV1::SetDestinationFee { .. } => "set_destination_fee",
        OperationV1::SetFeeRecipient { .. } => "set_fee_recipient",
        OperationV1::SetMessageInspector { .. } => "set_message_inspector",
        OperationV1::SetInboundRateLimit { .. } => "set_inbound_rate_limit",
        OperationV1::SetOutboundRateLimit { .. } => "set_outbound_rate_limit",
        OperationV1::PauseEmergency => "pause_emergency",
        OperationV1::UnpauseEmergency => "unpause_emergency",
        OperationV1::SetTtlConfig { .. } => "set_ttl_config",
        OperationV1::FreezeTtlConfig { .. } => "freeze_ttl_config",
        OperationV1::ExtendInstanceTtl { .. } => "extend_instance_ttl",
        OperationV1::GrantRole { .. } => "grant_role",
        OperationV1::RevokeRole { .. } => "revoke_role",
        OperationV1::SetRoleAdmin { .. } => "set_role_admin",
        OperationV1::RemoveRoleAdmin { .. } => "remove_role_admin",
        OperationV1::SendLeg { .. } => "send_leg",
        OperationV1::CommitVerification { .. } => "commit_verification",
        OperationV1::ExecuteReceive { .. } => "execute_receive",
        OperationV1::ContainOutbound { .. } => "contain_outbound",
        OperationV1::RestoreOutbound { .. } => "restore_outbound",
        OperationV1::RestoreFootprint { .. } => "restore_footprint",
    }
    .to_string()
}

/// Checked authorization decision. Returns the decision or a policy refusal
/// naming the failing boundary: contract auth, unsupported class, or missing
/// source-account auth.
pub fn authorize(request: &AuthorizationRequestV1) -> Result<AuthorizationDecisionV1> {
    let spec = authorization_spec_v1(&request.operation);
    let (class, signer) = match &request.evidence {
        StellarSignerEvidenceV1::AddressContract { address } => {
            return Err(Error::Policy(format!(
                "address_contract_auth_unsupported_v1: {address}"
            )));
        }
        StellarSignerEvidenceV1::OftRoleOperator { address, .. } => {
            (StellarAuthorizationClass::OftRoleOperator, address)
        }
        StellarSignerEvidenceV1::OwnerDerivedAuthorizer { owner } => {
            (StellarAuthorizationClass::OwnerDerivedAuthorizer, owner)
        }
        StellarSignerEvidenceV1::OAppDelegate { delegate } => {
            (StellarAuthorizationClass::OAppDelegate, delegate)
        }
    };
    if !spec.allowed_classes.contains(&class) {
        return Err(Error::Policy(format!(
            "stellar_authorization_class_unsupported_v1: {:?} on {}",
            class, spec.operation
        )));
    }
    let signer: &str = signer.as_str();
    if spec.require_source_account && signer != request.source_account {
        return Err(Error::Policy(format!(
            "source_account_auth_required_v1: signer {signer} is not source account {}",
            request.source_account
        )));
    }
    Ok(AuthorizationDecisionV1 {
        operation: spec.operation,
        signer: signer.to_string(),
        class,
        source_account_auth: true,
    })
}

/// Checked adapter implementation of [`StellarAuthorizationAdapter`].
#[derive(Debug, Default)]
pub struct CheckedStellarAuthorization;

impl StellarAuthorizationAdapter for CheckedStellarAuthorization {
    fn authorize(&self, request: &AuthorizationRequestV1) -> Result<AuthorizationDecisionV1> {
        authorize(request)
    }
}

/// Observed on-chain role state for one route.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StellarRoleStateV1 {
    pub owner: String,
    pub operator: Option<String>,
    pub delegate: Option<String>,
}

/// One role or delegate drift between desired and observed state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StellarRoleDriftV1 {
    pub field: &'static str,
    pub desired: String,
    pub observed: String,
}

/// Compares desired owner/operator/delegate against observed role state.
/// The explicit OFT role operator entry must exist and match the desired
/// delegate; missing entries are drift.
pub fn role_drift(
    desired_owner: &str,
    desired_delegate: &str,
    observed: &StellarRoleStateV1,
) -> Vec<StellarRoleDriftV1> {
    let mut drift = Vec::new();
    if observed.owner != desired_owner {
        drift.push(StellarRoleDriftV1 {
            field: "owner",
            desired: desired_owner.to_string(),
            observed: observed.owner.clone(),
        });
    }
    let operator = observed
        .operator
        .clone()
        .unwrap_or_else(|| "missing".to_string());
    if operator != desired_delegate {
        drift.push(StellarRoleDriftV1 {
            field: "operator",
            desired: desired_delegate.to_string(),
            observed: operator,
        });
    }
    let delegate = observed
        .delegate
        .clone()
        .unwrap_or_else(|| "missing".to_string());
    if delegate != desired_delegate {
        drift.push(StellarRoleDriftV1 {
            field: "delegate",
            desired: desired_delegate.to_string(),
            observed: delegate,
        });
    }

    drift
}
/// Typed marker a qualified simulation returns when the source account's
/// Soroban footprint needs restoration before the original transaction can
/// proceed.
pub const RESTORATION_REQUIRED: &str = "restoration_required";

/// RPC-backed simulation outcome for the exact typed transaction a
/// proposal commits to. Every field is adapter-derived; no value is ever
/// fabricated or defaulted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StellarSimulationV1 {
    /// Base64 transaction envelope XDR of the assembled, broadcast-exact
    /// transaction.
    pub envelope_xdr: String,
    /// Hex network-bound hash of the assembled envelope.
    pub envelope_sha256: String,
    /// Base64 Soroban authorization entries; the adapter guarantees only
    /// source-account credential entries in v1.
    pub auth_entries: Vec<String>,
    /// Ledger sequence the simulation ran against.
    pub simulation_ledger: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StellarTransactionStatusV1 {
    pub status: String,
    pub ledger: Option<u32>,
    pub envelope_xdr: Option<String>,
    /// Successful contract events decoded from the transaction metadata.
    /// Source-send finalization authenticates LayerZero packet evidence from
    /// these events before releasing the authority reservation.
    pub contract_events: Vec<stellar_baselib::xdr::ContractEvent>,
}
pub fn envelope_transaction_hash(envelope_xdr: &str, network_passphrase: &str) -> Result<String> {
    use stellar_baselib::transaction::{Transaction, TransactionBehavior as _};

    std::panic::catch_unwind(|| {
        hex::encode(Transaction::from_xdr_envelope(envelope_xdr, network_passphrase).hash())
    })
    .map_err(|_| Error::InvalidInput("invalid Stellar transaction envelope XDR".into()))
}

/// Signs one assembled envelope with the named secret provider. The seed is
/// zeroized by the provider and never returned.
pub fn sign_envelope(
    envelope_xdr: &str,
    network_passphrase: &str,
    signer: &StellarSecretProviderV1,
) -> Result<String> {
    use stellar_baselib::{
        keypair::KeypairBehavior as _,
        transaction::{Transaction, TransactionBehavior as _},
        xdr::{Limits, WriteXdr as _},
    };

    let mut seed = [0u8; 32];
    seed.copy_from_slice(signer.seed());
    let keypair = Keypair::from_raw_ed25519_seed(&seed)
        .map_err(|error| Error::InvalidInput(format!("invalid Stellar seed: {error}")))?;
    let mut transaction = std::panic::catch_unwind(|| {
        Transaction::from_xdr_envelope(envelope_xdr, network_passphrase)
    })
    .map_err(|_| Error::InvalidInput("invalid Stellar envelope XDR".into()))?;
    transaction.sign(&[keypair]);
    transaction
        .to_envelope()
        .map_err(|error| Error::Chain(format!("signed Stellar envelope failed: {error}")))?
        .to_xdr_base64(Limits::none())
        .map_err(|error| Error::Chain(format!("signed Stellar envelope XDR failed: {error}")))
}

/// Observable Stellar chain boundary for proposal construction. Live reads
/// beyond passphrase/ledger/sequence remain pending the native-mutation
/// qualification gate and fail closed with a typed refusal — never a
/// fabricated value.
pub trait StellarChain {
    /// RPC-reported network passphrase.
    fn network_passphrase(&self) -> Result<String>;
    /// WASM code hash bound by the deployed contract instance.
    fn contract_code_hash(&self, _contract: &str) -> Result<String> {
        Err(Error::Chain(
            "contract code-hash readback is unsupported by this Stellar adapter".into(),
        ))
    }
    fn contract_code_hash_optional(&self, contract: &str) -> Result<Option<String>> {
        self.contract_code_hash(contract).map(Some)
    }
    /// Current expiration ledger for the contract-instance entry.
    fn contract_instance_live_until(&self, _contract: &str) -> Result<u32> {
        Err(Error::Chain(
            "contract instance TTL readback is unsupported by this Stellar adapter".into(),
        ))
    }
    /// Whether the exact WASM hash exists in network contract-code storage.
    fn wasm_installed(&self, _wasm_sha256: &str) -> Result<bool> {
        Err(Error::Chain(
            "WASM install readback is unsupported by this Stellar adapter".into(),
        ))
    }
    /// Live EndpointV2 `eid()` view for the configured endpoint.
    fn endpoint_eid(&self, endpoint: &str, source: &str) -> Result<u32>;
    /// Current account sequence as a decimal string.
    fn account_sequence(&self, account: &str) -> Result<String>;
    /// Simulates a read-only contract invocation using XDR-encoded arguments.
    fn invoke_view(
        &self,
        contract: &str,
        function: &str,
        args_xdr_hex: &[String],
        source: &str,
    ) -> Result<stellar_baselib::xdr::ScVal>;
    /// Reads a Stellar asset-contract balance.
    fn token_balance(&self, token: &str, address: &str, source: &str) -> Result<String>;
    /// Live signer weights for `account`, keyed by public key.
    fn account_signers(&self, account: &str) -> Result<std::collections::BTreeMap<String, u32>>;
    /// Required threshold weight for `level` (`low`/`medium`/`high`).
    fn account_threshold(&self, account: &str, level: &str) -> Result<u32>;
    /// Sequence number of the latest ledger known to the RPC.
    fn latest_ledger(&self) -> Result<u32>;
    /// Constructs, simulates, and assembles the exact typed Soroban
    /// transaction a proposal commits to, from the live source account,
    /// sequence, and ledger bounds. The returned envelope is the exact
    /// object that would be broadcast; simulation failures, footprint
    /// restoration, and non-source-account auth classes are typed
    /// refusals, never silent markers.
    fn simulate_transaction(
        &self,
        state: &crate::domain::RouteStateV1,
        operation: &OperationV1,
        source: &str,
        sequence: &str,
        min_ledger: u32,
        max_ledger: u32,
    ) -> Result<StellarSimulationV1>;
    /// Submits an already-signed assembled envelope.
    fn submit_transaction(&self, signed_envelope_xdr: &str) -> Result<String>;
    /// Polls transaction status by network hash.
    fn transaction_status(&self, transaction_hash: &str) -> Result<StellarTransactionStatusV1>;
}

/// Live Soroban RPC implementation of [`StellarChain`] over
/// `soroban-client 0.5.5`. Read methods with direct RPC support are
/// implemented; account-entry decoding (signers/thresholds) and endpoint
/// views wait on the pinned qualification gate.
pub struct HttpStellarChain {
    server: soroban_client::Server,
    artifact_root: Option<std::path::PathBuf>,
}

const MAX_SIMULATION_XDR_BYTES: usize = 1_048_576;
const MAX_SIMULATION_AUTH_ENTRIES: usize = 64;

fn checked_simulation_result(
    response: &soroban_client::soroban_rpc::SimulateTransactionResponse,
) -> Result<(
    stellar_baselib::xdr::ScVal,
    Vec<stellar_baselib::xdr::SorobanAuthorizationEntry>,
)> {
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
    use stellar_baselib::xdr::{Limits, ReadXdr as _, ScVal, SorobanAuthorizationEntry};

    let raw = serde_json::to_value(response)?;
    for field in ["transactionData"] {
        if raw
            .get(field)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value.len() > MAX_SIMULATION_XDR_BYTES * 2)
        {
            return Err(Error::Chain(format!(
                "Stellar simulation {field} exceeds the bounded parser limit"
            )));
        }
    }
    let results = raw
        .get("results")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Error::Chain("Stellar simulation returned no results".into()))?;
    if results.len() != 1 {
        return Err(Error::Chain(format!(
            "Stellar simulation returned {} results; expected exactly one",
            results.len()
        )));
    }
    let decode = |value: &str, label: &str| {
        if value.len() > MAX_SIMULATION_XDR_BYTES * 2 {
            return Err(Error::Chain(format!(
                "Stellar simulation {label} exceeds the bounded parser limit"
            )));
        }
        let bytes = BASE64_STANDARD
            .decode(value)
            .map_err(|_| Error::Chain(format!("Stellar simulation {label} is not base64")))?;
        if bytes.len() > MAX_SIMULATION_XDR_BYTES {
            return Err(Error::Chain(format!(
                "Stellar simulation {label} exceeds the bounded parser limit"
            )));
        }
        Ok(bytes)
    };
    let result = &results[0];
    let value = result
        .get("xdr")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Chain("Stellar simulation result omitted XDR".into()))?;
    let value = ScVal::from_xdr(decode(value, "result XDR")?, Limits::none()).map_err(|error| {
        Error::Chain(format!("Stellar simulation result XDR is invalid: {error}"))
    })?;
    let auth = result
        .get("auth")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Error::Chain("Stellar simulation result omitted authorization".into()))?;
    if auth.len() > MAX_SIMULATION_AUTH_ENTRIES {
        return Err(Error::Chain(
            "Stellar simulation returned too many authorization entries".into(),
        ));
    }
    let auth = auth
        .iter()
        .map(|value| {
            let value = value.as_str().ok_or_else(|| {
                Error::Chain("Stellar simulation authorization is not a string".into())
            })?;
            SorobanAuthorizationEntry::from_xdr(decode(value, "authorization XDR")?, Limits::none())
                .map_err(|error| {
                    Error::Chain(format!(
                        "Stellar simulation authorization XDR is invalid: {error}"
                    ))
                })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((value, auth))
}

impl HttpStellarChain {
    pub fn new(url: &str) -> Result<Self> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|error| Error::InvalidInput(format!("invalid Stellar RPC URL: {error}")))?;
        if parsed.scheme() != "https" {
            return Err(Error::InvalidInput(
                "Stellar RPC URL must be https in v1".into(),
            ));
        }
        let mut options = soroban_client::Options::default();
        options.headers = crate::config::rpc_headers()
            .into_iter()
            .map(|(name, value)| (name, value.to_string()))
            .collect();
        let server = soroban_client::Server::new(url, options)
            .map_err(|error| Error::Chain(format!("stellar rpc connect failed: {error}")))?;
        Ok(Self {
            server,
            artifact_root: None,
        })
    }

    #[must_use]
    pub fn with_artifact_root(mut self, root: &std::path::Path) -> Self {
        self.artifact_root = Some(root.to_path_buf());
        self
    }

    fn account_entry(&self, account: &str) -> Result<stellar_baselib::xdr::AccountEntry> {
        use stellar_baselib::xdr::{LedgerEntryData, LedgerKey, LedgerKeyAccount};

        let account_id = Keypair::from_public_key(account)
            .map_err(|error| Error::InvalidInput(format!("invalid Stellar account: {error}")))?
            .xdr_account_id();
        let response = rpc(crate::block_on(self.server.get_ledger_entries(vec![
            LedgerKey::Account(LedgerKeyAccount { account_id }),
        ]))?)?;
        let entry = response
            .entries
            .and_then(|mut entries| entries.pop())
            .ok_or_else(|| Error::Chain(format!("Stellar account not found: {account}")))?;
        let data = std::panic::catch_unwind(|| entry.to_data())
            .map_err(|_| Error::Chain("stellar RPC returned malformed account XDR".into()))?;
        match data {
            LedgerEntryData::Account(account) => Ok(account),
            _ => Err(Error::Chain(
                "stellar RPC returned a non-account ledger entry".into(),
            )),
        }
    }

    fn simulate_view(
        &self,
        contract: &str,
        function: &str,
        args: Vec<stellar_baselib::xdr::ScVal>,
        source: &str,
    ) -> Result<stellar_baselib::xdr::ScVal> {
        use stellar_baselib::{
            account::{Account, AccountBehavior as _},
            operation::Operation,
            transaction_builder::{TransactionBuilder, TransactionBuilderBehavior as _},
            xdr::LedgerBounds,
        };

        let sequence = self.account_sequence(source)?;
        let mut account = Account::new(source, &sequence)
            .map_err(|error| Error::InvalidInput(format!("invalid source account: {error}")))?;
        let operation = Operation::new()
            .invoke_contract(contract, function, args, None)
            .map_err(|error| Error::InvalidInput(format!("invalid contract view: {error:?}")))?;
        let latest = self.latest_ledger()?;
        let mut builder = TransactionBuilder::new(&mut account, &self.network_passphrase()?, None);
        builder
            .fee(100u32)
            .set_ledger_bounds(LedgerBounds {
                min_ledger: latest,
                max_ledger: latest.saturating_add(1_000),
            })
            .add_operation(operation);
        let transaction = builder.build_for_simulation();
        let response = rpc(crate::block_on(
            self.server.simulate_transaction(&transaction, None),
        )?)?;
        if let Some(error) = response.error {
            return Err(Error::Chain(format!(
                "Stellar contract view failed: {error}"
            )));
        }
        checked_simulation_result(&response).map(|(value, _)| value)
    }
}

fn rpc<T>(result: std::result::Result<T, soroban_client::error::Error>) -> Result<T> {
    result.map_err(|error| Error::Chain(format!("stellar rpc call failed: {error:?}")))
}

impl StellarChain for HttpStellarChain {
    fn network_passphrase(&self) -> Result<String> {
        let response = rpc(crate::block_on(self.server.get_network())?)?;
        response
            .passphrase
            .ok_or_else(|| Error::Chain("stellar rpc omitted network passphrase".into()))
    }

    fn contract_code_hash(&self, contract: &str) -> Result<String> {
        use stellar_baselib::xdr::{
            ContractDataDurability, ContractExecutable, ContractId, Hash, LedgerEntryData,
            LedgerKey, LedgerKeyContractData, ScAddress, ScVal,
        };
        use stellar_strkey::Strkey;

        let id = match Strkey::from_str(contract) {
            Ok(Strkey::Contract(id)) => id.0,
            _ => {
                return Err(Error::InvalidInput(format!(
                    "Stellar contract address must be a C... strkey: {contract}"
                )))
            }
        };
        let response = rpc(crate::block_on(self.server.get_ledger_entries(vec![
            LedgerKey::ContractData(LedgerKeyContractData {
                contract: ScAddress::Contract(ContractId(Hash(id))),
                key: ScVal::LedgerKeyContractInstance,
                durability: ContractDataDurability::Persistent,
            }),
        ]))?)?;
        let entry = response
            .entries
            .and_then(|mut entries| entries.pop())
            .ok_or_else(|| Error::Chain(format!("Stellar contract not found: {contract}")))?;
        let data = std::panic::catch_unwind(|| entry.to_data()).map_err(|_| {
            Error::Chain("stellar RPC returned malformed contract instance XDR".into())
        })?;
        let LedgerEntryData::ContractData(contract_data) = data else {
            return Err(Error::Chain(
                "stellar RPC returned a non-contract-data ledger entry".into(),
            ));
        };
        if contract_data.key != ScVal::LedgerKeyContractInstance {
            return Err(Error::Chain(
                "stellar RPC returned a contract-data entry with an unexpected key".into(),
            ));
        }
        match contract_data.val {
            ScVal::ContractInstance(instance) => match instance.executable {
                ContractExecutable::Wasm(hash) => Ok(hex::encode(hash.0)),
                ContractExecutable::StellarAsset => Err(Error::Chain(format!(
                    "Stellar contract {contract} is an asset contract and has no WASM code"
                ))),
            },
            _ => Err(Error::Chain(
                "stellar RPC returned an instance entry without an instance value".into(),
            )),
        }
    }

    fn contract_code_hash_optional(&self, contract: &str) -> Result<Option<String>> {
        match self.contract_code_hash(contract) {
            Ok(hash) => Ok(Some(hash)),
            Err(Error::Chain(message))
                if message == format!("Stellar contract not found: {contract}") =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn contract_instance_live_until(&self, contract: &str) -> Result<u32> {
        use stellar_baselib::xdr::{
            ContractDataDurability, ContractId, Hash, LedgerKey, LedgerKeyContractData, ScAddress,
            ScVal,
        };
        use stellar_strkey::Strkey;

        let id = match Strkey::from_str(contract) {
            Ok(Strkey::Contract(id)) => id.0,
            _ => {
                return Err(Error::InvalidInput(format!(
                    "Stellar contract address must be a C... strkey: {contract}"
                )))
            }
        };
        let response = rpc(crate::block_on(self.server.get_ledger_entries(vec![
            LedgerKey::ContractData(LedgerKeyContractData {
                contract: ScAddress::Contract(ContractId(Hash(id))),
                key: ScVal::LedgerKeyContractInstance,
                durability: ContractDataDurability::Persistent,
            }),
        ]))?)?;
        response
            .entries
            .and_then(|mut entries| entries.pop())
            .and_then(|entry| entry.live_until_ledger_seq)
            .ok_or_else(|| Error::Chain(format!("Stellar contract TTL not found: {contract}")))
    }

    fn wasm_installed(&self, wasm_sha256: &str) -> Result<bool> {
        use stellar_baselib::xdr::{Hash, LedgerKey, LedgerKeyContractCode};

        let hash: [u8; 32] = hex::decode(wasm_sha256.trim_start_matches("0x"))
            .map_err(|_| Error::InvalidInput("Stellar WASM hash is not hex".into()))?
            .try_into()
            .map_err(|_| Error::InvalidInput("Stellar WASM hash must be 32 bytes".into()))?;
        let response = rpc(crate::block_on(self.server.get_ledger_entries(vec![
            LedgerKey::ContractCode(LedgerKeyContractCode { hash: Hash(hash) }),
        ]))?)?;
        Ok(response.entries.is_some_and(|entries| !entries.is_empty()))
    }

    fn endpoint_eid(&self, endpoint: &str, source: &str) -> Result<u32> {
        use stellar_baselib::{
            account::{Account, AccountBehavior as _},
            operation::Operation,
            transaction_builder::{TransactionBuilder, TransactionBuilderBehavior as _},
            xdr::{LedgerBounds, ScVal},
        };

        let sequence = self.account_sequence(source)?;
        let mut account = Account::new(source, &sequence)
            .map_err(|error| Error::InvalidInput(format!("invalid source account: {error}")))?;
        let operation = Operation::new()
            .invoke_contract(endpoint, "eid", vec![], None)
            .map_err(|error| {
                Error::InvalidInput(format!("invalid endpoint invocation: {error:?}"))
            })?;
        let latest = self.latest_ledger()?;
        let mut builder = TransactionBuilder::new(&mut account, &self.network_passphrase()?, None);
        builder
            .fee(100u32)
            .set_ledger_bounds(LedgerBounds {
                min_ledger: latest,
                max_ledger: latest.saturating_add(1_000),
            })
            .add_operation(operation);
        let transaction = builder.build_for_simulation();
        let simulation = rpc(crate::block_on(
            self.server.simulate_transaction(&transaction, None),
        )?)?;
        if let Some(error) = simulation.error {
            return Err(Error::Chain(format!(
                "endpoint eid simulation failed: {error}"
            )));
        }
        match checked_simulation_result(&simulation)?.0 {
            ScVal::U32(eid) => Ok(eid),
            _ => Err(Error::Chain(
                "endpoint eid simulation returned a non-u32 result".into(),
            )),
        }
    }

    fn account_sequence(&self, account: &str) -> Result<String> {
        use stellar_baselib::account::AccountBehavior as _;
        let loaded = rpc(crate::block_on(self.server.get_account(account))?)?;
        Ok(loaded.sequence_number())
    }

    fn invoke_view(
        &self,
        contract: &str,
        function: &str,
        args_xdr_hex: &[String],
        source: &str,
    ) -> Result<stellar_baselib::xdr::ScVal> {
        use stellar_baselib::xdr::{Limits, ReadXdr as _, ScVal};

        let args = args_xdr_hex
            .iter()
            .map(|encoded| {
                let bytes = hex::decode(encoded).map_err(|error| {
                    Error::InvalidInput(format!("invalid view argument hex: {error}"))
                })?;
                ScVal::from_xdr(bytes, Limits::none()).map_err(|error| {
                    Error::InvalidInput(format!("invalid view argument XDR: {error}"))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.simulate_view(contract, function, args, source)
    }

    fn token_balance(&self, token: &str, address: &str, source: &str) -> Result<String> {
        use stellar_baselib::{
            address::{Address, AddressTrait as _},
            xdr::ScVal,
        };

        let address = Address::new(address)
            .map_err(|error| Error::InvalidInput(format!("invalid balance address: {error}")))?;
        let address = address
            .to_sc_val()
            .map_err(|error| Error::InvalidInput(error.into()))?;
        match self.simulate_view(token, "balance", vec![address], source)? {
            ScVal::I128(parts) => {
                let value = (i128::from(parts.hi) << 64) | i128::from(parts.lo);
                Ok(value.to_string())
            }
            _ => Err(Error::Chain(
                "Stellar balance view returned a non-i128 result".into(),
            )),
        }
    }
    fn account_signers(&self, account: &str) -> Result<std::collections::BTreeMap<String, u32>> {
        use stellar_baselib::xdr::SignerKey;

        let entry = self.account_entry(account)?;
        let mut signers = std::collections::BTreeMap::new();
        signers.insert(account.to_string(), u32::from(entry.thresholds.0[0]));
        for signer in entry.signers.iter() {
            if let SignerKey::Ed25519(key) = &signer.key {
                let public = format!(
                    "{}",
                    Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey(key.0))
                );
                signers.insert(public, signer.weight);
            }
        }
        Ok(signers)
    }

    fn account_threshold(&self, account: &str, level: &str) -> Result<u32> {
        let index = match level {
            "low" => 1,
            "medium" => 2,
            "high" => 3,
            _ => {
                return Err(Error::InvalidInput(format!(
                    "unknown Stellar threshold level: {level}"
                )))
            }
        };
        Ok(u32::from(self.account_entry(account)?.thresholds.0[index]))
    }

    fn latest_ledger(&self) -> Result<u32> {
        let response = rpc(crate::block_on(self.server.get_latest_ledger())?)?;
        Ok(response.sequence)
    }

    fn simulate_transaction(
        &self,
        state: &crate::domain::RouteStateV1,
        operation: &OperationV1,
        source_account: &str,
        sequence: &str,
        min_ledger: u32,
        max_ledger: u32,
    ) -> Result<StellarSimulationV1> {
        use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
        use sha2::{Digest as _, Sha256};
        use soroban_client::transaction::assemble_transaction;
        use stellar_baselib::transaction::TransactionBehavior as _;
        use stellar_baselib::{
            account::{Account, AccountBehavior as _},
            operation::Operation,
            transaction_builder::{TransactionBuilder, TransactionBuilderBehavior as _},
            xdr::{LedgerBounds, Limits, ReadXdr as _, ScVal, WriteXdr as _},
        };
        let operation = match operation {
            OperationV1::InstallStellarWasm { wasm_sha256 } => {
                let root = self.artifact_root.as_ref().ok_or_else(|| {
                    Error::Custody("Stellar artifact root is not configured".into())
                })?;
                let code = std::fs::read(
                    root.join(".artifacts")
                        .join(format!("stellar-{wasm_sha256}.wasm")),
                )?;
                if hex::encode(Sha256::digest(&code)) != *wasm_sha256 {
                    return Err(Error::Custody(
                        "preserved Stellar WASM digest mismatch".into(),
                    ));
                }
                Operation::new().upload_wasm(&code, None).map_err(|error| {
                    Error::InvalidInput(format!("invalid WASM upload: {error:?}"))
                })?
            }
            OperationV1::DeployStellarOft {
                deployer,
                salt,
                wasm_sha256,
                token,
                shared_decimals,
                endpoint,
                delegate,
                expected_address,
            } => {
                let salt: [u8; 32] = hex::decode(salt)
                    .map_err(|_| Error::InvalidInput("Stellar deployment salt is not hex".into()))?
                    .try_into()
                    .map_err(|_| {
                        Error::InvalidInput("Stellar deployment salt must be 32 bytes".into())
                    })?;
                let wasm_hash: [u8; 32] = hex::decode(wasm_sha256)
                    .map_err(|_| Error::InvalidInput("Stellar WASM hash is not hex".into()))?
                    .try_into()
                    .map_err(|_| {
                        Error::InvalidInput("Stellar WASM hash must be 32 bytes".into())
                    })?;
                let derived = crate::codec::derive_stellar_contract_address(
                    &state.identity.stellar_passphrase,
                    deployer,
                    &salt,
                )?;
                if &derived != expected_address || source_account != deployer {
                    return Err(Error::Custody(
                        "Stellar deployment address or source binding mismatch".into(),
                    ));
                }
                let oft_type = ScVal::Vec(Some(stellar_baselib::xdr::ScVec(
                    stellar_baselib::xdr::VecM::try_from(vec![
                        stellar_baselib::xdr::ScVal::Symbol(stellar_baselib::xdr::ScSymbol(
                            stellar_baselib::xdr::StringM::try_from(b"LockUnlock".to_vec())
                                .map_err(|error| {
                                    Error::InvalidInput(format!("invalid OFT type: {error}"))
                                })?,
                        )),
                    ])
                    .map_err(|error| Error::InvalidInput(format!("invalid OFT type: {error}")))?,
                )));
                Operation::new()
                    .create_contract(
                        deployer,
                        wasm_hash,
                        Some(salt),
                        None,
                        vec![
                            crate::layerzero::stellar_address(token)?,
                            ScVal::U32(u32::from(*shared_decimals)),
                            oft_type,
                            crate::layerzero::stellar_address(endpoint)?,
                            crate::layerzero::stellar_address(delegate)?,
                        ],
                    )
                    .map_err(|error| {
                        Error::InvalidInput(format!(
                            "invalid Stellar contract deployment: {error:?}"
                        ))
                    })?
            }
            _ => {
                let target = match operation {
                    OperationV1::CommitVerification {
                        vm: crate::domain::Vm::Stellar,
                        message,
                    } => &message.current_receive_library,
                    OperationV1::SetStellarSendLibrary { .. }
                    | OperationV1::SetStellarReceiveLibrary { .. }
                    | OperationV1::RemoveStellarReceiveLibraryTimeout { .. }
                    | OperationV1::SetStellarUlnConfig { .. }
                    | OperationV1::SetStellarExecutorConfig { .. } => {
                        &state.identity.stellar_endpoint
                    }
                    _ => state.contracts.get("stellar_oft").ok_or_else(|| {
                        Error::Custody("route has no recorded stellar_oft contract".into())
                    })?,
                };
                let invocation =
                    crate::layerzero::build_stellar_operation_for_route(state, operation)?;
                let args = invocation
                    .args_xdr_hex
                    .iter()
                    .map(|encoded| {
                        let bytes = hex::decode(encoded).map_err(|error| {
                            Error::InvalidInput(format!("invalid Stellar argument hex: {error}"))
                        })?;
                        ScVal::from_xdr(bytes, Limits::none()).map_err(|error| {
                            Error::InvalidInput(format!("invalid Stellar argument XDR: {error}"))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Operation::new()
                    .invoke_contract(target, &invocation.function, args, None)
                    .map_err(|error| {
                        Error::InvalidInput(format!("invalid Stellar invocation: {error:?}"))
                    })?
            }
        };
        let mut account = Account::new(source_account, sequence)
            .map_err(|error| Error::InvalidInput(format!("invalid source account: {error}")))?;
        let mut builder = TransactionBuilder::new(&mut account, &self.network_passphrase()?, None);
        builder
            .fee(100u32)
            .set_ledger_bounds(LedgerBounds {
                min_ledger,
                max_ledger,
            })
            .add_operation(operation);
        let transaction = builder.build_for_simulation();
        let simulation = rpc(crate::block_on(
            self.server.simulate_transaction(&transaction, None),
        )?)?;
        if let Some(error) = &simulation.error {
            return Err(Error::Chain(format!(
                "Stellar transaction simulation failed: {error}"
            )));
        }
        if simulation.to_restore_transaction_data().is_some() {
            return Err(Error::Policy(RESTORATION_REQUIRED.into()));
        }
        let (_, auth) = checked_simulation_result(&simulation)?;
        if auth.iter().any(|entry| {
            !matches!(
                entry.credentials,
                stellar_baselib::xdr::SorobanCredentials::SourceAccount
            )
        }) {
            return Err(Error::Policy("address_contract_auth_unsupported_v1".into()));
        }
        let simulation_ledger = simulation.latest_ledger;
        assemble_transaction(&transaction, simulation)
            .map_err(|error| Error::Chain(format!("Stellar assembly failed: {error:?}")))?;
        let envelope = transaction
            .to_envelope()
            .map_err(|error| Error::Chain(format!("Stellar envelope failed: {error}")))?;
        let envelope_xdr = envelope
            .to_xdr_base64(Limits::none())
            .map_err(|error| Error::Chain(format!("Stellar envelope XDR failed: {error}")))?;
        let envelope_bytes = BASE64_STANDARD.decode(&envelope_xdr).map_err(|error| {
            Error::Chain(format!("Stellar envelope base64 decode failed: {error}"))
        })?;
        Ok(StellarSimulationV1 {
            envelope_sha256: hex::encode(Sha256::digest(envelope_bytes)),
            envelope_xdr,
            auth_entries: auth
                .iter()
                .map(|entry| {
                    entry.to_xdr_base64(Limits::none()).map_err(|error| {
                        Error::Chain(format!("Stellar authorization XDR failed: {error}"))
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            simulation_ledger,
        })
    }

    fn submit_transaction(&self, signed_envelope_xdr: &str) -> Result<String> {
        use stellar_baselib::transaction::{Transaction, TransactionBehavior as _};

        let passphrase = self.network_passphrase()?;
        let transaction = std::panic::catch_unwind(|| {
            Transaction::from_xdr_envelope(signed_envelope_xdr, &passphrase)
        })
        .map_err(|_| Error::InvalidInput("invalid signed Stellar envelope XDR".into()))?;
        let response = rpc(crate::block_on(self.server.send_transaction(transaction))?)?;
        match response.status {
            soroban_client::soroban_rpc::SendTransactionStatus::Pending => Ok(response.hash),
            status => Err(Error::Chain(format!(
                "Stellar transaction submission rejected with status {status:?}"
            ))),
        }
    }

    fn transaction_status(&self, transaction_hash: &str) -> Result<StellarTransactionStatusV1> {
        use stellar_baselib::xdr::{Limits, WriteXdr as _};

        let response = rpc(crate::block_on(
            self.server.get_transaction(transaction_hash),
        )?)?;
        let status = match response.status {
            soroban_client::soroban_rpc::TransactionStatus::Success => "success",
            soroban_client::soroban_rpc::TransactionStatus::NotFound => "not_found",
            soroban_client::soroban_rpc::TransactionStatus::Failed => "failed",
        };
        let envelope_xdr = response
            .to_envelope()
            .map(|envelope| envelope.to_xdr_base64(Limits::none()))
            .transpose()
            .map_err(|error| {
                Error::Chain(format!("transaction envelope encode failed: {error}"))
            })?;
        let contract_events = response
            .to_events()
            .map(|(_, operation_events)| operation_events.into_iter().flatten().collect())
            .unwrap_or_default();
        Ok(StellarTransactionStatusV1 {
            status: status.into(),
            ledger: response.ledger,
            envelope_xdr,
            contract_events,
        })
    }
}

#[cfg(test)]
mod simulation_parser_tests {
    use super::{checked_simulation_result, MAX_SIMULATION_XDR_BYTES};
    use stellar_baselib::xdr::{Limits, ScVal, WriteXdr as _};

    fn response(
        results: serde_json::Value,
    ) -> soroban_client::soroban_rpc::SimulateTransactionResponse {
        serde_json::from_value(serde_json::json!({
            "latestLedger": 1,
            "minResourceFee": "0",
            "error": null,
            "results": results,
            "transactionData": null,
            "restorePreamble": null,
            "events": null,
            "stateChanges": null
        }))
        .expect("response")
    }

    #[test]
    fn simulation_parser_rejects_missing_multiple_malformed_and_oversized_results() {
        assert!(checked_simulation_result(&response(serde_json::json!([]))).is_err());
        let valid = ScVal::U32(7).to_xdr_base64(Limits::none()).unwrap();
        let one = serde_json::json!({"auth": [], "xdr": valid});
        assert!(
            checked_simulation_result(&response(serde_json::json!([one.clone(), one]))).is_err()
        );
        assert!(checked_simulation_result(&response(serde_json::json!([{
            "auth": [],
            "xdr": "not base64"
        }])))
        .is_err());
        assert!(checked_simulation_result(&response(serde_json::json!([{
            "auth": [],
            "xdr": "A".repeat(MAX_SIMULATION_XDR_BYTES * 2 + 1)
        }])))
        .is_err());
    }
}
