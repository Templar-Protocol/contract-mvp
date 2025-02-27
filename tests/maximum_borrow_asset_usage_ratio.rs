use rstest::rstest;
use test_utils::*;

use templar_common::number::Decimal;

#[rstest]
#[case(1)]
#[case(50)]
#[case(99)]
#[case(100)]
#[tokio::test]
async fn borrow_within_maximum_usage_ratio(#[case] percent: u16) {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|c| {
        c.maximum_borrow_asset_usage_ratio = Decimal::from(percent) / 100u32;
    })
    .await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;
    c.borrow(
        &borrow_user,
        u128::from(percent) * 10,
        COLLATERAL_HALF_PRICE,
    )
    .await;
}

#[rstest]
#[case(1)]
#[case(50)]
#[case(99)]
#[case(100)]
#[tokio::test]
#[should_panic = "Smart contract panicked: Insufficient borrow asset available"]
async fn borrow_exceeds_maximum_usage_ratio(#[case] percent: u16) {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|c| {
        c.maximum_borrow_asset_usage_ratio = Decimal::from(percent) / 100u32;
    })
    .await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;
    c.borrow(
        &borrow_user,
        u128::from(percent) * 10 + 1,
        COLLATERAL_HALF_PRICE,
    )
    .await;
}
