use near_sdk::{
    json_types::U128,
    serde_json::{self, json},
    AccountId, AccountIdRef, NearToken,
};
use near_workspaces::{
    network::Sandbox, prelude::TopLevelAccountCreator, Account, Contract, DevNetwork, Worker,
};
use templar_common::{
    asset::FungibleAsset,
    borrow::{BorrowPosition, BorrowStatus},
    fee::{Fee, TimeBasedFee, TimeBasedFeeFunction},
    market::{
        LiquidationSpread, MarketConfiguration, Nep141MarketDepositMessage, OraclePriceProof,
    },
    rational::Rational,
    supply::SupplyPosition,
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
    account: Account,
    name: &str,
    symbol: &str,
    owner_id: &AccountIdRef,
    supply: u128,
) -> Contract {
    let wasm = near_workspaces::compile_project("./mock/ft/")
        .await
        .unwrap();
    let contract = account.deploy(&wasm).await.unwrap().unwrap();
    contract
        .call("new")
        .args_json(json!({
            "name": name,
            "symbol": symbol,
            "owner_id": owner_id,
            "supply": U128(supply),
        }))
        .deposit(NearToken::from_near(1))
        .transact()
        .await
        .unwrap()
        .unwrap();

    contract
}

// ===== TESTS =====

#[tokio::test]
async fn test_market_happy_path() {
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
    let borrow_asset = deploy_ft(
        borrow_asset,
        "Borrow Asset",
        "BORROW",
        supply_user.id(),
        1000,
    )
    .await;
    let collateral_asset = deploy_ft(
        collateral_asset,
        "Collateral Asset",
        "COLLATERAL",
        borrow_user.id(),
        1000,
    )
    .await;

    // Asset opt-ins.
    let storage_deposit = move |account: &Account, asset_id: &AccountId| {
        account
            .call(asset_id, "storage_deposit")
            .args_json(json!({}))
            .deposit(NearToken::from_near(1))
            .transact()
    };
    storage_deposit(contract.as_account(), borrow_asset.id())
        .await
        .unwrap()
        .unwrap();
    storage_deposit(contract.as_account(), collateral_asset.id())
        .await
        .unwrap()
        .unwrap();
    storage_deposit(&borrow_user, borrow_asset.id())
        .await
        .unwrap()
        .unwrap();

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

    println!("{} depositing 100 tokens for supply...", supply_user.id());

    // Step 1: Supply user sends tokens to contract to use for borrows.
    supply_user
        .call(borrow_asset.id(), "ft_transfer_call")
        .args_json(json!({
            "receiver_id": contract.id(),
            "amount": U128(100),
            "msg": serde_json::to_string(&Nep141MarketDepositMessage::Supply).unwrap(),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await
        .unwrap()
        .unwrap();

    println!("Checking supply position...");

    let supply_position = contract
        .view("get_supply_position")
        .args_json(json!({
            "account_id": supply_user.id(),
        }))
        .await
        .unwrap()
        .json::<Option<SupplyPosition>>()
        .unwrap()
        .unwrap();

    assert_eq!(
        supply_position.borrow_asset_deposited.0, 100,
        "Supply position should match amount of tokens supplied to contract",
    );

    let list_supplys = contract
        .view("list_supplys")
        .args_json(json!({}))
        .await
        .unwrap()
        .json::<Vec<AccountId>>()
        .unwrap();

    assert_eq!(
        list_supplys,
        [supply_user.id().clone()],
        "Supply user should be the only account listed",
    );

    // Step 2: Borrow user deposits collateral

    println!(
        "{} depositing 200 tokens for collateral...",
        borrow_user.id(),
    );

    borrow_user
        .call(collateral_asset.id(), "ft_transfer_call")
        .args_json(json!({
            "receiver_id": contract.id(),
            "amount": U128(200),
            "msg": serde_json::to_string(&Nep141MarketDepositMessage::Collateralize).unwrap(),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await
        .unwrap()
        .unwrap();

    let borrow_position = contract
        .view("get_borrow_position")
        .args_json(json!({
            "account_id": borrow_user.id(),
        }))
        .await
        .unwrap()
        .json::<Option<BorrowPosition>>()
        .unwrap()
        .unwrap();

    assert_eq!(
        borrow_position.collateral_asset_deposit.0, 200,
        "Collateral asset deposit should be equal to the number of collateral tokens sent",
    );

    let list_borrows = contract
        .view("list_borrows")
        .args_json(json!({}))
        .await
        .unwrap()
        .json::<Vec<AccountId>>()
        .unwrap();

    assert_eq!(
        list_borrows,
        [borrow_user.id().clone()],
        "Borrow user should be the only account listed",
    );

    let equal_price = OraclePriceProof {
        collateral_asset_price: Rational::new(1, 1),
        borrow_asset_price: Rational::new(1, 1),
    };

    let borrow_status = contract
        .view("get_borrow_status")
        .args_json(json!({
            "account_id": borrow_user.id(),
            "oracle_price_proof": equal_price,
        }))
        .await
        .unwrap()
        .json::<Option<BorrowStatus>>()
        .unwrap()
        .unwrap();

    assert_eq!(
        borrow_status,
        BorrowStatus::Healthy,
        "Borrow should be healthy when no assets are borrowed",
    );

    // Step 3: Withdraw some of the borrow asset

    println!("Requesting borrow...");

    // fn borrow(&mut self, amount: U128, oracle_price_proof: OraclePriceProof) -> PromiseOrValue<()>;
    // Borrowing 100 borrow tokens with 200 collateral tokens should be fine given equal price and MCR of 120%.
    borrow_user
        .call(contract.id(), "borrow")
        .args_json(json!({
            "amount": U128(100),
            "oracle_price_proof": equal_price,
        }))
        .transact()
        .await
        .unwrap()
        .unwrap();

    let balance = borrow_asset
        .view("ft_balance_of")
        .args_json(json!({
            "account_id": borrow_user.id(),
        }))
        .await
        .unwrap()
        .json::<U128>()
        .unwrap();

    assert_eq!(balance.0, 100, "Borrow user should receive assets");
}
