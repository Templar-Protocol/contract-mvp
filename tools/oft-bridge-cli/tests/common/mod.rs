use sha2::{Digest as _, Sha256};
use templar_oft_bridge_cli::{
    domain::{Direction, MessageRecordV1, MessageStageV1, MessageStatusEventV1, SCHEMA_VERSION},
    evm::keccak256_of,
    state::RouteStore,
};

pub const STELLAR_OFT: &str = "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV";
pub const EVM_OFT: &str = "0x1111111111111111111111111111111111111111";

pub fn seed_ofts(store: &RouteStore) {
    let mut state = store.load_state().expect("state");
    state
        .contracts
        .insert("stellar_oft".into(), STELLAR_OFT.into());
    state.contracts.insert("evm_oft".into(), EVM_OFT.into());
    state.effective_config.insert(
        templar_oft_bridge_cli::route::config_key_receive_library(
            templar_oft_bridge_cli::domain::Vm::Evm,
            40_600,
        ),
        serde_json::Value::String(EVM_OFT.into()),
    );
    state.effective_config.insert(
        templar_oft_bridge_cli::route::config_key_receive_library(
            templar_oft_bridge_cli::domain::Vm::Stellar,
            40_161,
        ),
        serde_json::Value::String(STELLAR_OFT.into()),
    );
    store.save_state(&state).expect("save state");
}

pub fn message_record(direction: Direction, nonce: u64, seed: &str) -> MessageRecordV1 {
    let (source_eid, destination_eid, sender, receiver, receive_library, initial_stage) =
        match direction {
            Direction::StellarToEvm => (
                40_600u32,
                40_161u32,
                templar_oft_bridge_cli::codec::strkey_to_bytes32(STELLAR_OFT).unwrap(),
                templar_oft_bridge_cli::codec::evm_address_to_bytes32(EVM_OFT).unwrap(),
                EVM_OFT,
                MessageStageV1::ForwardSourceAccepted,
            ),
            Direction::EvmToStellar => (
                40_161u32,
                40_600u32,
                templar_oft_bridge_cli::codec::evm_address_to_bytes32(EVM_OFT).unwrap(),
                templar_oft_bridge_cli::codec::strkey_to_bytes32(STELLAR_OFT).unwrap(),
                STELLAR_OFT,
                MessageStageV1::ReverseSourceAccepted,
            ),
        };
    let guid: [u8; 32] = Sha256::digest(format!("guid:{seed}")).into();
    let message = format!("message:{seed}").into_bytes();
    let mut header = Vec::with_capacity(81);
    header.push(1);
    header.extend(nonce.to_be_bytes());
    header.extend(source_eid.to_be_bytes());
    header.extend(sender);
    header.extend(destination_eid.to_be_bytes());
    header.extend(receiver);
    let mut payload = Vec::with_capacity(32 + message.len());
    payload.extend(guid);
    payload.extend(&message);
    let mut packet = header.clone();
    packet.extend(&payload);
    MessageRecordV1 {
        schema_name: "message_record".into(),
        schema_version: SCHEMA_VERSION,
        source_eid,
        sender: hex::encode(sender),
        nonce: nonce.to_string(),
        guid: hex::encode(guid),
        direction,
        amount_raw: "1000000".into(),
        packet_sha256: hex::encode(Sha256::digest(packet)),
        packet_header: hex::encode(header),
        message: hex::encode(message),
        payload_keccak256: hex::encode(keccak256_of(&payload)),
        origin: "layerzero_v1".into(),
        receiver: match direction {
            Direction::StellarToEvm => EVM_OFT,
            Direction::EvmToStellar => STELLAR_OFT,
        }
        .into(),
        current_receive_library: receive_library.into(),
        old_receive_library: None,
        receive_grace_until: None,
        send_library: receive_library.into(),
        uln_snapshot_sha256: "11".repeat(32),
        dvn_snapshot_sha256: "22".repeat(32),
        executor_snapshot_sha256: "33".repeat(32),
        config_snapshot_sha256: "44".repeat(32),
        source_height: "123".into(),
        source_event_coordinate: format!("tx-{seed}:event-0"),
        source_transaction: format!("tx-{seed}"),
        destination_transaction: None,
        recovery_transactions: Vec::new(),
        debited_raw: "1000000".into(),
        net_locked_raw: "1000000".into(),
        minted_raw: "0".into(),
        burned_raw: "0".into(),
        unlocked_raw: "0".into(),
        external_fee_raw: "0".into(),
        dust_raw: "0".into(),
        reconciliation_classification: None,
        status_events: vec![MessageStatusEventV1 {
            stage: initial_stage,
            observed_at_unix: 1,
            evidence_sha256: "55".repeat(32),
        }],
    }
}
