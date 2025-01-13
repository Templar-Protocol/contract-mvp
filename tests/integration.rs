use near_sdk::{
    json_types::{U128, U64},
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
    market::{MarketConfiguration, Nep141MarketDepositMessage, OraclePriceProof, YieldWeights},
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
        borrow_asset: FungibleAsset::nep141(borrow_asset_id),
        collateral_asset: FungibleAsset::nep141(collateral_asset_id),
        balance_oracle_account_id: "balance_oracle".parse().unwrap(),
        liquidator_account_id,
        minimum_collateral_ratio_per_borrow: Rational::new(120, 100),
        maximum_borrow_asset_usage_ratio: Rational::new(99, 100),
        borrow_origination_fee: Fee::Proportional(Rational::new(10, 100)),
        borrow_annual_maintenance_fee: Fee::Flat(0.into()),
        maximum_borrow_duration_ms: None,
        minimum_borrow_amount: 1.into(),
        maximum_borrow_amount: u128::MAX.into(),
        maximum_liquidator_spread: Rational::new(5, 100),
        supply_withdrawal_fee: TimeBasedFee {
            fee: Fee::Flat(0.into()),
            duration: 0.into(),
            behavior: TimeBasedFeeFunction::Fixed,
        },
        yield_weights: YieldWeights::new_with_supply_weight(8)
            .with_static("protocol".parse().unwrap(), 1)
            .with_static("insurance".parse().unwrap(), 1),
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

struct TestController {
    contract: Contract,
    borrow_asset: Contract,
    collateral_asset: Contract,
}

impl TestController {
    async fn storage_deposits(&self, account: &Account) {
        println!("Performing storage deposits for {}...", account.id());
        account
            .call(self.borrow_asset.id(), "storage_deposit")
            .args_json(json!({}))
            .deposit(NearToken::from_near(1))
            .transact()
            .await
            .unwrap()
            .unwrap();
        account
            .call(self.collateral_asset.id(), "storage_deposit")
            .args_json(json!({}))
            .deposit(NearToken::from_near(1))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    async fn get_configuration(&self) -> MarketConfiguration {
        self.contract
            .view("get_configuration")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<MarketConfiguration>()
            .unwrap()
    }

    async fn supply(&self, supply_user: &Account, amount: u128) {
        println!(
            "{} transferring {amount} tokens for supply...",
            supply_user.id()
        );
        supply_user
            .call(self.borrow_asset.id(), "ft_transfer_call")
            .args_json(json!({
                "receiver_id": self.contract.id(),
                "amount": U128(amount),
                "msg": serde_json::to_string(&Nep141MarketDepositMessage::Supply).unwrap(),
            }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    async fn get_supply_position(&self, account_id: &AccountId) -> Option<SupplyPosition> {
        self.contract
            .view("get_supply_position")
            .args_json(json!({
                "account_id": account_id,
            }))
            .await
            .unwrap()
            .json::<Option<SupplyPosition>>()
            .unwrap()
    }

    async fn list_supplys(&self) -> Vec<AccountId> {
        self.contract
            .view("list_supplys")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<Vec<AccountId>>()
            .unwrap()
    }

    async fn collateralize(&self, borrow_user: &Account, amount: u128) {
        println!(
            "{} transferring {amount} tokens for collateral...",
            borrow_user.id(),
        );
        borrow_user
            .call(self.collateral_asset.id(), "ft_transfer_call")
            .args_json(json!({
                "receiver_id": self.contract.id(),
                "amount": U128(amount),
                "msg": serde_json::to_string(&Nep141MarketDepositMessage::Collateralize).unwrap(),
            }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    async fn get_borrow_position(&self, account_id: &AccountId) -> Option<BorrowPosition> {
        self.contract
            .view("get_borrow_position")
            .args_json(json!({
                "account_id": account_id,
            }))
            .await
            .unwrap()
            .json::<Option<BorrowPosition>>()
            .unwrap()
    }

    async fn list_borrows(&self) -> Vec<AccountId> {
        self.contract
            .view("list_borrows")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<Vec<AccountId>>()
            .unwrap()
    }

    async fn get_borrow_status(
        &self,
        account_id: &AccountId,
        price: OraclePriceProof,
    ) -> Option<BorrowStatus> {
        self.contract
            .view("get_borrow_status")
            .args_json(json!({
                "account_id": account_id,
                "oracle_price_proof": price,
            }))
            .await
            .unwrap()
            .json::<Option<BorrowStatus>>()
            .unwrap()
    }

    async fn borrow(&self, borrow_user: &Account, amount: u128, price: OraclePriceProof) {
        println!("{} borrowing {amount} tokens...", borrow_user.id());
        borrow_user
            .call(self.contract.id(), "borrow")
            .args_json(json!({
                "amount": U128(amount),
                "oracle_price_proof": price,
            }))
            .max_gas()
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    async fn borrow_asset_balance_of(&self, account_id: &AccountId) -> u128 {
        self.borrow_asset
            .view("ft_balance_of")
            .args_json(json!({
                "account_id": account_id,
            }))
            .await
            .unwrap()
            .json::<U128>()
            .unwrap()
            .0
    }

    async fn asset_transfer(
        &self,
        asset_id: &AccountId,
        sender: &Account,
        receiver_id: &AccountId,
        amount: u128,
    ) {
        println!(
            "{} sending {amount} tokens of {asset_id} to {receiver_id}...",
            sender.id(),
        );
        sender
            .call(asset_id, "ft_transfer")
            .args_json(json!({
                "receiver_id": receiver_id,
                "amount": U128(amount),
            }))
            .deposit(NearToken::from_yoctonear(1))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    async fn borrow_asset_transfer(&self, sender: &Account, receiver_id: &AccountId, amount: u128) {
        self.asset_transfer(self.borrow_asset.id(), sender, receiver_id, amount)
            .await;
    }

    async fn repay(&self, borrow_user: &Account, amount: u128) {
        println!("{} repaying {amount} tokens...", borrow_user.id());
        borrow_user
            .call(self.borrow_asset.id(), "ft_transfer_call")
            .args_json(json!({
                "receiver_id": self.contract.id(),
                "amount": U128(amount),
                "msg": serde_json::to_string(&Nep141MarketDepositMessage::Repay).unwrap(),
            }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    async fn harvest_yield(&self, supply_user: &Account) {
        println!("{} harvesting yield...", supply_user.id());
        supply_user
            .call(self.contract.id(), "harvest_yield")
            .args_json(json!({}))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    async fn print_logs(&self) {
        let total_borrow_asset_deposited_log = self
            .contract
            .view("get_total_borrow_asset_deposited_log")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<Vec<(U64, U128)>>()
            .unwrap();

        println!("Total borrow asset deposited log:");
        for (i, (U64(block_height), U128(amount))) in
            total_borrow_asset_deposited_log.iter().enumerate()
        {
            println!("\t{i}: {amount}\t[#{block_height}]");
        }

        let borrow_asset_yield_distribution_log = self
            .contract
            .view("get_borrow_asset_yield_distribution_log")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<Vec<(U64, U128)>>()
            .unwrap();

        println!("Borrow asset yield distribution log:");
        for (i, (U64(block_height), U128(amount))) in
            borrow_asset_yield_distribution_log.iter().enumerate()
        {
            println!("\t{i}: {amount}\t[#{block_height}]");
        }
    }
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

    let c = TestController {
        contract,
        collateral_asset,
        borrow_asset,
    };

    // Asset opt-ins.
    tokio::join!(
        c.storage_deposits(c.contract.as_account()),
        c.storage_deposits(&borrow_user),
        c.storage_deposits(&supply_user),
    );

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
    c.supply(&supply_user, 100).await;

    let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();

    assert_eq!(
        supply_position.get_borrow_asset_deposit().as_u128(),
        100,
        "Supply position should match amount of tokens supplied to contract",
    );

    let list_supplys = c.list_supplys().await;

    assert_eq!(
        list_supplys,
        [supply_user.id().clone()],
        "Supply user should be the only account listed",
    );

    // Step 2: Borrow user deposits collateral

    c.collateralize(&borrow_user, 200).await;

    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(
        borrow_position.collateral_asset_deposit.as_u128(),
        200,
        "Collateral asset deposit should be equal to the number of collateral tokens sent",
    );

    let list_borrows = c.list_borrows().await;

    assert_eq!(
        list_borrows,
        [borrow_user.id().clone()],
        "Borrow user should be the only account listed",
    );

    let equal_price = OraclePriceProof {
        collateral_asset_price: Rational::new(1, 1),
        borrow_asset_price: Rational::new(1, 1),
    };

    let borrow_status = c
        .get_borrow_status(borrow_user.id(), equal_price)
        .await
        .unwrap();

    assert_eq!(
        borrow_status,
        BorrowStatus::Healthy,
        "Borrow should be healthy when no assets are borrowed",
    );

    // Step 3: Withdraw some of the borrow asset

    // Borrowing 100 borrow tokens with 200 collateral tokens should be fine given equal price and MCR of 120%.
    c.borrow(&borrow_user, 100, equal_price).await;

    let balance = c.borrow_asset_balance_of(borrow_user.id()).await;

    assert_eq!(balance, 100, "Borrow user should receive assets");

    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(borrow_position.collateral_asset_deposit.as_u128(), 200);
    assert_eq!(
        borrow_position.get_total_borrow_asset_liability().as_u128(),
        100 + 10
    ); // origination fee

    // Step 4: Repay borrow

    // Need extra to pay for origination fee.
    c.borrow_asset_transfer(&supply_user, borrow_user.id(), 10)
        .await;

    c.repay(&borrow_user, 110).await;

    // Ensure borrow is paid off.
    let borrow_position = c.get_borrow_position(borrow_user.id()).await.unwrap();

    assert_eq!(borrow_position.collateral_asset_deposit.as_u128(), 200);
    assert_eq!(
        borrow_position.get_total_borrow_asset_liability().as_u128(),
        0
    );

    // Check yield for supply.
    c.harvest_yield(&supply_user).await;
    let supply_position = c.get_supply_position(supply_user.id()).await.unwrap();
    // TODO: Divide yield among supply, liquidator, protocol, etc.
    assert_eq!(supply_position.borrow_asset_yield.amount.as_u128(), 10);
}
