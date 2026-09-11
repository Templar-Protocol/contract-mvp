//! Golden calldata tests for `layerzero::encode_calldata`.
//!
//! Expected vectors were generated with foundry `cast calldata` against the
//! official ABI signatures (IOAppOptionsType3 tuple-array form extracted from
//! the vendored @layerzerolabs contract artifacts), not from memory.

use templar_oft_bridge_cli::domain::OperationV1;
use templar_oft_bridge_cli::layerzero::encode_calldata;

fn hex(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

#[test]
fn set_peer_matches_foundry_golden() {
    let operation = OperationV1::SetEvmPeer {
        remote_eid: 40161,
        peer: "0x417ea6acBC114DaEA057c88e0424f742E9D004Ff".into(),
    };
    assert_eq!(
        hex(&encode_calldata(&operation).expect("peer calldata")),
        "0x3400288b0000000000000000000000000000000000000000000000000000000000009ce1000000000000000000000000417ea6acbc114daea057c88e0424f742e9d004ff"
    );
}

#[test]
fn set_receive_library_matches_foundry_golden() {
    let operation = OperationV1::SetEvmReceiveLibrary {
        remote_eid: 40161,
        library: "0x6EDCE65403992e310A62460808c4b910D972f10f".into(),
        grace_period_seconds: 604_800,
    };
    assert_eq!(
        hex(&encode_calldata(&operation).expect("library calldata")),
        "0xb8849ac90000000000000000000000000000000000000000000000000000000000009ce10000000000000000000000006edce65403992e310a62460808c4b910d972f10f0000000000000000000000000000000000000000000000000000000000093a80"
    );
}

#[test]
fn enforced_options_match_foundry_golden() {
    let operation = OperationV1::SetEvmReceiveOptions {
        remote_eid: 40161,
        message_type: 1,
        options: "0x0001".into(),
    };
    assert_eq!(
        hex(&encode_calldata(&operation).expect("options calldata")),
        "0xb98bd0700000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000009ce10000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000020001000000000000000000000000000000000000000000000000000000000000"
    );
}

#[test]
fn transfer_ownership_matches_foundry_golden() {
    let operation = OperationV1::TransferEvmOwnership {
        new_owner: "0xc61B17BF20b4B16bb70C1942CD8D9eBDe6726386".into(),
    };
    assert_eq!(
        hex(&encode_calldata(&operation).expect("ownership calldata")),
        "0xf2fde38b000000000000000000000000c61b17bf20b4b16bb70c1942cd8d9ebde6726386"
    );
}

#[test]
fn stellar_only_operations_fail_closed() {
    let operation = OperationV1::PauseEmergency;
    let error = encode_calldata(&operation).expect_err("stellar op has no calldata");
    assert!(
        error.to_string().contains("stellar_only_operation"),
        "unexpected error: {error}"
    );
}

#[test]
fn typed_config_operation_encodes_endpoint_set_config() {
    let config = templar_oft_bridge_cli::layerzero::UlnConfigType3V1 {
        required_dvns: vec!["0x1111111111111111111111111111111111111111".into()],
        optional_dvns: vec![],
        optional_threshold: 0,
        confirmations: 1,
        use_default_confirmations: false,
        use_default_required_dvns: false,
        use_default_optional_dvns: false,
    };
    let operation = templar_oft_bridge_cli::layerzero::set_uln_operation(
        templar_oft_bridge_cli::domain::Vm::Evm,
        40_161,
        "send",
        "0x2222222222222222222222222222222222222222",
        "0x3333333333333333333333333333333333333333",
        "0x4444444444444444444444444444444444444444",
        &config,
    )
    .unwrap();
    let calldata = encode_calldata(&operation).unwrap();
    assert!(!calldata.is_empty());
}

#[test]
fn empty_and_short_enforced_options_fail_closed() {
    for options in ["", "0x01"] {
        let operation = OperationV1::SetEvmReceiveOptions {
            remote_eid: 40161,
            message_type: 1,
            options: options.into(),
        };
        assert!(
            encode_calldata(&operation).is_err(),
            "options {options:?} must be rejected"
        );
    }
}
