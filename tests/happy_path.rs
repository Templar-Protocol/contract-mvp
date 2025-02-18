use templar_common::{borrow::BorrowStatus, market::YieldWeights, rational::Rational};
use test_utils::*;
use tokio::join;

#[test]
#[ignore = "generates a test configuration"]
fn gen_config() {
    println!(
        "{{\"configuration\":{}}}",
        near_sdk::serde_json::to_string(&market_configuration(
            "usdt.fakes.testnet".parse().unwrap(),
            "wrap.testnet".parse().unwrap(),
            "liquidator".parse().unwrap(),
            YieldWeights::new_with_supply_weight(1)
        ))
        .unwrap()
    );
}

#[tokio::test]
async fn test_happy() {
    let SetupEverything {
        c,
        supply_user,
        borrow_user,
        protocol_yield_user,
        insurance_yield_user,
        ..
    } = setup_everything(|_| {}).await;

    let configuration = c.get_configuration().await;

    assert_eq!(
        &configuration.collateral_asset.into_nep141().unwrap(),
        c.collateral_asset.id(),
    );
    assert_eq!(
        &configuration.borrow_asset.into_nep141().unwrap(),
        c.borrow_asset.id()
    );
    assert_eq!(
        configuration.minimum_collateral_ratio_per_borrow,
        Rational::new(120, 100)
    );

    // Step 1: Supply user sends tokens to contract to use for borrows.
    c.supply(&supply_user, 1100).await;

    let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();

    assert_eq!(
        supply_position.get_borrow_asset_deposit().as_u128(),
        1100,
        "Supply position should match amount of tokens supplied to contract",
    );

    let list_supplys = c.list_supplys().await;

    assert_eq!(
        list_supplys,
        [supply_user.id().clone()],
        "Supply user should be the only account listed",
    );

    // Step 2: Borrow user deposits collateral

    c.collateralize(&borrow_user, 2000).await;

    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(
        borrow_position.collateral_asset_deposit.as_u128(),
        2000,
        "Collateral asset deposit should be equal to the number of collateral tokens sent",
    );

    let list_borrows = c.list_borrows().await;

    assert_eq!(
        list_borrows,
        [borrow_user.id().clone()],
        "Borrow user should be the only account listed",
    );

    let borrow_status = c
        .get_borrow_status(borrow_user.id(), EQUAL_PRICE)
        .await
        .unwrap();

    assert_eq!(
        borrow_status,
        BorrowStatus::Healthy,
        "Borrow should be healthy when no assets are borrowed",
    );

    // Step 3: Withdraw some of the borrow asset

    // Borrowing 1000 borrow tokens with 2000 collateral tokens should be fine given equal price and MCR of 120%.
    c.borrow(&borrow_user, 1000, EQUAL_PRICE).await;

    let balance = c.borrow_asset_balance_of(borrow_user.id()).await;

    assert_eq!(balance, 1000, "Borrow user should receive assets");

    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(borrow_position.collateral_asset_deposit.as_u128(), 2000);
    assert_eq!(
        borrow_position.get_total_borrow_asset_liability().as_u128(),
        1000 + 100, // origination fee
    );

    // Step 4: Repay borrow

    // Need extra to pay for origination fee.
    c.borrow_asset_transfer(&supply_user, borrow_user.id(), 100)
        .await;

    c.repay(&borrow_user, 1100).await;

    // Ensure borrow is paid off.
    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(borrow_position.collateral_asset_deposit.as_u128(), 2000);
    assert_eq!(
        borrow_position.get_total_borrow_asset_liability().as_u128(),
        0
    );

    join!(
        // Supply withdrawals.
        async {
            // Withdraw yield.
            {
                c.harvest_yield(&supply_user).await;
                let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();
                assert_eq!(supply_position.borrow_asset_yield.amount.as_u128(), 80);

                let balance_before = c.borrow_asset_balance_of(supply_user.id()).await;
                // Withdraw all
                c.withdraw_supply_yield(&supply_user, None).await;
                let balance_after = c.borrow_asset_balance_of(supply_user.id()).await;

                assert_eq!(
                    balance_after - balance_before,
                    supply_position.borrow_asset_yield.amount.as_u128(),
                );
            }

            // Withdraw supply.
            {
                // Queue should be empty at first.
                let request_status = c
                    .get_supply_withdrawal_request_status(supply_user.id())
                    .await;
                assert!(
                    request_status.is_none(),
                    "Supply user should not be enqueued yet.",
                );
                let queue_status = c.get_supply_withdrawal_queue_status().await;
                assert!(queue_status.depth.is_zero());
                assert_eq!(queue_status.length, 0);

                let balance_before = c.borrow_asset_balance_of(supply_user.id()).await;
                c.create_supply_withdrawal_request(&supply_user, 1100).await;

                // Queue should have 1 request now.
                let request_status = c
                    .get_supply_withdrawal_request_status(supply_user.id())
                    .await
                    .expect("Should be enqueued now");
                assert_eq!(request_status.amount.as_u128(), 1100);
                assert_eq!(request_status.depth.as_u128(), 0);
                assert_eq!(request_status.index, 0);
                let queue_status = c.get_supply_withdrawal_queue_status().await;
                assert_eq!(queue_status.depth.as_u128(), 1100);
                assert_eq!(queue_status.length, 1);

                c.execute_next_supply_withdrawal_request(&supply_user).await;

                // Check the queue is empty again.
                let request_status = c
                    .get_supply_withdrawal_request_status(supply_user.id())
                    .await;
                assert!(
                    request_status.is_none(),
                    "Supply user should not be enqueued yet.",
                );
                let queue_status = c.get_supply_withdrawal_queue_status().await;
                assert!(queue_status.depth.is_zero());
                assert_eq!(queue_status.length, 0);

                let balance_after = c.borrow_asset_balance_of(supply_user.id()).await;

                assert_eq!(balance_after - balance_before, 1100);
            }
        },
        // Protocol yield.
        async {
            let protocol_yield = c.get_static_yield(protocol_yield_user.id()).await.unwrap();
            assert_eq!(protocol_yield.borrow_asset.as_u128(), 10);
            let balance_before = c.borrow_asset_balance_of(&protocol_yield_user.id()).await;
            c.withdraw_static_yield(&protocol_yield_user, None, None)
                .await;
            let balance_after = c.borrow_asset_balance_of(&protocol_yield_user.id()).await;
            assert_eq!(balance_after - balance_before, 10);
        },
        // Insurance yield.
        async {
            let insurance_yield = c.get_static_yield(insurance_yield_user.id()).await.unwrap();
            assert_eq!(insurance_yield.borrow_asset.as_u128(), 10);
            let balance_before = c.borrow_asset_balance_of(&insurance_yield_user.id()).await;
            c.withdraw_static_yield(&insurance_yield_user, None, None)
                .await;
            let balance_after = c.borrow_asset_balance_of(&insurance_yield_user.id()).await;
            assert_eq!(balance_after - balance_before, 10);
        },
        // Borrower withdraws collateral.
        async {
            let balance_before = c.collateral_asset_balance_of(borrow_user.id()).await;
            c.withdraw_collateral(&borrow_user, 2000, None).await;
            let balance_after = c.collateral_asset_balance_of(borrow_user.id()).await;
            assert_eq!(balance_after - balance_before, 2000);
            let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();
            assert!(!borrow_position.exists());
        },
    );
}
