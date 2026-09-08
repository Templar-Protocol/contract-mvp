#![allow(clippy::expect_used)]

mod common;

use std::collections::BTreeMap;

use templar_oft_bridge_cli::{
    canary::{DestinationPacketReader, DestinationPacketState},
    domain::{
        AssetKind, AssetPolicyV1, ChainIdentityV1, DesiredRouteV1, Direction, Environment,
        MessageStageV1, OperationV1,
    },
    error::{Error, Result},
    scan::{ScanClient, ScanMessageV1},
    state::RouteStore,
};

fn desired() -> DesiredRouteV1 {
    DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: 1,
        route_id: "route-watch".into(),
        identity: ChainIdentityV1 {
            environment: Environment::StellarTestnetSepolia,
            stellar_passphrase: "Test SDF Network ; September 2015".into(),
            stellar_eid: 40_600,
            stellar_endpoint: templar_oft_bridge_cli::environment::STELLAR_TESTNET_ENDPOINT.into(),
            stellar_endpoint_code_hash: "1".repeat(64),
            evm_chain_id: 11_155_111,
            evm_eid: 40_161,
            evm_endpoint: templar_oft_bridge_cli::environment::SEPOLIA_ENDPOINT.into(),
            evm_endpoint_code_hash: "2".repeat(64),
        },
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
        stellar_owner: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
        stellar_delegate: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
        evm_owner: "0x0000000000000000000000000000000000000001".into(),
        evm_delegate: "0x0000000000000000000000000000000000000001".into(),
        config: BTreeMap::new(),
    }
}

struct FakeScan {
    status: &'static str,
    destination: Option<&'static str>,
}

#[async_trait::async_trait]
impl ScanClient for FakeScan {
    async fn messages_by_transaction(&self, _: &str) -> Result<Vec<ScanMessageV1>> {
        let record = common::message_record(Direction::StellarToEvm, 1, "watch");
        Ok(vec![ScanMessageV1 {
            guid: record.guid,
            status: self.status.into(),
            source_transaction: record.source_transaction,
            destination_transaction: self.destination.map(str::to_owned),
            raw: serde_json::json!({"status": self.status}),
        }])
    }
}

struct FakeDestination(DestinationPacketState);

impl DestinationPacketReader for FakeDestination {
    fn packet_state(
        &self,
        _: &templar_oft_bridge_cli::domain::RouteStateV1,
        _: &templar_oft_bridge_cli::domain::MessageRecordV1,
    ) -> Result<DestinationPacketState> {
        Ok(self.0)
    }
}

fn route() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path().join("route");
    let (store, _) = RouteStore::create(&root, desired()).expect("route");
    common::seed_ofts(&store);
    let mut record = common::message_record(Direction::StellarToEvm, 1, "watch");
    record.status_events[0].stage = MessageStageV1::ForwardLocked;
    let guid = record.guid.clone();
    store.append_message(record).expect("message");
    (directory, root, guid)
}

#[test]
fn verified_packet_selects_commit_before_receive() {
    let (_directory, root, guid) = route();
    templar_oft_bridge_cli::canary::watch_with_scan(
        &root,
        &guid,
        &FakeScan {
            status: "inflight",
            destination: None,
        },
        &FakeDestination(DestinationPacketState::Verified),
    )
    .expect("watch");
    assert!(matches!(
        templar_oft_bridge_cli::canary::recovery_operation(&root, &guid).expect("recover"),
        OperationV1::CommitVerification { .. }
    ));
}

#[test]
fn committed_packet_selects_receive() {
    let (_directory, root, guid) = route();
    templar_oft_bridge_cli::canary::watch_with_scan(
        &root,
        &guid,
        &FakeScan {
            status: "inflight",
            destination: None,
        },
        &FakeDestination(DestinationPacketState::Committed),
    )
    .expect("watch");
    let operation =
        templar_oft_bridge_cli::canary::recovery_operation(&root, &guid).expect("recover");
    assert!(matches!(operation, OperationV1::ExecuteReceive { .. }));
    assert!(matches!(
        operation,
        OperationV1::ExecuteReceive {
            vm: templar_oft_bridge_cli::domain::Vm::Evm,
            ..
        }
    ));
}

#[test]
fn stellar_receive_encoder_matches_six_argument_oft_abi() {
    let (_directory, root, _) = route();
    let mut state = RouteStore::open(&root).unwrap().load_state().unwrap();
    state.contracts.insert(
        "stellar_recovery_executor".into(),
        "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
    );
    let operation = OperationV1::ExecuteReceive {
        vm: templar_oft_bridge_cli::domain::Vm::Stellar,
        message: Box::new(common::message_record(
            Direction::EvmToStellar,
            1,
            "reverse-receive",
        )),
    };
    let invocation =
        templar_oft_bridge_cli::layerzero::build_stellar_operation_for_route(&state, &operation)
            .expect("encode");
    assert_eq!(invocation.function, "lz_receive");
    assert_eq!(
        invocation.args_xdr_hex.len(),
        6,
        "executor, origin, guid, message, extra_data, value"
    );
}

#[test]
fn delayed_dvn_stays_unverified_and_scan_cannot_override_rpc() {
    let (_directory, root, guid) = route();
    templar_oft_bridge_cli::canary::watch_with_scan(
        &root,
        &guid,
        &FakeScan {
            status: "inflight",
            destination: None,
        },
        &FakeDestination(DestinationPacketState::Unverified),
    )
    .expect("watch");
    let error = templar_oft_bridge_cli::canary::recovery_operation(&root, &guid)
        .expect_err("not recoverable");
    assert!(matches!(error, Error::Policy(_)));

    let error = templar_oft_bridge_cli::canary::watch_with_scan(
        &root,
        &guid,
        &FakeScan {
            status: "delivered",
            destination: Some("0xdestination"),
        },
        &FakeDestination(DestinationPacketState::Committed),
    )
    .expect_err("Scan is corroborating only");
    assert!(matches!(error, Error::Conflict(_)));
}

#[test]
fn executed_packet_records_terminal_transaction_and_refuses_duplicate_recovery() {
    let (_directory, root, guid) = route();
    templar_oft_bridge_cli::canary::watch_with_scan(
        &root,
        &guid,
        &FakeScan {
            status: "delivered",
            destination: Some("0xdestination"),
        },
        &FakeDestination(DestinationPacketState::Executed),
    )
    .expect("terminal watch");
    let message = RouteStore::open(&root)
        .expect("open")
        .load_messages()
        .expect("messages")
        .pop()
        .expect("message");
    assert_eq!(
        message.destination_transaction.as_deref(),
        Some("0xdestination")
    );
    assert_eq!(
        message.status_events.last().expect("stage").stage,
        MessageStageV1::ForwardMinted
    );
    assert!(matches!(
        templar_oft_bridge_cli::canary::recovery_operation(&root, &guid),
        Err(Error::Policy(_))
    ));
}
