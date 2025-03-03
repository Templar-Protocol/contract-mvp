use rstest::rstest;

use templar_common::{fee::Fee, market::OraclePriceProof, number::Decimal};
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

// Caveat to this test: Make sure that the yield distribution value is
// divisible by 10 for easy maths.
#[rstest]
#[case(110, 5000, 2450, 50, 2500)]
#[case(120, 1250, 1000, 88, 1100)] // fmv
#[case(120, 1250, 1000, 88, 1070)] // liquidator spread of ~2.7%
#[tokio::test]
async fn successful_liquidation_good_debt_under_mcr(
    #[case] mcr: u16,
    #[case] collateral_amount: u128,
    #[case] borrow_amount: u128,
    #[case] collateral_asset_price_pct: u128,
    #[case] liquidation_amount: u128,
) {
    let SetupEverything {
        c,
        liquidator_user,
        supply_user,
        borrow_user,
        protocol_yield_user,
        insurance_yield_user,
        ..
    } = setup_everything(|config| {
        config.borrow_origination_fee = Fee::zero();
        config.minimum_collateral_ratio_per_borrow = Decimal::from(mcr) / 100u32;
    })
    .await;

    c.supply(&supply_user, 10000).await;
    c.collateralize(&borrow_user, collateral_amount).await;
    c.borrow(&borrow_user, borrow_amount, EQUAL_PRICE).await;

    let collateral_balance_before = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset_balance_of(liquidator_user.id()).await;

    c.liquidate(
        &liquidator_user,
        borrow_user.id(),
        liquidation_amount,
        OraclePriceProof {
            collateral_asset_price: Decimal::from(collateral_asset_price_pct) / 100u32,
            borrow_asset_price: Decimal::one(),
        },
    )
    .await;

    let collateral_balance_after = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset_balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        collateral_amount,
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        liquidation_amount,
        "Liquidation should transfer correct amount of tokens",
    );

    let yield_amount = liquidation_amount - borrow_amount;

    tokio::join!(
        async {
            c.harvest_yield(&supply_user).await;
            let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();
            assert_eq!(
                supply_position.borrow_asset_yield.amount.as_u128(),
                yield_amount * 8 / 10,
            );
        },
        async {
            let protocol_yield = c.get_static_yield(protocol_yield_user.id()).await.unwrap();
            assert_eq!(protocol_yield.borrow_asset.as_u128(), yield_amount / 10);
        },
        async {
            let insurance_yield = c.get_static_yield(insurance_yield_user.id()).await.unwrap();
            assert_eq!(insurance_yield.borrow_asset.as_u128(), yield_amount / 10,);
        },
    );
}

#[rstest]
#[case(120, 5, 0)]
#[case(120, 5, 2)]
#[case(120, 5, 5)]
#[case(110, 2, 1)]
#[case(150, 33, 32)]
#[tokio::test]
async fn successful_liquidation_with_spread(
    #[case] mcr: u16,
    #[case] maximum_spread_pct: u16,
    #[case] spread_pct: u16,
) {
    assert!(spread_pct <= maximum_spread_pct);

    let maximum_liquidator_spread: Decimal = Decimal::from(maximum_spread_pct) / 100u32;
    let target_spread: Decimal = Decimal::from(spread_pct) / 100u32;
    let mcr: Decimal = Decimal::from(mcr) / 100u32;

    let SetupEverything {
        c,
        liquidator_user,
        supply_user,
        borrow_user,
        ..
    } = setup_everything(|config| {
        config.minimum_collateral_ratio_per_borrow = mcr.clone();
        config.maximum_liquidator_spread = maximum_liquidator_spread;
    })
    .await;

    c.supply(&supply_user, 10000).await;
    c.collateralize(&borrow_user, 2000).await; // 2:1 collateralization
    c.borrow(&borrow_user, 1000, EQUAL_PRICE).await;

    let collateral_balance_before = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_before = c.borrow_asset_balance_of(liquidator_user.id()).await;

    let collateral_asset_price: Decimal = mcr /
        201u32 * 100u32 // 2:1 collateralization + a bit to ensure we're under MCR
        ;

    let liquidation_amount = (&collateral_asset_price * (1u32 - target_spread) * 2000u32)
        .to_u128_ceil()
        .unwrap();

    c.liquidate(
        &liquidator_user,
        borrow_user.id(),
        liquidation_amount,
        OraclePriceProof {
            collateral_asset_price,
            borrow_asset_price: Decimal::one(),
        },
    )
    .await;

    let collateral_balance_after = c.collateral_asset_balance_of(liquidator_user.id()).await;
    let borrow_balance_after = c.borrow_asset_balance_of(liquidator_user.id()).await;

    assert_eq!(
        collateral_balance_after - collateral_balance_before,
        2000,
        "Liquidator should obtain all collateral after a successful liquidation",
    );
    assert_eq!(
        borrow_balance_before - borrow_balance_after,
        liquidation_amount,
        "Liquidation should transfer correct amount of tokens",
    );
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
