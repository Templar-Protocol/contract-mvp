use templar_common::rational::Rational;
use test_utils::*;

#[tokio::test]
async fn successful_liquidation_totally_underwater() {
    let SetupEverything {
        c,
        liquidator_user,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|_| {}).await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 500).await;
    c.borrow(&borrow_user, 300, EQUAL_PRICE).await;

    // value of collateral will go 500->250
    // collateralization: 250/300 ~= 83%
    // which is bad debt (<100%).

    let collateral_balance_before = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset_balance_of(liquidator_user.id()).await;

    c.liquidate(
        &liquidator_user,
        borrow_user.id(),
        300, // this is fmv (i.e. NOT what a real liquidator would do to purchase bad debt)
        COLLATERAL_HALF_PRICE,
    )
    .await;

    let collateral_balance_after = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset_balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        500,
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        300,
        "Liquidation should transfer correct amount of tokens",
    );
}

#[tokio::test]
async fn successful_liquidation_good_debt_under_mcr() {
    let SetupEverything {
        c,
        liquidator_user,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|config| {
        config.minimum_collateral_ratio_per_borrow = Rational::new(110, 100);
    })
    .await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 500).await;
    c.borrow(&borrow_user, 245, EQUAL_PRICE).await;

    // when collateral halves in price, that means value will go 500->250.
    // collateralization: 250 / 245 ~= 102%
    // still good debt but under MCR (110%).

    let collateral_balance_before = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset_balance_of(liquidator_user.id()).await;

    c.liquidate(
        &liquidator_user,
        borrow_user.id(),
        250, // still liquidate at fmv for this test
        COLLATERAL_HALF_PRICE,
    )
    .await;

    let collateral_balance_after = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset_balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        500,
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        250,
        "Liquidation should transfer correct amount of tokens",
    );

    // TODO: test yield distributions
}

#[tokio::test]
async fn fail_liquidation_too_little_attached() {
    let SetupEverything {
        c,
        liquidator_user,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|_| {}).await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 500).await;
    c.borrow(&borrow_user, 300, EQUAL_PRICE).await;

    let collateral_balance_before = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset_balance_of(liquidator_user.id()).await;

    c.liquidate(
        &liquidator_user,
        borrow_user.id(),
        150,
        COLLATERAL_HALF_PRICE,
    )
    .await;

    let collateral_balance_after = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset_balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_before, collateral_balance_after,
        "Liquidator should not obtain any additional collateral from a rejected liquidation attempt",
    );
    assert_eq!(
        borrow_balance_before, borrow_balance_after,
        "Liquidator should be refunded for a rejected liquidation attempt",
    );

    // ensure borrow position remains unchanged
    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    assert_eq!(borrow_position.get_borrow_asset_principal().as_u128(), 300);
    assert_eq!(borrow_position.collateral_asset_deposit.as_u128(), 500);
}

#[tokio::test]
async fn fail_liquidation_healthy_borrow() {
    let SetupEverything {
        c,
        liquidator_user,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|_| {}).await;

    c.supply(&supply_user, 1000).await;
    c.collateralize(&borrow_user, 500).await;
    c.borrow(&borrow_user, 300, EQUAL_PRICE).await;

    let collateral_balance_before = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset_balance_of(liquidator_user.id()).await;

    c.liquidate(&liquidator_user, borrow_user.id(), 300, EQUAL_PRICE)
        .await;

    let collateral_balance_after = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset_balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_before, collateral_balance_after,
        "Liquidator should not obtain any additional collateral from a rejected liquidation attempt",
    );
    assert_eq!(
        borrow_balance_before, borrow_balance_after,
        "Liquidator should be refunded for a rejected liquidation attempt",
    );

    // ensure borrow position remains unchanged
    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
    assert_eq!(borrow_position.get_borrow_asset_principal().as_u128(), 300);
    assert_eq!(borrow_position.collateral_asset_deposit.as_u128(), 500);
}
