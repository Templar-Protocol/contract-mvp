use near_sdk::{
    json_types::{U128, U64},
    serde_json::json,
    AccountId, AccountIdRef, NearToken,
};
use near_workspaces::{
    network::Sandbox, operations::Function, prelude::TopLevelAccountCreator, Account, Contract,
    DevNetwork, Worker,
};
use templar_common::{
    asset::FungibleAsset,
    fee::{Fee, TimeBasedFee, TimeBasedFeeFunction},
    market::{LiquidationSpread, MarketConfiguration},
    rational::Rational,
};

// ===== UTIL FUNCTIONS =====

async fn create_prefixed_account<T: DevNetwork + TopLevelAccountCreator + 'static>(
    prefix: &str,
    worker: &near_workspaces::Worker<T>,
) -> Account {
    let (genid, sk) = worker.dev_generate().await;
    let new_id: AccountId = format!("{prefix}{}", &genid.as_str()[prefix.len()..])
        .parse()
        .unwrap();
    worker.create_tla(new_id, sk).await.unwrap().unwrap()
}

macro_rules! accounts {
    ($w: ident, $($n:ident),*) => {
        let ($($n,)*) = tokio::join!( $(create_prefixed_account(stringify!($n), &$w)),* );
    };
}

async fn deploy_mock_ft(
    worker: &Worker<Sandbox>,
    account_id: AccountId,
    name: &str,
    symbol: &str,
    total_supply: u128,
) -> Contract {
    let contract = worker
        .dev_deploy(&near_workspaces::compile_project("./mock/ft").await.unwrap())
        .await
        .unwrap();

    contract
        .call("new")
        .args_json(json!({
            "name": name,
            "symbol": symbol,
            "owner_id": account_id,
            "supply": U128(total_supply),
        }))
        .transact()
        .await
        .unwrap()
        .unwrap();

    contract
}

fn market_configuration(
    borrow_asset_id: AccountId,
    collateral_asset_id: AccountId,
    liquidator_account_id: AccountId,
) -> MarketConfiguration {
    MarketConfiguration {
        borrow_asset: FungibleAsset::Nep141(borrow_asset_id),
        collateral_asset: FungibleAsset::Nep141(collateral_asset_id),
        balance_oracle_account_id: "balance_oracle".parse().unwrap(),
        liquidator_account_id,
        minimum_collateral_ratio_per_borrow: Rational::new(120, 100),
        maximum_borrow_asset_usage_ratio: Rational::new(99, 100),
        origination_fee: Fee::Proportional(Rational::new(1, 100)),
        annual_maintenance_fee: Fee::Flat(0.into()),
        maximum_borrow_duration: None,
        minimum_borrow_amount: 1.into(),
        maximum_borrow_amount: u128::MAX.into(),
        withdrawal_fee: TimeBasedFee {
            fee: Fee::Flat(0.into()),
            duration: 0.into(),
            behavior: TimeBasedFeeFunction::Fixed,
        },
        liquidation_spread: LiquidationSpread {
            supply_position: 8.into(),
            liquidator: 1.into(),
            protocol: 1.into(),
        },
    }
}

async fn setup_market(worker: &Worker<Sandbox>, configuration: MarketConfiguration) -> Contract {
    let contract_wasm = near_workspaces::compile_project("./").await.unwrap();

    let contract = worker.dev_deploy(&contract_wasm).await.unwrap();
    contract
        .call("new")
        .args_json(json!({
            "configuration": configuration,
        }))
        .transact()
        .await
        .unwrap()
        .unwrap();

    contract
}

async fn deploy_ft(
    account: &Account,
    name: &str,
    symbol: &str,
    owner_id: &AccountIdRef,
    supply: u128,
) {
    let wasm = near_workspaces::compile_project("./mock/ft/")
        .await
        .unwrap();
    account
        .batch(account.id())
        .deploy(&wasm)
        .call(
            Function::new("new")
                .args_json(json!({
                    "name": name,
                    "symbol": symbol,
                    "owner_id": owner_id,
                    "supply": U128(supply),
                }))
                .deposit(NearToken::from_near(1)),
        )
        .transact()
        .await
        .unwrap()
        .unwrap();
}

// ===== TESTS =====

#[tokio::test]
async fn test_create_market() {
    let worker = near_workspaces::sandbox().await.unwrap();
    accounts!(
        worker,
        owner_user,
        supply_user,
        borrow_user,
        collateral_asset,
        borrow_asset
    );
    let contract = setup_market(
        &worker,
        market_configuration(
            borrow_asset.id().clone(),
            collateral_asset.id().clone(),
            owner_user.id().clone(),
        ),
    )
    .await;
    deploy_ft(
        &borrow_asset,
        "Borrow Asset",
        "BORROW",
        supply_user.id(),
        1000,
    )
    .await;
    deploy_ft(
        &collateral_asset,
        "Collateral Asset",
        "COLLATERAL",
        borrow_user.id(),
        1000,
    )
    .await;

    let configuration = contract
        .view("get_configuration")
        .args_json(json!({}))
        .await
        .unwrap()
        .json::<MarketConfiguration>()
        .unwrap();

    assert_eq!(
        &configuration.collateral_asset.into_nep141().unwrap(),
        collateral_asset.id(),
    );
    assert_eq!(
        &configuration.borrow_asset.into_nep141().unwrap(),
        borrow_asset.id()
    );
    assert_eq!(
        configuration.minimum_collateral_ratio_per_borrow,
        Rational::new(120, 100)
    );
}
