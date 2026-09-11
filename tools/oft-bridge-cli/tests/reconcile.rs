use templar_oft_bridge_cli::error::Error;
use templar_oft_bridge_cli::reconcile::{
    aggregate, reconcile, ObservedCustody, PacketKey, PacketRecord, PacketStage,
};

fn record(nonce: u64, stage: PacketStage, amount_raw: u128) -> PacketRecord {
    PacketRecord {
        key: PacketKey {
            source_eid: 40_600,
            sender: [1; 32],
            nonce,
            guid: [nonce as u8; 32],
        },
        stage,
        amount_raw,
    }
}

#[test]
fn stage_deltas_cover_forward_reverse_and_settled_packets() {
    let deltas = aggregate(&[
        record(1, PacketStage::ForwardLocked, 300),
        record(2, PacketStage::ReverseAfterBurn, 500),
        record(3, PacketStage::Delivered, 700),
    ])
    .unwrap();
    assert_eq!(deltas.pending_forward_raw, 300);
    assert_eq!(deltas.pending_reverse_raw, 500);

    let report = reconcile(
        &ObservedCustody {
            observed_lockbox_raw: 1_850,
            normalized_evm_supply_raw: 1_000,
            lockbox_retained_fee_or_dust_raw: 50,
            external_fee_reported_raw: 17,
        },
        &deltas,
    )
    .unwrap();
    assert_eq!(report.expected_lockbox_raw, 1_850);
    assert_eq!(report.deficit_raw, 0);
    assert_eq!(report.surplus_raw, 0);
    assert_eq!(report.external_fee_reported_raw, 17);
}

#[test]
fn proven_two_million_reverse_obligation_has_zero_deficit() {
    let deltas = aggregate(&[
        record(1, PacketStage::ReverseAfterBurn, 900_000),
        record(2, PacketStage::ReverseAfterBurn, 1_100_000),
    ])
    .unwrap();
    let report = reconcile(
        &ObservedCustody {
            observed_lockbox_raw: 2_000_000,
            normalized_evm_supply_raw: 0,
            lockbox_retained_fee_or_dust_raw: 0,
            external_fee_reported_raw: 0,
        },
        &deltas,
    )
    .unwrap();
    assert_eq!(report.expected_lockbox_raw, 2_000_000);
    assert_eq!(report.deficit_raw, 0);
}

#[test]
fn duplicate_packet_identity_is_rejected() {
    let packet = record(1, PacketStage::ForwardLocked, 100);
    let error = aggregate(&[packet, packet]).unwrap_err();
    assert!(matches!(error, Error::Conflict(_)));
}

#[test]
fn deficit_and_surplus_are_mutually_exclusive() {
    let none = aggregate(&[]).unwrap();
    let deficit = reconcile(
        &ObservedCustody {
            observed_lockbox_raw: 7,
            normalized_evm_supply_raw: 9,
            ..ObservedCustody::default()
        },
        &none,
    )
    .unwrap();
    assert_eq!((deficit.deficit_raw, deficit.surplus_raw), (2, 0));

    let surplus = reconcile(
        &ObservedCustody {
            observed_lockbox_raw: 11,
            normalized_evm_supply_raw: 9,
            ..ObservedCustody::default()
        },
        &none,
    )
    .unwrap();
    assert_eq!((surplus.deficit_raw, surplus.surplus_raw), (0, 2));
}
