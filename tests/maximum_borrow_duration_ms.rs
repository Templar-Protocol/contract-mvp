use near_sdk::json_types::U64;
use templar_common::borrow::{BorrowStatus, LiquidationReason};
use test_utils::*;

#[tokio::test]
async fn liquidation_after_expiration() {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|c| {
        c.maximum_borrow_duration_ms = Some(U64(100));
    })
    .await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;
    c.borrow(&borrow_user, 100, equal_price()).await;

    let status = c
        .get_borrow_status(borrow_user.id(), equal_price())
        .await
        .unwrap();

    assert!(status.is_healthy());

    c.worker.fast_forward(10).await.unwrap();

    let status = c
        .get_borrow_status(borrow_user.id(), equal_price())
        .await
        .unwrap();

    assert_eq!(
        status,
        BorrowStatus::Liquidation(LiquidationReason::Expiration),
        "Borrow should be in liquidation after expiration",
    );
}
