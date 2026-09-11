use templar_oft_bridge_cli::scan::HttpScanClient;

#[test]
fn decodes_scan_transaction_fixture_without_treating_it_as_chain_authority() {
    let messages = HttpScanClient::decode_response(serde_json::json!({
        "data": [{
            "guid": "0xguid",
            "status": {"name": "DELIVERED"},
            "source": {"tx": {"txHash": "0xsource"}},
            "destination": {"tx": {"txHash": "0xdestination"}},
            "extra": {"retained": true}
        }]
    }))
    .expect("decode");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].guid, "0xguid");
    assert_eq!(messages[0].status, "DELIVERED");
    assert_eq!(messages[0].source_transaction, "0xsource");
    assert_eq!(
        messages[0].destination_transaction.as_deref(),
        Some("0xdestination")
    );
    assert_eq!(messages[0].raw["extra"]["retained"], true);
}

#[test]
fn rejects_scan_messages_without_packet_identity_or_source_transaction() {
    assert!(HttpScanClient::decode_response(serde_json::json!({
        "data": [{"status": "INFLIGHT"}]
    }))
    .is_err());
}
