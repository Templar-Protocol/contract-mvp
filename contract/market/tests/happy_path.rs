//! Parametrized over the four
//! NEP-141 / NEP-245 (multi-token) asset combinations. The market's asset path
//! is standard-agnostic, so the full lifecycle — supply, collateralize, borrow,
//! repay, and the 8/1/1 yield split — runs identically for each; only the
//! deployed asset contracts and the config's asset shape differ.

use anyhow::{Context, Result};
use rstest::rstest;
use templar_common::{
    borrow::BorrowStatus, dec, fee::Fee, interest_rate_strategy::InterestRateStrategy,
    market::YieldWeights, Decimal,
};
use templar_gateway_testing::{failed_receipts, harness, SandboxHarness};

#[rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn test_happy(
    #[future(awt)] harness: SandboxHarness,
    #[case] borrow_mt: bool,
    #[case] collateral_mt: bool,
) -> Result<()> {
    let protocol = harness.create_user("protocol").await?;
    let insurance = harness.create_user("insurance").await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;

    let protocol_id = protocol.0.clone();
    let insurance_id = insurance.0.clone();
    let market = harness
        .deploy_full_market_std(borrow_mt, collateral_mt, move |c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(Decimal::ZERO, Decimal::ZERO).unwrap();
            // Pin the 10% origination fee this test's 1000 -> 1100 liability
            // assumes, rather than depending on the shared helper default.
            c.borrow_origination_fee = Fee::Proportional(dec!("0.1"));
            c.yield_weights = YieldWeights::new_with_supply_weight(8)
                .with_static(protocol_id, 1)
                .with_static(insurance_id, 1);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    for user in [&protocol, &insurance, &supply_user, &borrow_user] {
        harness.fund_user(user, &market).await?;
    }

    // The deployed configuration carries each asset in the requested standard.
    let deployed = harness.get_configuration(&market.market_id).await?;
    assert_eq!(deployed.borrow_asset.nep245_token_id().is_some(), borrow_mt);
    assert_eq!(
        deployed.collateral_asset.nep245_token_id().is_some(),
        collateral_mt,
    );

    assert!(market
        .configuration
        .borrow_mcr_liquidation
        .near_equal(dec!("1.2")));

    // The market charges a nonzero storage stake per registration.
    assert!(!harness
        .storage_balance_bounds(&market.market_id)
        .await?
        .min
        .is_zero());

    // A single snapshot is generated on init, with no activity.
    let snapshots = harness.list_finalized_snapshots(&market).await?;
    assert_eq!(
        snapshots.len(),
        1,
        "should generate a single snapshot on init"
    );
    assert!(snapshots[0].yield_distribution.is_zero());
    assert!(snapshots[0].borrow_asset_deposited_active.is_zero());
    assert!(snapshots[0].borrow_asset_borrowed.is_zero());

    // Supply, then activate.
    harness.supply(&supply_user, &market, 1100).await?;
    assert_eq!(
        u128::from(
            harness
                .get_supply_position(&market, &supply_user.0)
                .await?
                .context("supply position missing")?
                .total_incoming()
        ),
        1100,
    );
    while !harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .context("supply position missing")?
        .get_deposit()
        .incoming
        .is_empty()
    {
        harness
            .harvest_yield(&supply_user, &market, Some(supply_user.0.clone()))
            .await?;
    }
    assert_eq!(
        u128::from(
            harness
                .get_supply_position(&market, &supply_user.0)
                .await?
                .context("supply position missing")?
                .get_deposit()
                .active
        ),
        1100,
    );

    // Collateralize, and confirm healthy with nothing borrowed.
    harness.collateralize(&borrow_user, &market, 2000).await?;
    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .collateral_asset_deposit
        ),
        2000,
    );
    let prices = harness.get_oracle_prices(&market).await?;
    assert_eq!(
        harness
            .get_borrow_status(&market, &borrow_user.0, prices)
            .await?
            .context("borrow status missing")?,
        BorrowStatus::Healthy,
    );

    // Borrow (1000 + 100 origination fee).
    let balance_before = harness
        .asset_balance_of(&market.configuration.borrow_asset, &borrow_user.0)
        .await?;
    harness.borrow(&borrow_user, &market, 1000).await?;
    assert_eq!(
        harness
            .asset_balance_of(&market.configuration.borrow_asset, &borrow_user.0)
            .await?,
        balance_before + 1000,
    );
    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .get_total_borrow_asset_liability()
        ),
        1100,
    );
    let near = harness.gateway_client();
    let market_client = near.market(market.market_id.clone());
    assert_eq!(
        u128::from(
            market_client
                .get_borrow_asset_metrics(())
                .await?
                .paid_to_fees
        ),
        0,
    );

    // Repay in full.
    harness.repay(&borrow_user, &market, 1100, None).await?;
    assert_eq!(
        u128::from(
            harness
                .get_borrow_position(&market, &borrow_user.0)
                .await?
                .context("borrow position missing")?
                .get_total_borrow_asset_liability()
        ),
        0,
    );
    assert_eq!(
        u128::from(
            market_client
                .get_borrow_asset_metrics(())
                .await?
                .paid_to_fees
        ),
        100,
    );

    // Advance beyond the fixture's 1 ms snapshot boundary before harvesting the repaid fee.
    harness.fast_forward(10).await?;

    // The 100 origination fee is split 8/1/1 across supply / protocol / insurance.

    // Supply yield: 80.
    harness
        .harvest_yield(&supply_user, &market, Some(supply_user.0.clone()))
        .await?;
    assert_eq!(
        u128::from(
            harness
                .get_supply_position(&market, &supply_user.0)
                .await?
                .context("supply position missing")?
                .borrow_asset_yield
                .get_total()
        ),
        80,
    );
    let balance_before = harness
        .asset_balance_of(&market.configuration.borrow_asset, &supply_user.0)
        .await?;
    harness
        .create_supply_withdrawal_request(&supply_user, &market, 80)
        .await?;
    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    assert_eq!(
        harness
            .asset_balance_of(&market.configuration.borrow_asset, &supply_user.0)
            .await?,
        balance_before + 80,
    );
    // The position's yield ledger is cleared once the yield is withdrawn.
    assert!(
        harness
            .get_supply_position(&market, &supply_user.0)
            .await?
            .context("supply position missing")?
            .borrow_asset_yield
            .get_total()
            .is_zero(),
        "supply position should carry no yield after withdrawing it all",
    );
    assert_eq!(
        u128::from(
            market_client
                .get_borrow_asset_metrics(())
                .await?
                .paid_to_fees
        ),
        20,
    );

    // Withdraw the supplied principal (1100); the position then closes.
    let balance_before = harness
        .asset_balance_of(&market.configuration.borrow_asset, &supply_user.0)
        .await?;
    harness
        .create_supply_withdrawal_request(&supply_user, &market, 1100)
        .await?;
    harness
        .execute_next_supply_withdrawal_request(&supply_user, &market, None)
        .await?;
    assert_eq!(
        harness
            .asset_balance_of(&market.configuration.borrow_asset, &supply_user.0)
            .await?,
        balance_before + 1100,
    );
    assert!(harness
        .get_supply_position(&market, &supply_user.0)
        .await?
        .is_none());
    assert_eq!(
        u128::from(
            market_client
                .get_borrow_asset_metrics(())
                .await?
                .paid_to_fees
        ),
        20,
    );

    // Protocol and insurance static yield: 10 each, paid in the borrow asset and
    // touching only it.
    for recipient in [&protocol, &insurance] {
        harness
            .accumulate_static_yield(recipient, &market, Some(recipient.0.clone()), None)
            .await?;
        assert_eq!(harness.static_yield_total(&market, &recipient.0).await?, 10);
        let balance_before = harness
            .asset_balance_of(&market.configuration.borrow_asset, &recipient.0)
            .await?;
        let collateral_before = harness
            .asset_balance_of(&market.configuration.collateral_asset, &recipient.0)
            .await?;
        let result = harness
            .withdraw_static_yield(recipient, &market, None)
            .await?;
        assert!(
            failed_receipts(&result).next().is_none(),
            "static-yield withdrawal should not fail any receipt",
        );
        assert_eq!(
            harness
                .asset_balance_of(&market.configuration.borrow_asset, &recipient.0)
                .await?,
            balance_before + 10,
        );
        assert_eq!(
            harness
                .asset_balance_of(&market.configuration.collateral_asset, &recipient.0)
                .await?,
            collateral_before,
            "static-yield withdrawal must not move the collateral asset",
        );
    }

    // Borrower withdraws all collateral; the borrow position closes.
    let balance_before = harness
        .asset_balance_of(&market.configuration.collateral_asset, &borrow_user.0)
        .await?;
    harness
        .withdraw_collateral(&borrow_user, &market, 2000)
        .await?;
    assert_eq!(
        harness
            .asset_balance_of(&market.configuration.collateral_asset, &borrow_user.0)
            .await?,
        balance_before + 2000,
    );
    assert!(harness
        .get_borrow_position(&market, &borrow_user.0)
        .await?
        .is_none());

    Ok(())
}
