use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use alloy::network::Ethereum;
use alloy::primitives::{keccak256, Address, Bytes, TxKind, B256, U256};
use alloy::providers::{Provider, ProviderBuilder, RootProvider};
use alloy::rpc::types::{TransactionInput, TransactionRequest};
use alloy::signers::local::PrivateKeySigner;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::domain::OperationV1;
use crate::error::{Error, Result};

/// Parses a strict 20-byte `0x`-prefixed EVM address at the trust boundary.
pub fn parse_address(value: &str) -> Result<Address> {
    let hex = value
        .strip_prefix("0x")
        .ok_or_else(|| Error::InvalidInput("EVM address must be 0x-prefixed".into()))?;
    if hex.len() != 40 {
        return Err(Error::InvalidInput(format!(
            "EVM address must be 40 hex characters, got {}",
            hex.len()
        )));
    }
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::InvalidInput("EVM address must be hex".into()));
    }
    let decoded = hex::decode(hex)
        .map_err(|error| Error::InvalidInput(format!("invalid EVM address: {error}")))?;
    Ok(Address::from_slice(&decoded))
}

/// Derives the plain `CREATE` address for `deployer` at `nonce`:
/// `keccak256(rlp([deployer, nonce]))[12..]`.
#[must_use]
pub fn derive_create_address(deployer: Address, nonce: u64) -> Address {
    deployer.create(nonce)
}

/// Lowercase fixed `0x`-hex rendering used by every canonical artifact.
#[must_use]
pub fn canonical_address(address: Address) -> String {
    format!("{address:#x}")
}

/// EVM-side deployment binding for `DeployEvmOft`: derives the plain `CREATE`
/// address from deployer and nonce and binds the wrapper initializer arguments.
/// `init_code_hash` is `None` until the pinned wrapper artifact is actually
/// built; verification fails closed on `None`.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct DeployEvmOftBindingV1 {
    pub deployer: String,
    pub nonce: u64,
    pub derived_address: String,
    /// keccak256 of the wrapper init code; `None` means not built yet.
    pub init_code_hash: Option<String>,
    pub name: String,
    pub symbol: String,
    pub endpoint: String,
    pub owner_delegate: String,
}

impl DeployEvmOftBindingV1 {
    /// Binds a deployment. `init_code_hash` is carried verbatim and stays
    /// `None` until the wrapper artifact is built; never fabricate a hash.
    pub fn bind(
        deployer: Address,
        nonce: u64,
        init_code_hash: Option<B256>,
        name: String,
        symbol: String,
        endpoint: Address,
        owner_delegate: Address,
    ) -> Result<Self> {
        if name.trim().is_empty() {
            return Err(Error::InvalidInput("wrapper name must not be empty".into()));
        }
        if symbol.trim().is_empty() {
            return Err(Error::InvalidInput(
                "wrapper symbol must not be empty".into(),
            ));
        }
        let derived_address = derive_create_address(deployer, nonce);
        Ok(Self {
            deployer: canonical_address(deployer),
            nonce,
            derived_address: canonical_address(derived_address),
            init_code_hash: init_code_hash.map(canonical_b256),
            name,
            symbol,
            endpoint: canonical_address(endpoint),
            owner_delegate: canonical_address(owner_delegate),
        })
    }

    /// Canonical-JSON intent digest bound into plan/proposal/journal.
    pub fn intent_sha256(&self) -> Result<String> {
        crate::canonical_sha256(self)
    }
}

/// Canonical `B256` rendering.
#[must_use]
pub fn canonical_b256(value: alloy::primitives::B256) -> String {
    format!("{value:#x}")
}

/// EVM-side proposal data: the exact transaction identity a proposal commits
/// to. Digests cover the canonical JSON of the populated fields; an unset gas
/// limit serializes as `null` rather than an invented value.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct EvmProposalDataV1 {
    pub chain_id: u64,
    pub target: String,
    pub value_wei: u128,
    pub nonce: u64,
    pub calldata: String,
    pub gas_limit: Option<u64>,
}

impl EvmProposalDataV1 {
    pub fn build(
        chain_id: u64,
        target: Address,
        value_wei: u128,
        nonce: u64,
        calldata: Vec<u8>,
        gas_limit: Option<u64>,
    ) -> Self {
        Self {
            chain_id,
            target: canonical_address(target),
            value_wei,
            nonce,
            calldata: format!("0x{}", hex::encode(calldata)),
            gas_limit,
        }
    }

    /// Canonical-JSON digest of the proposal data.
    pub fn proposal_sha256(&self) -> Result<String> {
        crate::canonical_sha256(self)
    }
}

/// Validates a keystore or secret-provider file path: must be a regular file,
/// must not be a symlink, and must have mode 0600.
pub fn validate_secret_file(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_symlink() {
        return Err(Error::Custody(format!(
            "secret file {} must not be a symlink",
            path.display()
        )));
    }
    if !metadata.is_file() {
        return Err(Error::Custody(format!(
            "secret file {} must be a regular file",
            path.display()
        )));
    }
    let mode = metadata.permissions().mode();
    if mode & 0o777 != 0o600 {
        return Err(Error::Custody(format!(
            "secret file {} must have mode 0600, got {:o}",
            path.display(),
            mode & 0o777
        )));
    }
    Ok(())
}

/// Reads a password file after validating its path; the value is zeroized on
/// drop and never enters logs or error strings.
pub fn read_password_file(path: &Path) -> Result<Zeroizing<String>> {
    validate_secret_file(path)?;
    let raw = std::fs::read(path)?;
    let text = String::from_utf8(raw)
        .map_err(|_| Error::InvalidInput("password file must be UTF-8".into()))?;
    let trimmed = text.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err(Error::InvalidInput(
            "password file must not be empty".into(),
        ));
    }
    Ok(Zeroizing::new(trimmed.to_string()))
}

/// Qualifies an encrypted Foundry V3 keystore file and decrypts it into a
/// local signer. Qualification is fail-closed: regular file, no symlink,
/// mode 0600, and the decrypted signer identity must equal `expected`.
/// The password never enters the error string.
pub fn keystore_signer(path: &Path, password: &str, expected: Address) -> Result<PrivateKeySigner> {
    validate_secret_file(path)?;
    let signer = PrivateKeySigner::decrypt_keystore(path, password)
        .map_err(|error| Error::Chain(format!("keystore decrypt failed: {error}")))?;
    if signer.address() != expected {
        return Err(Error::Custody(format!(
            "keystore {} decrypts to signer {} but expected identity {}; refusing to sign",
            path.display(),
            canonical_address(signer.address()),
            canonical_address(expected),
        )));
    }
    Ok(signer)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignedEvmTransactionV1 {
    pub encoded: Vec<u8>,
    pub transaction_hash: String,
}

/// Signs the exact direct EIP-1559 transaction bound by an executable plan.
/// Safe plans are deliberately not converted into direct account sends.
pub fn sign_eip1559(
    binding: &crate::domain::EvmPlanBindingV1,
    signer: &PrivateKeySigner,
) -> Result<SignedEvmTransactionV1> {
    use alloy::{
        consensus::{SignableTransaction as _, TxEip1559, TxEnvelope},
        eips::eip2718::Encodable2718 as _,
        signers::Signer as _,
    };

    if binding.safe.is_some() {
        return Err(Error::Policy(
            "Safe proposals require external Safe execution".into(),
        ));
    }
    let bound_sender = parse_address(&binding.sender)?;
    if signer.address() != bound_sender {
        return Err(Error::Custody(format!(
            "EVM signer {} differs from plan sender {}; refusing to sign",
            canonical_address(signer.address()),
            canonical_address(bound_sender),
        )));
    }
    let transaction = TxEip1559 {
        chain_id: binding
            .chain_id
            .parse()
            .map_err(|error| Error::InvalidInput(format!("invalid chain id: {error}")))?,
        nonce: binding
            .nonce
            .parse()
            .map_err(|error| Error::InvalidInput(format!("invalid nonce: {error}")))?,
        gas_limit: binding
            .gas_limit
            .parse()
            .map_err(|error| Error::InvalidInput(format!("invalid gas limit: {error}")))?,
        max_fee_per_gas: binding
            .max_fee_per_gas_wei
            .parse()
            .map_err(|error| Error::InvalidInput(format!("invalid max fee per gas: {error}")))?,
        max_priority_fee_per_gas: binding.max_priority_fee_per_gas_wei.parse().map_err(
            |error| Error::InvalidInput(format!("invalid priority fee per gas: {error}")),
        )?,
        to: if binding.target == "create" {
            TxKind::Create
        } else {
            TxKind::Call(parse_address(&binding.target)?)
        },
        value: U256::from_str_radix(&binding.value, 10)
            .map_err(|error| Error::InvalidInput(format!("invalid EVM value: {error}")))?,
        access_list: Default::default(),
        input: Bytes::from(
            hex::decode(binding.calldata.trim_start_matches("0x"))
                .map_err(|error| Error::InvalidInput(format!("invalid calldata: {error}")))?,
        ),
    };
    let signature = crate::block_on(signer.sign_hash(&transaction.signature_hash()))?
        .map_err(|error| Error::Chain(format!("EVM transaction signing failed: {error}")))?;
    let envelope: TxEnvelope = transaction.into_signed(signature).into();
    let encoded = envelope.encoded_2718();
    Ok(SignedEvmTransactionV1 {
        transaction_hash: format!("{:#x}", keccak256(&encoded)),
        encoded,
    })
}

/// Observable EVM chain boundary for preflight, verification, and simulation
/// reads. Production implementations use Alloy; fakes serve command tests.
#[async_trait::async_trait]
pub trait EvmChain: Send + Sync {
    /// RPC-reported chain ID.
    async fn chain_id(&self) -> Result<u64>;
    /// Contract code bytes at `address` (empty for EOAs and empty accounts).
    async fn code(&self, address: Address) -> Result<Vec<u8>>;
    /// `eth_call` against the latest state; returns raw return bytes.
    async fn call(&self, to: Address, calldata: Vec<u8>) -> Result<Vec<u8>>;
    /// Live EndpointV2 `eid()` view.
    async fn endpoint_eid(&self, endpoint: &str) -> Result<u32>;
    /// Pending account nonce for the transaction sender.
    async fn account_nonce(&self, address: Address) -> Result<u64>;
    /// Safe `getThreshold()` and `nonce()` when `safe` is a Safe proxy.
    async fn safe_state(&self, safe: Address) -> Result<Option<(u32, String)>>;
    /// Latest confirmed block number known to the RPC.
    async fn latest_block(&self) -> Result<u64> {
        Err(Error::Chain(
            "EVM latest-block readback is unsupported by this adapter".into(),
        ))
    }
    /// Simulates the exact typed transaction (sender, target, value,
    /// calldata) against latest state and returns the RPC-derived gas and
    /// fee policy. Estimation failure is a typed refusal; no invented
    /// constants.
    async fn estimate_transaction(
        &self,
        from: Address,
        to: Address,
        value: U256,
        calldata: Vec<u8>,
    ) -> Result<EvmSimulationV1>;
    /// Resolves and validates preserved creation bytecode, then appends the
    /// exact constructor ABI for a deployment operation.
    fn deployment_init_code(&self, _operation: &OperationV1) -> Result<Vec<u8>> {
        Err(Error::Chain(
            "EVM deployment artifacts are unavailable to this adapter".into(),
        ))
    }
    /// Simulates a contract-creation transaction.
    async fn estimate_creation(
        &self,
        _from: Address,
        _value: U256,
        _init_code: Vec<u8>,
    ) -> Result<EvmSimulationV1> {
        Err(Error::Chain(
            "EVM contract-creation simulation is unsupported by this adapter".into(),
        ))
    }
    /// Submits a locally signed EIP-2718 transaction.
    async fn send_raw_transaction(&self, encoded: &[u8]) -> Result<String>;
    /// Reads a receipt and its emitted logs by transaction hash.
    async fn transaction_receipt(&self, transaction_hash: &str) -> Result<Option<EvmReceiptV1>>;
    /// Reads the canonical RPC transaction object by hash.
    async fn transaction_by_hash(
        &self,
        transaction_hash: &str,
    ) -> Result<Option<serde_json::Value>>;
}

/// Live gas/fee simulation for the exact typed transaction a proposal
/// commits to. Every field is adapter-derived; no constant policy values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvmSimulationV1 {
    /// RPC gas estimate for the exact sender, target, value, and calldata.
    pub gas_limit: u64,
    /// Decimal-string wei EIP-1559 max base fee.
    pub max_fee_per_gas_wei: String,
    /// Decimal-string wei EIP-1559 max priority fee.
    pub max_priority_fee_per_gas_wei: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EvmReceiptV1 {
    pub transaction_hash: String,
    pub block_number: Option<u64>,
    pub succeeded: Option<bool>,
    pub logs: Vec<serde_json::Value>,
    pub raw: serde_json::Value,
}
/// HTTP JSON-RPC implementation of [`EvmChain`].
pub struct HttpEvmChain {
    provider: RootProvider<Ethereum>,
    artifact_root: Option<std::path::PathBuf>,
}

impl HttpEvmChain {
    /// Connects to a credential-safe HTTP JSON-RPC endpoint.
    pub fn connect_http(url: &str) -> Result<Self> {
        let url = reqwest::Url::parse(url)
            .map_err(|error| Error::InvalidInput(format!("invalid EVM RPC URL: {error}")))?;
        let headers = crate::config::rpc_headers();
        let provider = if headers.is_empty() {
            ProviderBuilder::new()
                .disable_recommended_fillers()
                .connect_http(url)
        } else {
            let mut header_map = reqwest::header::HeaderMap::new();
            for (name, value) in &headers {
                let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|error| {
                        Error::InvalidInput(format!("invalid RPC header name {name}: {error}"))
                    })?;
                let header_value =
                    reqwest::header::HeaderValue::from_str(value.as_str()).map_err(|error| {
                        Error::InvalidInput(format!("RPC header {name} is invalid: {error}"))
                    })?;
                header_map.insert(header_name, header_value);
            }
            let client = reqwest::Client::builder()
                .default_headers(header_map)
                .build()
                .map_err(|error| {
                    Error::Chain(format!("rpc header client build failed: {error}"))
                })?;
            ProviderBuilder::new()
                .disable_recommended_fillers()
                .connect_reqwest(client, url)
        };
        Ok(Self {
            provider,
            artifact_root: None,
        })
    }

    /// Contract-name alias used by the adapter boundary contract.
    pub fn new(url: &str) -> Result<Self> {
        Self::connect_http(url)
    }

    #[must_use]
    pub fn with_artifact_root(mut self, root: &std::path::Path) -> Self {
        self.artifact_root = Some(root.to_path_buf());
        self
    }
}

#[async_trait::async_trait]
impl EvmChain for HttpEvmChain {
    async fn chain_id(&self) -> Result<u64> {
        self.provider
            .get_chain_id()
            .await
            .map_err(|error| Error::Chain(format!("evm chain id read failed: {error}")))
    }

    async fn code(&self, address: Address) -> Result<Vec<u8>> {
        self.provider
            .get_code_at(address)
            .await
            .map(|code| code.to_vec())
            .map_err(|error| Error::Chain(format!("evm code read failed: {error}")))
    }

    async fn call(&self, to: Address, calldata: Vec<u8>) -> Result<Vec<u8>> {
        let transaction = TransactionRequest {
            to: Some(TxKind::Call(to)),
            input: TransactionInput::new(Bytes::from(calldata)),
            ..Default::default()
        };
        self.provider
            .call(transaction)
            .await
            .map(|data| data.to_vec())
            .map_err(|error| Error::Chain(format!("evm call failed: {error}")))
    }

    async fn endpoint_eid(&self, endpoint: &str) -> Result<u32> {
        let target = parse_address(endpoint)?;
        let data = self.call(target, selector("eid()")).await?;
        decode_u32_word(&data)
    }

    async fn latest_block(&self) -> Result<u64> {
        self.provider
            .get_block_number()
            .await
            .map_err(|error| Error::Chain(format!("evm block read failed: {error}")))
    }

    async fn account_nonce(&self, address: Address) -> Result<u64> {
        self.provider
            .get_transaction_count(address)
            .pending()
            .await
            .map_err(|error| Error::Chain(format!("evm nonce read failed: {error}")))
    }

    async fn safe_state(&self, safe: Address) -> Result<Option<(u32, String)>> {
        let threshold = match self.call(safe, selector("getThreshold()")).await {
            Ok(data) => decode_u32_word(&data)?,
            Err(_) => return Ok(None),
        };
        let nonce_word = match self.call(safe, selector("nonce()")).await {
            Ok(data) => decode_u256_word(&data)?,
            Err(_) => return Ok(None),
        };
        let nonce_bytes: [u8; 16] = nonce_word[16..32]
            .try_into()
            .map_err(|_| Error::Chain("short safe nonce word".to_string()))?;
        Ok(Some((
            threshold,
            u128::from_be_bytes(nonce_bytes).to_string(),
        )))
    }

    fn deployment_init_code(&self, operation: &OperationV1) -> Result<Vec<u8>> {
        use alloy::sol_types::SolValue as _;

        let OperationV1::DeployEvmOft {
            deployer,
            nonce,
            creation_bytecode_keccak256,
            name,
            symbol,
            endpoint,
            owner_delegate,
            expected_address,
        } = operation
        else {
            return Err(Error::InvalidInput(
                "deployment init code requires deploy_evm_oft".into(),
            ));
        };
        let deployer_address = parse_address(deployer)?;
        if canonical_address(derive_create_address(deployer_address, *nonce))
            != canonical_address(parse_address(expected_address)?)
        {
            return Err(Error::Custody(
                "EVM deployment expected address does not match deployer nonce".into(),
            ));
        }
        let hash = creation_bytecode_keccak256.trim_start_matches("0x");
        let root = self
            .artifact_root
            .as_ref()
            .ok_or_else(|| Error::Custody("EVM artifact root is not configured".into()))?;
        let artifact: serde_json::Value = serde_json::from_slice(&std::fs::read(
            root.join(".artifacts").join(format!("evm-{hash}.json")),
        )?)?;
        let bytecode = artifact
            .pointer("/bytecode/object")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                Error::Custody("preserved EVM artifact has no creation bytecode".into())
            })?;
        let mut init_code = hex::decode(bytecode.trim_start_matches("0x"))
            .map_err(|_| Error::Custody("preserved EVM creation bytecode is not hex".into()))?;
        if !hex::encode(keccak256_of(&init_code)).eq_ignore_ascii_case(hash) {
            return Err(Error::Custody(
                "preserved EVM creation bytecode digest mismatch".into(),
            ));
        }
        init_code.extend(
            (
                name.clone(),
                symbol.clone(),
                parse_address(endpoint)?,
                parse_address(owner_delegate)?,
            )
                .abi_encode(),
        );
        Ok(init_code)
    }

    async fn estimate_creation(
        &self,
        from: Address,
        value: U256,
        init_code: Vec<u8>,
    ) -> Result<EvmSimulationV1> {
        let transaction = TransactionRequest {
            from: Some(from),
            to: Some(TxKind::Create),
            value: Some(value),
            input: TransactionInput::new(Bytes::from(init_code)),
            ..Default::default()
        };
        let gas_limit = self
            .provider
            .estimate_gas(transaction)
            .await
            .map_err(|error| {
                Error::Chain(format!("EVM creation gas estimation failed: {error}"))
            })?;
        let fees = self
            .provider
            .estimate_eip1559_fees()
            .await
            .map_err(|error| Error::Chain(format!("EVM fee estimation failed: {error}")))?;
        Ok(EvmSimulationV1 {
            gas_limit,
            max_fee_per_gas_wei: fees.max_fee_per_gas.to_string(),
            max_priority_fee_per_gas_wei: fees.max_priority_fee_per_gas.to_string(),
        })
    }

    async fn estimate_transaction(
        &self,
        from: Address,
        to: Address,
        value: U256,
        calldata: Vec<u8>,
    ) -> Result<EvmSimulationV1> {
        let transaction = TransactionRequest {
            from: Some(from),
            to: Some(TxKind::Call(to)),
            value: Some(value),
            input: TransactionInput::new(Bytes::from(calldata)),
            ..Default::default()
        };
        let gas_limit = self
            .provider
            .estimate_gas(transaction)
            .await
            .map_err(|error| Error::Chain(format!("evm gas estimation failed: {error}")))?;
        let fees = self
            .provider
            .estimate_eip1559_fees()
            .await
            .map_err(|error| Error::Chain(format!("evm fee estimation failed: {error}")))?;
        Ok(EvmSimulationV1 {
            gas_limit,
            max_fee_per_gas_wei: fees.max_fee_per_gas.to_string(),
            max_priority_fee_per_gas_wei: fees.max_priority_fee_per_gas.to_string(),
        })
    }

    async fn send_raw_transaction(&self, encoded: &[u8]) -> Result<String> {
        let pending = self
            .provider
            .send_raw_transaction(encoded)
            .await
            .map_err(|error| Error::Chain(format!("evm raw transaction send failed: {error}")))?;
        Ok(format!("{:#x}", pending.tx_hash()))
    }

    async fn transaction_receipt(&self, transaction_hash: &str) -> Result<Option<EvmReceiptV1>> {
        let hash = transaction_hash.parse().map_err(|error| {
            Error::InvalidInput(format!("invalid EVM transaction hash: {error}"))
        })?;
        let Some(receipt) = self
            .provider
            .get_transaction_receipt(hash)
            .await
            .map_err(|error| Error::Chain(format!("evm receipt read failed: {error}")))?
        else {
            return Ok(None);
        };
        let raw = serde_json::to_value(&receipt)?;
        let block_number = raw
            .get("blockNumber")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| u64::from_str_radix(value.trim_start_matches("0x"), 16).ok())
            .or_else(|| raw.get("blockNumber").and_then(serde_json::Value::as_u64));
        let succeeded = raw
            .get("status")
            .and_then(serde_json::Value::as_str)
            .map(|status| status == "0x1")
            .or_else(|| raw.get("status").and_then(serde_json::Value::as_bool));
        let logs = raw
            .get("logs")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(Some(EvmReceiptV1 {
            transaction_hash: transaction_hash.into(),
            block_number,
            succeeded,
            logs,
            raw,
        }))
    }

    async fn transaction_by_hash(
        &self,
        transaction_hash: &str,
    ) -> Result<Option<serde_json::Value>> {
        let hash = transaction_hash.parse().map_err(|error| {
            Error::InvalidInput(format!("invalid EVM transaction hash: {error}"))
        })?;
        self.provider
            .get_transaction_by_hash(hash)
            .await
            .map_err(|error| Error::Chain(format!("evm transaction read failed: {error}")))?
            .map(|transaction| serde_json::to_value(transaction).map_err(Error::from))
            .transpose()
    }
}
/// ABI selector for a canonical function signature, computed at runtime so
/// no memorized constant can drift.
fn selector(signature: &str) -> Vec<u8> {
    keccak256(signature.as_bytes())[..4].to_vec()
}
fn decode_u32_word(data: &[u8]) -> Result<u32> {
    if data.len() < 32 {
        return Err(Error::Chain("evm view returned a short word".to_string()));
    }
    let mut word = [0u8; 4];
    word.copy_from_slice(&data[28..32]);
    Ok(u32::from_be_bytes(word))
}

fn decode_u256_word(data: &[u8]) -> Result<[u8; 32]> {
    if data.len() < 32 {
        return Err(Error::Chain("evm view returned a short word".to_string()));
    }
    let mut word = [0u8; 32];
    word.copy_from_slice(&data[..32]);
    Ok(word)
}

/// keccak256 helper used for init-code hashing once the wrapper is built.
#[must_use]
pub fn keccak256_of(data: &[u8]) -> alloy::primitives::B256 {
    keccak256(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_derivation_matches_pinned_vectors() {
        let deployer =
            parse_address("0xb20a608c624ca5003905aa834de7156c68b2e1d0").expect("valid deployer");
        assert_eq!(
            canonical_address(derive_create_address(deployer, 0)),
            "0x00000000219ab540356cbb839cbe05303d7705fa"
        );
        assert_eq!(
            canonical_address(derive_create_address(deployer, 1)),
            "0xe33c6e89e69d085897f98e92b06ebd541d1daa99"
        );
    }

    #[test]
    fn address_parsing_rejects_malformed_input() {
        assert!(matches!(parse_address(""), Err(Error::InvalidInput(_))));
        assert!(matches!(
            parse_address("b20a608c624ca5003905aa834de7156c68b2e1d0"),
            Err(Error::InvalidInput(_))
        ));
        assert!(matches!(
            parse_address("0xb20a608c624ca5003905aa834de7156c68b2e1d"),
            Err(Error::InvalidInput(_))
        ));
        assert!(matches!(
            parse_address("0xb20a608c624ca5003905aa834de7156c68b2e1d00"),
            Err(Error::InvalidInput(_))
        ));
        assert!(parse_address("0xb20A608C624Ca5003905aA834De7156C68b2E1d0").is_ok());
    }

    #[test]
    fn binding_starts_without_init_code_hash() {
        let deployer =
            parse_address("0xb20a608c624ca5003905aa834de7156c68b2e1d0").expect("valid deployer");
        let endpoint = parse_address("0x00000000219ab540356cbb839cbe05303d7705fa").expect("valid");
        let binding = DeployEvmOftBindingV1::bind(
            deployer,
            0,
            None,
            "Test".into(),
            "TST".into(),
            endpoint,
            deployer,
        )
        .expect("binding valid");
        assert_eq!(binding.init_code_hash, None);
        assert_eq!(
            binding.derived_address,
            "0x00000000219ab540356cbb839cbe05303d7705fa"
        );
        let digest = binding.intent_sha256().expect("digest computable");
        assert_eq!(digest.len(), 64);
    }

    #[test]
    fn secret_file_checks_reject_symlink_and_loose_modes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("secret.txt");
        std::fs::write(&file, b"secret").expect("write");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        assert!(validate_secret_file(&file).is_ok());

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(matches!(
            validate_secret_file(&file),
            Err(Error::Custody(_))
        ));

        let link = dir.path().join("link.txt");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        std::os::unix::fs::symlink(&file, &link).expect("symlink");
        assert!(matches!(
            validate_secret_file(&link),
            Err(Error::Custody(_))
        ));
    }

    /// Writes a real encrypted Foundry V3 keystore fixture for `key` with the
    /// given password and POSIX mode, returning its path.
    fn write_encrypted_fixture(
        dir: &std::path::Path,
        key: &[u8; 32],
        password: &str,
        mode: u32,
    ) -> std::path::PathBuf {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        eth_keystore::encrypt_key(dir, &mut rng, key, password, Some("fixture.json"))
            .expect("encrypt fixture");
        let path = dir.join("fixture.json");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
        path
    }

    fn fixture_signer(key: &[u8; 32]) -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::from(*key)).expect("signer from key")
    }

    #[test]
    fn encrypted_keystore_fixture_qualifies_with_expected_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = [0xAB; 32];
        let password = "op-secret-marker-7f3a";
        let expected = fixture_signer(&key).address();
        let path = write_encrypted_fixture(dir.path(), &key, password, 0o600);

        let loaded = keystore_signer(&path, password, expected).expect("fixture qualifies");
        assert_eq!(loaded.address(), expected);
    }

    #[test]
    fn encrypted_keystore_fixture_rejects_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = [0xAB; 32];
        let password = "op-secret-marker-7f3a";
        let expected = fixture_signer(&key).address();
        let path = write_encrypted_fixture(dir.path(), &key, password, 0o600);
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).expect("symlink");

        let error =
            keystore_signer(&link, password, expected).expect_err("symlink must be rejected");
        assert!(matches!(error, Error::Custody(_)));
        assert!(error.to_string().contains("symlink"));
        assert!(!error.to_string().contains(password));
    }

    #[test]
    fn encrypted_keystore_fixture_rejects_unsafe_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = [0xAB; 32];
        let password = "op-secret-marker-7f3a";
        let expected = fixture_signer(&key).address();
        let path = write_encrypted_fixture(dir.path(), &key, password, 0o644);

        let error =
            keystore_signer(&path, password, expected).expect_err("loose mode must be rejected");
        assert!(matches!(error, Error::Custody(_)));
        assert!(error.to_string().contains("0600"));
        assert!(!error.to_string().contains(password));
    }

    #[test]
    fn encrypted_keystore_identity_mismatch_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = [0xAB; 32];
        let other_key = [0xCD; 32];
        let password = "op-secret-marker-7f3a";
        let expected = fixture_signer(&other_key).address();
        let path = write_encrypted_fixture(dir.path(), &key, password, 0o600);

        let error = keystore_signer(&path, password, expected)
            .expect_err("identity mismatch must fail closed");
        assert!(matches!(error, Error::Custody(_)));
        assert!(error.to_string().contains("expected identity"));
        assert!(!error.to_string().contains(password));
    }

    #[test]
    fn encrypted_keystore_secrets_never_enter_errors_or_rendered_commands() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = [0xAB; 32];
        let password = "op-secret-marker-7f3a";
        let expected = fixture_signer(&key).address();
        let path = write_encrypted_fixture(dir.path(), &key, password, 0o600);

        let error = keystore_signer(&path, "wrong-password", expected)
            .expect_err("wrong password must fail decryption");
        assert!(matches!(error, Error::Chain(_)));
        assert!(!error.to_string().contains(password));

        let rendered = crate::process::display_command(
            "cast",
            &[
                "wallet".to_string(),
                "sign".to_string(),
                "--password".to_string(),
                password.to_string(),
            ],
            &[3],
            &[],
        );
        assert!(!rendered.contains(password));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn password_file_trims_only_line_endings_and_zeroizes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("pass.txt");
        std::fs::write(&file, b"op secret\n").expect("write");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        let password = read_password_file(&file).expect("readable");
        assert_eq!(password.as_str(), "op secret");
    }
    #[test]
    fn signs_exact_eip1559_plan_binding() {
        use alloy::{consensus::TxEnvelope, eips::eip2718::Decodable2718 as _};
        let signer = fixture_signer(&[0xAB; 32]);
        let binding = crate::domain::EvmPlanBindingV1 {
            chain_id: "11155111".into(),
            sender: canonical_address(signer.address()),
            target: "0x1111111111111111111111111111111111111111".into(),
            value: "17".into(),
            nonce: "9".into(),
            calldata: "0x1234".into(),
            gas_limit: "50000".into(),
            max_fee_per_gas_wei: "2000000000".into(),
            max_priority_fee_per_gas_wei: "1000000000".into(),
            transaction_digest: "plan-digest".into(),
            safe: None,
        };
        let signed = sign_eip1559(&binding, &signer).expect("sign");
        assert_eq!(
            signed.transaction_hash,
            format!("{:#x}", keccak256(&signed.encoded))
        );
        let mut encoded = signed.encoded.as_slice();
        let envelope = TxEnvelope::decode_2718(&mut encoded).expect("decode");
        assert!(encoded.is_empty());
        match envelope {
            TxEnvelope::Eip1559(transaction) => {
                assert_eq!(transaction.tx().chain_id, 11_155_111);
                assert_eq!(transaction.tx().nonce, 9);
                assert_eq!(transaction.tx().value, U256::from(17));
                assert_eq!(transaction.tx().input.as_ref(), &[0x12, 0x34]);
                assert_eq!(
                    transaction
                        .signature()
                        .recover_address_from_prehash(&transaction.signature_hash())
                        .expect("recover"),
                    signer.address()
                );
            }
            other => panic!("unexpected transaction envelope: {other:?}"),
        }
    }

    #[test]
    fn refuses_signer_that_differs_from_bound_sender() {
        let signer = fixture_signer(&[0xAB; 32]);
        let other = fixture_signer(&[0xCD; 32]);
        let binding = crate::domain::EvmPlanBindingV1 {
            chain_id: "11155111".into(),
            sender: canonical_address(other.address()),
            target: "0x1111111111111111111111111111111111111111".into(),
            value: "0".into(),
            nonce: "9".into(),
            calldata: "0x".into(),
            gas_limit: "50000".into(),
            max_fee_per_gas_wei: "2000000000".into(),
            max_priority_fee_per_gas_wei: "1000000000".into(),
            transaction_digest: "plan-digest".into(),
            safe: None,
        };

        let error = sign_eip1559(&binding, &signer).expect_err("wrong signer must refuse");
        assert!(matches!(error, Error::Custody(_)));
        assert!(error.to_string().contains("differs from plan sender"));
    }
}
