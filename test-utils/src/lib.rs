use near_sdk::{
    json_types::{U128, U64},
    serde_json::{self, json},
    AccountId, AccountIdRef, NearToken,
};
use near_workspaces::{
    network::Sandbox, prelude::*, result::ExecutionSuccess, Account, Contract, DevNetwork, Worker,
};
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount, FungibleAsset},
    borrow::{BorrowPosition, BorrowStatus},
    fee::{Fee, TimeBasedFee},
    market::{
        LiquidateMsg, MarketConfiguration, Nep141MarketDepositMessage, OraclePriceProof,
        YieldWeights,
    },
    rational::{Fraction, Rational},
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};
use tokio::sync::OnceCell;

pub const EQUAL_PRICE: OraclePriceProof = OraclePriceProof {
    collateral_asset_price: Rational::<u128>::one(),
    borrow_asset_price: Rational::<u128>::one(),
};

pub const COLLATERAL_HALF_PRICE: OraclePriceProof = OraclePriceProof {
    collateral_asset_price: Rational::<u128>::new_const(1, 2),
    borrow_asset_price: Rational::<u128>::one(),
};

pub enum TestAsset {
    Native,
    Nep141(Contract),
}

impl TestAsset {
    pub fn is_native(&self) -> bool {
        matches!(self, Self::Native)
    }

    pub fn nep141_id(&self) -> Option<&AccountId> {
        if let Self::Nep141(ref contract) = self {
            Some(contract.id())
        } else {
            None
        }
    }
}

pub struct TestController {
    pub worker: Worker<Sandbox>,
    pub contract: Contract,
    pub borrow_asset: TestAsset,
    pub collateral_asset: TestAsset,
}

impl TestController {
    pub async fn storage_deposits(&self, account: &Account) {
        println!("Performing storage deposits for {}...", account.id());
        if let TestAsset::Nep141(ref borrow_asset) = self.borrow_asset {
            account
                .call(borrow_asset.id(), "storage_deposit")
                .args_json(json!({}))
                .deposit(NearToken::from_near(1))
                .transact()
                .await
                .unwrap()
                .unwrap();
        }
        if let TestAsset::Nep141(ref collateral_asset) = self.collateral_asset {
            account
                .call(collateral_asset.id(), "storage_deposit")
                .args_json(json!({}))
                .deposit(NearToken::from_near(1))
                .transact()
                .await
                .unwrap()
                .unwrap();
        }
    }

    pub async fn get_configuration(&self) -> MarketConfiguration {
        self.contract
            .view("get_configuration")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<MarketConfiguration>()
            .unwrap()
    }

    pub async fn supply_native(&self, supply_user: &Account, amount: u128) {
        supply_user
            .call(self.contract.id(), "supply_native")
            .args_json(json!({}))
            .deposit(NearToken::from_yoctonear(amount))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn supply(&self, supply_user: &Account, amount: u128) {
        println!(
            "{} transferring {amount} tokens for supply...",
            supply_user.id()
        );
        match self.borrow_asset {
            TestAsset::Native => self.supply_native(supply_user, amount).await,
            TestAsset::Nep141(_) => {
                self.borrow_asset_transfer_call(
                    supply_user,
                    self.contract.id(),
                    amount,
                    &serde_json::to_string(&Nep141MarketDepositMessage::Supply).unwrap(),
                )
                .await;
            }
        }
    }

    pub async fn get_supply_position(&self, account_id: &AccountId) -> Option<SupplyPosition> {
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

    pub async fn list_supplys(&self) -> Vec<AccountId> {
        self.contract
            .view("list_supplys")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<Vec<AccountId>>()
            .unwrap()
    }

    pub async fn collateralize_native(&self, borrow_user: &Account, amount: u128) {
        borrow_user
            .call(self.contract.id(), "collateralize_native")
            .args_json(json!({}))
            .deposit(NearToken::from_yoctonear(amount))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn collateralize(&self, borrow_user: &Account, amount: u128) {
        println!(
            "{} transferring {amount} tokens for collateral...",
            borrow_user.id(),
        );
        match self.collateral_asset {
            TestAsset::Native => self.collateralize_native(borrow_user, amount).await,
            TestAsset::Nep141(_) => {
                self.collateral_asset_transfer_call(
                    borrow_user,
                    self.contract.id(),
                    amount,
                    &serde_json::to_string(&Nep141MarketDepositMessage::Collateralize).unwrap(),
                )
                .await;
            }
        }
    }

    pub async fn get_borrow_position(&self, account_id: &AccountId) -> Option<BorrowPosition> {
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

    pub async fn list_borrows(&self) -> Vec<AccountId> {
        self.contract
            .view("list_borrows")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<Vec<AccountId>>()
            .unwrap()
    }

    pub async fn get_borrow_status(
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

    pub async fn borrow(&self, borrow_user: &Account, amount: u128, price: OraclePriceProof) {
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

    pub async fn collateral_asset_balance_of(&self, account_id: &AccountId) -> u128 {
        match self.collateral_asset {
            TestAsset::Native => self
                .worker
                .view_account(self.contract.id())
                .await
                .map(|v| v.balance.as_yoctonear())
                .unwrap(),
            TestAsset::Nep141(ref collateral_asset) => {
                collateral_asset
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
        }
    }

    pub async fn borrow_asset_balance_of(&self, account_id: &AccountId) -> u128 {
        match self.borrow_asset {
            TestAsset::Native => self
                .worker
                .view_account(self.contract.id())
                .await
                .map(|v| v.balance.as_yoctonear() - v.locked.as_yoctonear())
                .unwrap(),
            TestAsset::Nep141(ref borrow_asset) => {
                borrow_asset
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
        }
    }

    pub async fn asset_transfer(
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

    pub async fn asset_transfer_call(
        &self,
        asset_id: &AccountId,
        sender: &Account,
        receiver_id: &AccountId,
        amount: u128,
        msg: &str,
    ) -> ExecutionSuccess {
        println!(
            "{} sending {amount} tokens of {asset_id} to {receiver_id} with msg {msg}...",
            sender.id(),
        );
        sender
            .call(asset_id, "ft_transfer_call")
            .args_json(json!({
                "receiver_id": receiver_id,
                "amount": U128(amount),
                "msg": msg,
            }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn borrow_asset_transfer(
        &self,
        sender: &Account,
        receiver_id: &AccountId,
        amount: u128,
    ) {
        match self.borrow_asset {
            TestAsset::Native => {
                sender
                    .transfer_near(receiver_id, NearToken::from_yoctonear(amount))
                    .await
                    .unwrap()
                    .unwrap();
            }
            TestAsset::Nep141(ref contract) => {
                self.asset_transfer(contract.id(), sender, receiver_id, amount)
                    .await;
            }
        }
    }

    pub async fn borrow_asset_transfer_call(
        &self,
        sender: &Account,
        receiver_id: &AccountId,
        amount: u128,
        msg: &str,
    ) -> ExecutionSuccess {
        if let TestAsset::Nep141(ref borrow_asset) = self.borrow_asset {
            self.asset_transfer_call(borrow_asset.id(), sender, receiver_id, amount, msg)
                .await
        } else {
            panic!("Cannot perform an ft_transfer_call with a native asset");
        }
    }

    pub async fn collateral_asset_transfer_call(
        &self,
        sender: &Account,
        receiver_id: &AccountId,
        amount: u128,
        msg: &str,
    ) -> ExecutionSuccess {
        if let TestAsset::Nep141(ref collateral_asset) = self.collateral_asset {
            self.asset_transfer_call(collateral_asset.id(), sender, receiver_id, amount, msg)
                .await
        } else {
            panic!("Cannot perform an ft_transfer_call with a native asset");
        }
    }

    pub async fn repay_native(&self, borrow_user: &Account, amount: u128) {
        borrow_user
            .call(self.contract.id(), "repay_native")
            .args_json(json!({}))
            .deposit(NearToken::from_yoctonear(amount))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn repay(&self, borrow_user: &Account, amount: u128) {
        println!("{} repaying {amount} tokens...", borrow_user.id());
        match self.borrow_asset {
            TestAsset::Native => self.repay_native(borrow_user, amount).await,
            TestAsset::Nep141(_) => {
                self.borrow_asset_transfer_call(
                    borrow_user,
                    self.contract.id(),
                    amount,
                    &serde_json::to_string(&Nep141MarketDepositMessage::Repay).unwrap(),
                )
                .await;
            }
        }
    }

    pub async fn harvest_yield(&self, supply_user: &Account) {
        println!("{} harvesting yield...", supply_user.id());
        supply_user
            .call(self.contract.id(), "harvest_yield")
            .args_json(json!({}))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn withdraw_static_yield(
        &self,
        account: &Account,
        borrow_asset_amount: Option<BorrowAssetAmount>,
        collateral_asset_amount: Option<CollateralAssetAmount>,
    ) {
        println!("{} withdrawing static yield...", account.id());
        account
            .call(self.contract.id(), "withdraw_static_yield")
            .args_json(json!({
                "borrow_asset_amount": borrow_asset_amount,
                "collateral_asset_amount": collateral_asset_amount,
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn withdraw_supply_yield(
        &self,
        supply_user: &Account,
        amount: Option<u128>,
    ) -> ExecutionSuccess {
        println!("{} withdrawing supply yield...", supply_user.id());
        supply_user
            .call(self.contract.id(), "withdraw_supply_yield")
            .args_json(json!({
                "amount": amount.map(U128),
            }))
            .transact()
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn get_static_yield(&self, account_id: &AccountId) -> Option<StaticYieldRecord> {
        self.contract
            .view("get_static_yield")
            .args_json(json!({
                "account_id": account_id,
            }))
            .await
            .unwrap()
            .json::<Option<StaticYieldRecord>>()
            .unwrap()
    }

    pub async fn withdraw_collateral(
        &self,
        borrow_user: &Account,
        amount: u128,
        price: Option<OraclePriceProof>,
    ) -> ExecutionSuccess {
        println!("{} withdrawing {amount} collateral...", borrow_user.id());
        borrow_user
            .call(self.contract.id(), "withdraw_collateral")
            .args_json(json!({
                "amount": U128(amount),
                "oracle_price_proof": price,
            }))
            .transact()
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn create_supply_withdrawal_request(&self, supply_user: &Account, amount: u128) {
        println!(
            "{} creating supply withdrawal request for {amount}...",
            supply_user.id()
        );
        supply_user
            .call(self.contract.id(), "create_supply_withdrawal_request")
            .args_json(json!({
                "amount": U128(amount),
            }))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn get_supply_withdrawal_request_status(
        &self,
        account_id: &AccountId,
    ) -> Option<WithdrawalRequestStatus> {
        self.contract
            .view("get_supply_withdrawal_request_status")
            .args_json(json!({
                "account_id": account_id,
            }))
            .await
            .unwrap()
            .json::<Option<WithdrawalRequestStatus>>()
            .unwrap()
    }

    pub async fn get_supply_withdrawal_queue_status(&self) -> WithdrawalQueueStatus {
        self.contract
            .view("get_supply_withdrawal_queue_status")
            .args_json(json!({}))
            .await
            .unwrap()
            .json::<WithdrawalQueueStatus>()
            .unwrap()
    }

    pub async fn execute_next_supply_withdrawal_request(&self, account: &Account) {
        println!(
            "{} executing next supply withdrawal request...",
            account.id(),
        );
        account
            .call(self.contract.id(), "execute_next_supply_withdrawal_request")
            .args_json(json!({}))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn liquidate_native(
        &self,
        liquidator_user: &Account,
        account_id: &AccountId,
        borrow_asset_amount: u128,
        oracle_price_proof: OraclePriceProof,
    ) {
        liquidator_user
            .call(self.contract.id(), "liquidate_native")
            .args_json(json!({
                "account_id": account_id,
                "oracle_price_proof": oracle_price_proof,
            }))
            .deposit(NearToken::from_yoctonear(borrow_asset_amount))
            .transact()
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn liquidate(
        &self,
        liquidator_user: &Account,
        account_id: &AccountId,
        borrow_asset_amount: u128,
        oracle_price_proof: OraclePriceProof,
    ) {
        println!(
            "{} executing liquidation against {} for {}...",
            liquidator_user.id(),
            account_id,
            borrow_asset_amount,
        );
        match self.borrow_asset {
            TestAsset::Native => {
                self.liquidate_native(
                    liquidator_user,
                    account_id,
                    borrow_asset_amount,
                    oracle_price_proof,
                )
                .await
            }
            TestAsset::Nep141(_) => {
                self.borrow_asset_transfer_call(
                    liquidator_user,
                    self.contract.id(),
                    borrow_asset_amount,
                    &serde_json::to_string(&Nep141MarketDepositMessage::Liquidate(LiquidateMsg {
                        account_id: account_id.clone(),
                        oracle_price_proof,
                    }))
                    .unwrap(),
                )
                .await;
            }
        }
    }

    #[allow(unused)] // This is useful for debugging tests
    pub async fn print_logs(&self) {
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

pub async fn create_prefixed_account<T: DevNetwork + TopLevelAccountCreator + 'static>(
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

pub fn market_configuration(
    borrow_asset_id: AccountId,
    collateral_asset_id: AccountId,
    yield_weights: YieldWeights,
) -> MarketConfiguration {
    MarketConfiguration {
        borrow_asset: FungibleAsset::nep141(borrow_asset_id),
        collateral_asset: FungibleAsset::nep141(collateral_asset_id),
        balance_oracle_account_id: "balance_oracle".parse().unwrap(),
        minimum_collateral_ratio_per_borrow: Rational::new(120, 100),
        maximum_borrow_asset_usage_ratio: Fraction::new(99, 100).unwrap(),
        borrow_origination_fee: Fee::Proportional(Rational::new(10, 100)),
        borrow_annual_maintenance_fee: Fee::zero(),
        maximum_borrow_duration_ms: None,
        minimum_borrow_amount: 1.into(),
        maximum_borrow_amount: u128::MAX.into(),
        maximum_liquidator_spread: Fraction::new(5, 100).unwrap(),
        supply_withdrawal_fee: TimeBasedFee::zero(),
        yield_weights,
    }
}

pub static WASM_MARKET: OnceCell<Vec<u8>> = OnceCell::const_new();
pub static WASM_MOCK_FT: OnceCell<Vec<u8>> = OnceCell::const_new();

pub async fn setup_market(
    worker: &Worker<Sandbox>,
    configuration: &MarketConfiguration,
) -> Contract {
    let wasm = WASM_MARKET
        .get_or_init(|| async { near_workspaces::compile_project("./").await.unwrap() })
        .await;

    let contract = worker.dev_deploy(wasm).await.unwrap();
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

pub async fn deploy_ft(
    account: Account,
    name: &str,
    symbol: &str,
    owner_id: &AccountIdRef,
    supply: u128,
) -> Contract {
    let wasm = WASM_MOCK_FT
        .get_or_init(|| async {
            near_workspaces::compile_project("./mock/ft/")
                .await
                .unwrap()
        })
        .await;

    let contract = account.deploy(wasm).await.unwrap().unwrap();
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

pub struct SetupEverything {
    pub c: TestController,
    pub liquidator_user: Account,
    pub supply_user: Account,
    pub borrow_user: Account,
    pub protocol_yield_user: Account,
    pub insurance_yield_user: Account,
}

pub async fn setup_everything(
    customize_market_configuration: impl FnOnce(&mut MarketConfiguration),
) -> SetupEverything {
    let worker = near_workspaces::sandbox().await.unwrap();
    accounts!(
        worker,
        liquidator_user,
        supply_user,
        borrow_user,
        protocol_yield_user,
        insurance_yield_user,
        collateral_asset,
        borrow_asset
    );
    let mut config = market_configuration(
        borrow_asset.id().clone(),
        collateral_asset.id().clone(),
        YieldWeights::new_with_supply_weight(8)
            .with_static(protocol_yield_user.id().clone(), 1)
            .with_static(insurance_yield_user.id().clone(), 1),
    );
    customize_market_configuration(&mut config);

    let (contract, borrow_asset, collateral_asset) = tokio::join!(
        setup_market(&worker, &config),
        deploy_ft(
            borrow_asset,
            "Borrow Asset",
            "BORROW",
            supply_user.id(),
            200000,
        ),
        deploy_ft(
            collateral_asset,
            "Collateral Asset",
            "COLLATERAL",
            borrow_user.id(),
            100000,
        ),
    );

    let collateral_asset = config
        .collateral_asset
        .into_nep141()
        .map_or(TestAsset::Native, |_| TestAsset::Nep141(collateral_asset));
    let borrow_asset = config
        .borrow_asset
        .into_nep141()
        .map_or(TestAsset::Native, |_| TestAsset::Nep141(borrow_asset));

    let c = TestController {
        worker,
        contract,
        collateral_asset,
        borrow_asset,
    };

    // Asset opt-ins.
    tokio::join!(
        c.storage_deposits(c.contract.as_account()),
        async {
            c.storage_deposits(&liquidator_user).await;
            c.borrow_asset_transfer(&supply_user, liquidator_user.id(), 100000)
                .await;
        },
        c.storage_deposits(&borrow_user),
        c.storage_deposits(&supply_user),
        c.storage_deposits(&protocol_yield_user),
        c.storage_deposits(&insurance_yield_user),
    );

    SetupEverything {
        c,
        liquidator_user,
        supply_user,
        borrow_user,
        protocol_yield_user,
        insurance_yield_user,
    }
}
