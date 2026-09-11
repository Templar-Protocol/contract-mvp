use crate::{
    domain::{ChainIdentityV1, Environment},
    error::{Error, Result},
    evm::EvmChain,
    stellar::StellarChain,
};

pub const STELLAR_TESTNET_PASSPHRASE: &str = "Test SDF Network ; September 2015";
pub const STELLAR_PUBLIC_PASSPHRASE: &str = "Public Global Stellar Network ; September 2015";
pub const STELLAR_TESTNET_EID: u32 = 40_600;
pub const STELLAR_MAINNET_EID: u32 = 30_600;
pub const SEPOLIA_EID: u32 = 40_161;
pub const ETHEREUM_EID: u32 = 30_101;
pub const STELLAR_TESTNET_ENDPOINT: &str =
    "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV";
pub const STELLAR_MAINNET_ENDPOINT: &str =
    "CCQLLRE5JBAWYCW3KTWOIWLMFDUOKROQVZNSALQMGOSXNW3ERUOWTZGK";
pub const SEPOLIA_ENDPOINT: &str = "0x6EDCE65403992e310A62460808c4b910D972f10f";
pub const ETHEREUM_ENDPOINT: &str = "0x1a44076050125825900e736c501f859c50fE728c";

pub fn classify(identity: &ChainIdentityV1) -> Result<Environment> {
    let environment = if identity.stellar_passphrase == STELLAR_TESTNET_PASSPHRASE
        && identity.stellar_eid == STELLAR_TESTNET_EID
        && identity.stellar_endpoint == STELLAR_TESTNET_ENDPOINT
        && identity.evm_chain_id == 11_155_111
        && identity.evm_eid == SEPOLIA_EID
        && identity.evm_endpoint.eq_ignore_ascii_case(SEPOLIA_ENDPOINT)
    {
        Environment::StellarTestnetSepolia
    } else if identity.stellar_passphrase == STELLAR_PUBLIC_PASSPHRASE
        || identity.stellar_eid == STELLAR_MAINNET_EID
        || identity.evm_chain_id == 1
        || identity.evm_eid == ETHEREUM_EID
    {
        if identity.stellar_passphrase == STELLAR_PUBLIC_PASSPHRASE
            && identity.stellar_eid == STELLAR_MAINNET_EID
            && identity.stellar_endpoint == STELLAR_MAINNET_ENDPOINT
            && identity.evm_chain_id == 1
            && identity.evm_eid == ETHEREUM_EID
            && identity
                .evm_endpoint
                .eq_ignore_ascii_case(ETHEREUM_ENDPOINT)
        {
            Environment::StellarMainnetEthereum
        } else {
            return Err(Error::Policy("unknown_or_mixed_environment".into()));
        }
    } else {
        return Err(Error::Policy("unknown_environment".into()));
    };

    if identity.environment != environment {
        return Err(Error::Policy("requested_environment_mismatch".into()));
    }
    let valid_code_hash = |value: &str| {
        let value = value.trim_start_matches("0x");
        value.len() == 64 && hex::decode(value).is_ok()
    };
    if !valid_code_hash(&identity.stellar_endpoint_code_hash)
        || !valid_code_hash(&identity.evm_endpoint_code_hash)
    {
        return Err(Error::InvalidInput(
            "endpoint code hashes must be 32-byte hex digests".into(),
        ));
    }
    Ok(environment)
}

/// Verifies the deployed endpoint programs against the code identities bound
/// into authoritative route state. Production proposal creation must pass this
/// live check before an externally signable payload is emitted.
pub fn verify_live_endpoint_code(
    identity: &ChainIdentityV1,
    stellar: &dyn StellarChain,
    evm: &dyn EvmChain,
) -> Result<()> {
    let stellar_hash = stellar.contract_code_hash(&identity.stellar_endpoint)?;
    if !stellar_hash
        .trim_start_matches("0x")
        .eq_ignore_ascii_case(identity.stellar_endpoint_code_hash.trim_start_matches("0x"))
    {
        return Err(Error::Chain(
            "Stellar endpoint code hash differs from the bound identity".into(),
        ));
    }
    let endpoint = crate::evm::parse_address(&identity.evm_endpoint)?;
    let code = crate::block_on_result(evm.code(endpoint))?;
    if code.is_empty() {
        return Err(Error::Chain("EVM endpoint has no deployed code".into()));
    }
    let evm_hash = hex::encode(crate::evm::keccak256_of(&code));
    if !evm_hash.eq_ignore_ascii_case(identity.evm_endpoint_code_hash.trim_start_matches("0x")) {
        return Err(Error::Chain(
            "EVM endpoint code hash differs from the bound identity".into(),
        ));
    }
    Ok(())
}

pub fn require_testnet(identity: &ChainIdentityV1) -> Result<()> {
    if classify(identity)?.is_mainnet() {
        return Err(Error::Policy("production_mutation_unsupported_v1".into()));
    }
    Ok(())
}

/// Derives live chain facts and requires them to match the desired identity.
///
/// Preview (`write == false`) validates the desired identity offline. A
/// write additionally requires live RPC URLs and compares every fact the
/// qualified adapters can honestly derive: the Stellar passphrase, the EVM
/// chain ID, and the live EVM endpoint EID. The Stellar endpoint EID view
/// is part of the pending native-mutation qualification gate and is bound
/// by the desired-identity equality instead.
pub fn init_environment(
    desired: &crate::domain::DesiredRouteV1,
    stellar_rpc: Option<&str>,
    evm_rpc: Option<&str>,
    write: bool,
) -> Result<()> {
    let identity = &desired.identity;
    classify(identity)?;
    if !write {
        return Ok(());
    }
    let stellar_rpc = stellar_rpc.ok_or_else(|| {
        Error::Chain("live_environment_required: --stellar-rpc-env is required for --write".into())
    })?;
    let evm_rpc = evm_rpc.ok_or_else(|| {
        Error::Chain("live_environment_required: --evm-rpc-env is required for --write".into())
    })?;
    let stellar = crate::stellar::HttpStellarChain::new(stellar_rpc)?;
    let evm = crate::evm::HttpEvmChain::new(evm_rpc)?;
    verify_live_endpoint_code(identity, &stellar, &evm)?;
    let live_passphrase = stellar.network_passphrase()?;
    if live_passphrase != identity.stellar_passphrase {
        return Err(Error::Chain(format!(
            "stellar passphrase mismatch: desired {} but rpc reports {live_passphrase}",
            identity.stellar_passphrase
        )));
    }
    let live_stellar_eid =
        stellar.endpoint_eid(&identity.stellar_endpoint, &desired.stellar_owner)?;
    if live_stellar_eid != identity.stellar_eid {
        return Err(Error::Chain(format!(
            "stellar endpoint eid mismatch: desired {} but rpc reports {live_stellar_eid}",
            identity.stellar_eid
        )));
    }
    let live_chain_id = crate::block_on_result(evm.chain_id())?;
    if live_chain_id != identity.evm_chain_id {
        return Err(Error::Chain(format!(
            "evm chain id mismatch: desired {} but rpc reports {live_chain_id}",
            identity.evm_chain_id
        )));
    }
    let live_eid = crate::block_on_result(evm.endpoint_eid(&identity.evm_endpoint))?;
    if live_eid != identity.evm_eid {
        return Err(Error::Chain(format!(
            "evm endpoint eid mismatch: desired {} but rpc reports {live_eid}",
            identity.evm_eid
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mainnet_identity(evm_endpoint: &str) -> ChainIdentityV1 {
        ChainIdentityV1 {
            environment: Environment::StellarMainnetEthereum,
            stellar_passphrase: STELLAR_PUBLIC_PASSPHRASE.into(),
            stellar_eid: STELLAR_MAINNET_EID,
            stellar_endpoint: STELLAR_MAINNET_ENDPOINT.into(),
            stellar_endpoint_code_hash: "a".repeat(64),
            evm_chain_id: 1,
            evm_eid: ETHEREUM_EID,
            evm_endpoint: evm_endpoint.into(),
            evm_endpoint_code_hash: "b".repeat(64),
        }
    }

    #[test]
    fn ethereum_mainnet_requires_the_official_endpoint_v2() {
        assert_eq!(
            classify(&mainnet_identity(ETHEREUM_ENDPOINT)).expect("official endpoint"),
            Environment::StellarMainnetEthereum
        );
        assert_eq!(
            classify(&mainnet_identity(&ETHEREUM_ENDPOINT.to_ascii_uppercase()))
                .expect("address comparison is case-insensitive"),
            Environment::StellarMainnetEthereum
        );

        let error = classify(&mainnet_identity(SEPOLIA_ENDPOINT))
            .expect_err("mainnet identity with the Sepolia endpoint must fail closed");
        assert!(
            matches!(error, Error::Policy(message) if message == "unknown_or_mixed_environment")
        );
    }
}
