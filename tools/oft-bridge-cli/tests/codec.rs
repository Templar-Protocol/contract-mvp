use sha2::{Digest as _, Sha256};
use templar_oft_bridge_cli::codec::{
    bytes32_to_evm_address, bytes32_to_strkey_account, bytes32_to_strkey_contract,
    decode_evm_executor_config, decode_evm_uln_config, decode_type3_options,
    encode_evm_executor_config, encode_evm_uln_config, encode_stellar_oapp_uln_config,
    encode_type3_options, evm_address_to_bytes32, from_shared, strkey_to_bytes32, to_shared,
    NativeDrop, StellarOAppUlnConfig, Type3Options,
};
use templar_oft_bridge_cli::error::{Error, Result};

const ACCOUNT_ZEROS: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
const ACCOUNT_AAS: &str = "GCVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVH7N";
const CONTRACT_RAMP: &str = "CAAACAQDAQCQMBYIBEFAWDANBYHRAEISCMKBKFQXDAMRUGY4DUPB6N4O";
const BAD_CRC: &str = "GCVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVKVH7B";

fn addr32(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    out
}

fn is_invalid<T>(result: &Result<T>) -> bool {
    matches!(result, Err(Error::InvalidInput(_)))
}

#[test]
fn decodes_golden_strkeys_to_exact_bytes() {
    assert_eq!(
        strkey_to_bytes32(ACCOUNT_ZEROS).unwrap(),
        [0u8; 32],
        "zeros vector must decode to zero bytes32"
    );
    assert_eq!(strkey_to_bytes32(ACCOUNT_AAS).unwrap(), addr32(&[0xAA; 32]));
    let ramp: Vec<u8> = (0u8..32).collect();
    assert_eq!(
        strkey_to_bytes32(CONTRACT_RAMP).unwrap(),
        addr32(&ramp),
        "contract vector must decode to ramp bytes32"
    );
}

#[test]
fn reencodes_bytes32_to_golden_strkeys() {
    let ramp: Vec<u8> = (0u8..32).collect();
    let ramp = addr32(&ramp);
    assert_eq!(
        bytes32_to_strkey_account(&[0u8; 32]).unwrap(),
        ACCOUNT_ZEROS
    );
    assert_eq!(bytes32_to_strkey_account(&[0xAA; 32]).unwrap(), ACCOUNT_AAS);
    assert_eq!(bytes32_to_strkey_contract(&ramp).unwrap(), CONTRACT_RAMP);
}

#[test]
fn rejects_malformed_strkeys() {
    // Corrupted checksum.
    assert!(is_invalid(&strkey_to_bytes32(BAD_CRC)));
    // Too short to carry version + payload + checksum.
    assert!(is_invalid(&strkey_to_bytes32("GAAAA")));
    // Right length but non-base32 alphabet.
    assert!(is_invalid(&strkey_to_bytes32(
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWH0"
    )));
    // Valid strkey but wrong kind for a peer: private key (S).
    assert!(is_invalid(&strkey_to_bytes32(
        "SBKAJAETSOS4IN2GF5AJO5RQ2ERUKVA7GLF4CWD2GDHEOSKVBDKUXJWB"
    )));
    // Round-trip consistency: encode then decode returns identical bytes.
    let bytes = addr32(&[7u8; 32]);
    let encoded = bytes32_to_strkey_contract(&bytes).unwrap();
    assert_eq!(strkey_to_bytes32(&encoded).unwrap(), bytes);
}

#[test]
fn evm_address_left_pads_into_bytes32() {
    let address = "0x1111111111111111111111111111111111111111";
    let bytes = evm_address_to_bytes32(address).unwrap();
    assert_eq!(&bytes[..12], &[0u8; 12]);
    assert_eq!(&bytes[12..], &[0x11; 20]);
    assert_eq!(bytes32_to_evm_address(&bytes).unwrap(), address);
}

#[test]
fn rejects_malformed_evm_addresses() {
    // Missing 0x prefix.
    assert!(is_invalid(&evm_address_to_bytes32(
        "1111111111111111111111111111111111111111"
    )));
    // 39 hex digits.
    assert!(is_invalid(&evm_address_to_bytes32(
        "0x11111111111111111111111111111111111111"
    )));
    // 41 hex digits.
    assert!(is_invalid(&evm_address_to_bytes32(
        "0x11111111111111111111111111111111111111111"
    )));
    // Non-hex digit.
    assert!(is_invalid(&evm_address_to_bytes32(
        "0xzz11111111111111111111111111111111111111"
    )));
    // Upper 12 bytes not zero.
    let mut bytes = [0u8; 32];
    bytes[5] = 1;
    assert!(matches!(
        bytes32_to_evm_address(&bytes),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn shared_conversion_is_identity_at_six_decimals() {
    let amount = 987_654_321u128;
    assert_eq!(to_shared(amount, 6).unwrap(), amount);
    assert_eq!(from_shared(amount, 6).unwrap(), amount);
}

#[test]
fn shared_conversion_scales_by_local_minus_shared() {
    // 7 local decimals: factor 10.
    assert_eq!(to_shared(1_234_560, 7).unwrap(), 123_456);
    assert_eq!(from_shared(123_456, 7).unwrap(), 1_234_560);
    // 8 local decimals: factor 100.
    assert_eq!(from_shared(2, 8).unwrap(), 200);
}

#[test]
fn shared_conversion_rejects_dust() {
    assert!(is_invalid(&to_shared(1_234_567, 7)));
    assert!(is_invalid(&to_shared(u128::MAX, 7)));
}

#[test]
fn shared_conversion_rejects_bad_decimals_and_overflow() {
    assert!(is_invalid(&to_shared(100, 5)));
    assert!(is_invalid(&from_shared(100, 5)));
    assert!(is_invalid(&from_shared(u128::MAX, 7)));
    // Decimals so large that even the scaling factor overflows u128
    // (10^39 > u128::MAX).
    assert!(is_invalid(&from_shared(1, 45)));
}

fn lz_receive_bytes(gas: u128) -> Vec<u8> {
    let mut bytes = vec![3u8, 1, 0, 17, 1];
    bytes.extend_from_slice(&gas.to_be_bytes());
    bytes
}

fn invalid_decode(bytes: &[u8]) -> bool {
    is_invalid(&decode_type3_options(bytes))
}

#[test]
fn encodes_lz_receive_only_options() {
    let options = Type3Options {
        gas: 100_000,
        native_drop: None,
    };
    assert_eq!(
        encode_type3_options(&options).unwrap(),
        lz_receive_bytes(100_000)
    );
    assert_eq!(
        decode_type3_options(&lz_receive_bytes(u128::MAX)).unwrap(),
        Type3Options {
            gas: u128::MAX,
            native_drop: None,
        }
    );
}

#[test]
fn encodes_gas_with_native_value() {
    let receiver = addr32(&[9u8; 32]);
    let options = Type3Options {
        gas: 200_000,
        native_drop: Some(NativeDrop {
            amount: 1_000,
            receiver,
        }),
    };
    let encoded = encode_type3_options(&options).unwrap();
    let mut expected = lz_receive_bytes(200_000);
    expected.extend_from_slice(&[1, 0, 49, 2]);
    expected.extend_from_slice(&1_000u128.to_be_bytes());
    expected.extend_from_slice(&receiver);
    assert_eq!(encoded, expected);
    assert_eq!(decode_type3_options(&encoded).unwrap(), options);
}

#[test]
fn decodes_rejects_malformed_type3_options() {
    // Empty and truncated envelopes.
    assert!(invalid_decode(&[]));
    assert!(invalid_decode(&[3]));
    assert!(invalid_decode(&[1]));
    assert!(invalid_decode(&[3, 1]));
    assert!(invalid_decode(&[3, 1, 0]));
    // Header claims option size 17 but only 16 bytes follow.
    assert!(invalid_decode(&[
        3, 1, 0, 17, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
    ]));
    // Unknown worker id.
    let mut unknown_worker = lz_receive_bytes(1);
    unknown_worker[1] = 9;
    assert!(invalid_decode(&unknown_worker));
    // Unknown executor option type.
    let mut unknown_type = lz_receive_bytes(1);
    unknown_type[4] = 7;
    assert!(invalid_decode(&unknown_type));
}

#[test]
fn stellar_oapp_uln_config_matches_frozen_xdr_vector() {
    let config = StellarOAppUlnConfig {
        use_default_confirmations: true,
        use_default_required_dvns: true,
        use_default_optional_dvns: true,
        confirmations: 0,
        required_dvns: Vec::new(),
        optional_dvns: Vec::new(),
        optional_dvn_threshold: 0,
    };
    let encoded = encode_stellar_oapp_uln_config(&config).expect("encode ULN config");
    assert_eq!(encoded.len(), 324);
    assert_eq!(
        hex::encode(Sha256::digest(&encoded)),
        "46dccfecca1f1e085b04e442a9084bc19173c6139b449a5cf0938b8ca9160342"
    );
}

#[test]
fn evm_uln_config_matches_foundry_abi_oracle() {
    let encoded = encode_evm_uln_config(
        15,
        &["0x1111111111111111111111111111111111111111".into()],
        &["0x2222222222222222222222222222222222222222".into()],
        1,
    )
    .expect("encode ULN config");
    assert_eq!(
        hex::encode(&encoded),
        concat!(
            "0000000000000000000000000000000000000000000000000000000000000020",
            "000000000000000000000000000000000000000000000000000000000000000f",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "00000000000000000000000000000000000000000000000000000000000000c0",
            "0000000000000000000000000000000000000000000000000000000000000100",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000001111111111111111111111111111111111111111",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000002222222222222222222222222222222222222222"
        )
    );
    let decoded = decode_evm_uln_config(&encoded).expect("decode ULN config");
    assert_eq!(decoded.confirmations, 15);
    assert_eq!(decoded.optional_dvn_threshold, 1);
    assert_eq!(
        decoded.required_dvns,
        ["0x1111111111111111111111111111111111111111"]
    );
    assert_eq!(
        decoded.optional_dvns,
        ["0x2222222222222222222222222222222222222222"]
    );
    let mut mismatched_count = encoded.clone();
    mismatched_count[95] = 2;
    assert!(decode_evm_uln_config(&mismatched_count).is_err());
    assert!(decode_evm_uln_config(&encoded[..encoded.len() - 1]).is_err());
}

#[test]
fn evm_executor_config_matches_foundry_abi_oracle() {
    let encoded = encode_evm_executor_config(10_000, "0x3333333333333333333333333333333333333333")
        .expect("encode executor config");
    assert_eq!(
        hex::encode(&encoded),
        concat!(
            "0000000000000000000000000000000000000000000000000000000000002710",
            "0000000000000000000000003333333333333333333333333333333333333333"
        )
    );
    assert_eq!(
        decode_evm_executor_config(&encoded)
            .expect("decode executor")
            .max_message_size,
        10_000
    );
}

#[test]
fn decodes_rejects_duplicate_and_missing_options() {
    // Duplicate lzReceive option.
    let mut duplicated = lz_receive_bytes(1);
    duplicated.extend_from_slice(&lz_receive_bytes(2)[1..]);
    assert!(invalid_decode(&duplicated));
    // Native drop without mandatory lzReceive gas.
    let mut drop_only = vec![3u8];
    drop_only.extend_from_slice(&[1, 0, 49, 2]);
    drop_only.extend_from_slice(&1u128.to_be_bytes());
    drop_only.extend_from_slice(&[0u8; 32]);
    assert!(invalid_decode(&drop_only));
    // Duplicate native drop option.
    let mut duplicated_drop = vec![3u8];
    duplicated_drop.extend_from_slice(&[1, 0, 17, 1]);
    duplicated_drop.extend_from_slice(&1u128.to_be_bytes());
    duplicated_drop.extend_from_slice(&[1, 0, 49, 2]);
    duplicated_drop.extend_from_slice(&1u128.to_be_bytes());
    duplicated_drop.extend_from_slice(&[0u8; 32]);
    duplicated_drop.extend_from_slice(&[1, 0, 49, 2]);
    duplicated_drop.extend_from_slice(&2u128.to_be_bytes());
    duplicated_drop.extend_from_slice(&[1u8; 32]);
    assert!(invalid_decode(&duplicated_drop));
}

#[test]
fn encodes_rejects_zero_gas_and_zero_drop() {
    assert!(is_invalid(&encode_type3_options(&Type3Options {
        gas: 0,
        native_drop: None,
    })));
    assert!(is_invalid(&encode_type3_options(&Type3Options {
        gas: 1,
        native_drop: Some(NativeDrop {
            amount: 0,
            receiver: [0u8; 32],
        }),
    })));
}
