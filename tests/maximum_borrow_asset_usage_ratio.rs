use rstest::rstest;
use templar_common::rational::Fraction;
use test_utils::*;

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
        c.maximum_borrow_asset_usage_ratio = Fraction::new(percent, 100).unwrap();
    })
    .await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;
    c.borrow(&borrow_user, percent as u128 * 10, EQUAL_PRICE)
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
        c.maximum_borrow_asset_usage_ratio = Fraction::new(percent, 100).unwrap();
    })
    .await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;
    c.borrow(&borrow_user, percent as u128 * 10 + 1, EQUAL_PRICE)
        .await;
}
