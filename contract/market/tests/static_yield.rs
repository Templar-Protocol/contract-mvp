//! The original slept real
//! wall-clock to accrue interest; here we advance time with `fast_forward`. The
//! second test's mock `patch_storage_unregister` is replaced with the gateway
//! `storage::unregister` op.

use anyhow::Result;
use near_token::NearToken;
use rstest::rstest;
use templar_common::{dec, interest_rate_strategy::InterestRateStrategy, market::YieldWeights};
use templar_gateway_testing::{harness, DeployedMarket, SandboxHarness};
use templar_gateway_types::{ManagedAccountId, OperationStatus};

struct Fixture {
    market: DeployedMarket,
    protocol: ManagedAccountId,
    insurance: ManagedAccountId,
    borrow_user: ManagedAccountId,
}

/// Deploy a high-interest market with `protocol`/`insurance` as static-yield
/// recipients, supply liquidity, and post collateral.
#[allow(clippy::unwrap_used)] // infallible strategy construction, in a non-`#[test]` helper
async fn setup(harness: &SandboxHarness) -> Result<Fixture> {
    let protocol = harness.create_user("protocol").await?;
    let insurance = harness.create_user("insurance").await?;
    let protocol_id = protocol.0.clone();
    let insurance_id = insurance.0.clone();
    let market = harness
        .deploy_full_market_with(move |c| {
            c.borrow_interest_rate_strategy =
                InterestRateStrategy::linear(dec!("1000"), dec!("1000")).unwrap();
            c.yield_weights = YieldWeights::new_with_supply_weight(8)
                .with_static(protocol_id, 1)
                .with_static(insurance_id, 1);
        })
        .await?;
    harness.set_asset_prices(&market, 1.0, 1.0).await?;
    let supply_user = harness.create_user("supply").await?;
    let borrow_user = harness.create_user("borrow").await?;
    for user in [&protocol, &insurance, &supply_user, &borrow_user] {
        harness.fund_user(user, &market).await?;
    }
    // Register the recipients on the market so they can hold yield records.
    for user in [&protocol, &insurance] {
        harness
            .storage_deposit(user, &market.market_id, NearToken::from_millinear(50))
            .await?;
    }

    harness
        .supply_and_harvest_until_activation(&supply_user, &market, 10_000_000)
        .await?;
    harness
        .collateralize(&borrow_user, &market, 2_000_000)
        .await?;

    Ok(Fixture {
        market,
        protocol,
        insurance,
        borrow_user,
    })
}

#[rstest]
#[tokio::test]
async fn static_yield_success(#[future(awt)] harness: SandboxHarness) -> Result<()> {
    let Fixture {
        market,
        protocol,
        insurance,
        borrow_user,
    } = setup(&harness).await?;

    // No record before any accumulation, and a zero record after a no-op one.
    assert_eq!(
        harness.static_yield_record(&market, &protocol.0).await?,
        None
    );
    harness
        .accumulate_static_yield(&protocol, &market, None, None)
        .await?;
    assert_eq!(
        harness.static_yield_record(&market, &protocol.0).await?,
        Some(0),
    );

    // Accrue interest, then realize it as static yield.
    harness.borrow(&borrow_user, &market, 1_000_000).await?;
    harness.fast_forward(200).await?;
    harness
        .repay(&borrow_user, &market, 1_200_000, None)
        .await?;

    assert_eq!(harness.static_yield_total(&market, &protocol.0).await?, 0);
    harness
        .accumulate_static_yield(&protocol, &market, None, None)
        .await?;
    let accumulated = harness.static_yield_total(&market, &protocol.0).await?;
    assert_ne!(accumulated, 0);

    // Anyone can accumulate another account's yield.
    harness
        .accumulate_static_yield(&protocol, &market, Some(insurance.0.clone()), None)
        .await?;
    assert!(harness.static_yield_total(&market, &insurance.0).await? >= accumulated);

    // Partial then full withdrawal.
    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &protocol.0)
        .await?;
    harness
        .withdraw_static_yield(&protocol, &market, Some(1))
        .await?;
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &protocol.0)
            .await?,
        balance_before + 1,
    );
    assert_eq!(
        harness.static_yield_total(&market, &protocol.0).await?,
        accumulated - 1,
    );

    harness
        .withdraw_static_yield(&protocol, &market, None)
        .await?;
    assert_eq!(harness.static_yield_total(&market, &protocol.0).await?, 0);
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &protocol.0)
            .await?,
        balance_before + accumulated,
    );

    Ok(())
}

#[rstest]
#[tokio::test]
async fn static_yield_withdrawal_beyond_total_is_rejected(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let Fixture {
        market,
        protocol,
        borrow_user,
        ..
    } = setup(&harness).await?;

    harness.borrow(&borrow_user, &market, 1_000_000).await?;
    harness.fast_forward(200).await?;
    harness
        .repay(&borrow_user, &market, 1_200_000, None)
        .await?;
    harness
        .accumulate_static_yield(&protocol, &market, None, None)
        .await?;

    let accumulated = harness.static_yield_total(&market, &protocol.0).await?;
    assert_ne!(accumulated, 0);
    let balance_before = harness
        .ft_balance_of(&market.borrow_ft_id, &protocol.0)
        .await?;

    let result = harness
        .try_withdraw_static_yield(&protocol, &market, Some(accumulated + 1))
        .await?;
    assert_eq!(result.operation.status, OperationStatus::Failed);
    assert!(
        result
            .operation
            .failure_message()
            .unwrap_or_default()
            .contains("Attempt to withdraw more than accumulated static yield"),
        "unexpected failure reason: {:?}",
        result.operation.failure_message(),
    );
    assert_eq!(
        harness.static_yield_total(&market, &protocol.0).await?,
        accumulated,
        "an oversized withdrawal must not reduce static yield",
    );
    assert_eq!(
        harness
            .ft_balance_of(&market.borrow_ft_id, &protocol.0)
            .await?,
        balance_before,
        "an oversized withdrawal must not transfer tokens",
    );

    Ok(())
}

#[rstest]
#[tokio::test]
async fn static_yield_withdrawal_blocked_when_unregistered(
    #[future(awt)] harness: SandboxHarness,
) -> Result<()> {
    let Fixture {
        market,
        protocol,
        borrow_user,
        ..
    } = setup(&harness).await?;

    harness.borrow(&borrow_user, &market, 1_000_000).await?;
    harness.fast_forward(200).await?;
    harness
        .repay(&borrow_user, &market, 1_200_000, None)
        .await?;

    harness
        .accumulate_static_yield(&protocol, &market, None, None)
        .await?;
    let accumulated = harness.static_yield_total(&market, &protocol.0).await?;
    assert_ne!(accumulated, 0);

    // Unregister from the borrow token so the yield transfer cannot land.
    harness
        .storage_unregister(&protocol, &market.borrow_ft_id, true)
        .await?;

    // The gateway op reports top-level success even though the yield transfer
    // fails on the unregistered receiver (the failure surfaces in a callback,
    // not the outer tx — ENG-407), so we assert the *effect*: the withdrawal
    // record is restored rather than consumed.
    harness
        .try_withdraw_static_yield(&protocol, &market, None)
        .await?;

    // The record is preserved when the withdrawal transfer fails.
    assert_eq!(
        harness.static_yield_total(&market, &protocol.0).await?,
        accumulated,
    );

    Ok(())
}
