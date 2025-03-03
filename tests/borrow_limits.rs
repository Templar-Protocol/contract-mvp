use rstest::rstest;
use test_utils::*;

#[rstest]
#[case(0, 1, u128::MAX)]
#[case(1, 1, u128::MAX)]
#[case(10, 10, 10)]
#[case(0, 100, 100)]
#[case(0, 50, 100)]
#[case(100, 100, 100)]
#[tokio::test]
async fn borrow_within_bounds(#[case] minimum: u128, #[case] amount: u128, #[case] maximum: u128) {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|c| {
        c.maximum_borrow_amount = maximum.into();
        c.minimum_borrow_amount = minimum.into();
    })
    .await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;
    c.borrow(&borrow_user, amount, EQUAL_PRICE).await;
}

#[rstest]
#[case(2, 1, 2)]
#[case(100, 99, 1000)]
#[case(u128::MAX, 1, u128::MAX)]
#[case(1000, 738, u128::MAX)]
#[tokio::test]
#[should_panic = "Smart contract panicked: Borrow amount is smaller than minimum allowed"]
async fn borrow_below_minimum(#[case] minimum: u128, #[case] amount: u128, #[case] maximum: u128) {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|c| {
        c.maximum_borrow_amount = maximum.into();
        c.minimum_borrow_amount = minimum.into();
    })
    .await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;
    c.borrow(&borrow_user, amount, EQUAL_PRICE).await;
}

#[rstest]
#[case(0, 2, 1)]
#[case(0, 1001, 1000)]
#[case(1000, 1001, 1000)]
#[case(100, 1001, 500)]
#[tokio::test]
#[should_panic = "Smart contract panicked: Borrow amount is greater than maximum allowed"]
async fn borrow_above_maximum(#[case] minimum: u128, #[case] amount: u128, #[case] maximum: u128) {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|c| {
        c.maximum_borrow_amount = maximum.into();
        c.minimum_borrow_amount = minimum.into();
    })
    .await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 2000).await;
    c.borrow(&borrow_user, amount, EQUAL_PRICE).await;
}
