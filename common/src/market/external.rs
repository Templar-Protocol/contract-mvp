use near_sdk::{json_types::U128, AccountId, Promise, PromiseOrValue};

use crate::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus},
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};

use super::{BorrowAssetMetrics, MarketConfiguration, OraclePriceProof};

#[near_sdk::ext_contract(ext_market)]
pub trait MarketExternalInterface {
    // ========================
    // MARKET GENERAL FUNCTIONS
    // ========================

    fn get_configuration(&self) -> MarketConfiguration;
    /// Takes current balance as an argument so that it can be called as view.
    /// `borrow_asset_balance` should be retrieved from the borrow asset
    /// contract specified in the market configuration.
    fn get_borrow_asset_metrics(
        &self,
        borrow_asset_balance: BorrowAssetAmount,
    ) -> BorrowAssetMetrics;

    // TODO: Decide how to work with remote balances:
    // Option 1:
    // Balance oracle calls a function directly.
    // Option 2: Balance oracle creates/maintains separate NEP-141-ish contracts that track remote
    // balances.

    fn list_borrows(&self, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId>;
    fn list_supplys(&self, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId>;

    // ==================
    // BORROW FUNCTIONS
    // ==================

    // ft_on_receive :: where msg = Collateralize
    fn collateralize_native(&mut self);
    // ft_on_receive :: where msg = Repay
    fn repay_native(&mut self) -> PromiseOrValue<()>;

    fn get_borrow_position(&self, account_id: AccountId) -> Option<BorrowPosition>;
    /// This is just a read-only function, so we don't care about validating
    /// the provided price data.
    fn get_borrow_status(
        &self,
        account_id: AccountId,
        oracle_price_proof: OraclePriceProof,
    ) -> Option<BorrowStatus>;

    fn borrow(
        &mut self,
        amount: BorrowAssetAmount,
        oracle_price_proof: OraclePriceProof,
    ) -> Promise;
    fn withdraw_collateral(
        &mut self,
        amount: U128,
        oracle_price_proof: Option<OraclePriceProof>,
    ) -> Promise;

    // ================
    // SUPPLY FUNCTIONS
    // ================
    // We assume that all borrowed assets are NEAR-local. That is to say, we
    // don't yet support supplying of remote assets.

    // ft_on_receive :: where msg = Supply
    fn supply_native(&mut self);

    fn get_supply_position(&self, account_id: AccountId) -> Option<SupplyPosition>;

    fn create_supply_withdrawal_request(&mut self, amount: U128);
    fn cancel_supply_withdrawal_request(&mut self);
    /// Auto-harvests yield.
    fn execute_next_supply_withdrawal_request(&mut self) -> PromiseOrValue<()>;
    fn get_supply_withdrawal_request_status(
        &self,
        account_id: AccountId,
    ) -> Option<WithdrawalRequestStatus>;
    fn get_supply_withdrawal_queue_status(&self) -> WithdrawalQueueStatus;

    fn harvest_yield(&mut self);

    // =====================
    // LIQUIDATION FUNCTIONS
    // =====================

    // ft_on_receive :: where msg = Liquidate { account_id }
    fn liquidate_native(
        &mut self,
        account_id: AccountId,
        oracle_price_proof: OraclePriceProof,
    ) -> Promise;

    // =================
    // YIELD FUNCTIONS
    // =================
    fn get_static_yield(&self, account_id: AccountId) -> Option<StaticYieldRecord>;
    fn withdraw_supply_yield(&mut self, amount: Option<U128>) -> Promise;
    fn withdraw_static_yield(
        &mut self,
        borrow_asset_amount: Option<BorrowAssetAmount>,
        collateral_asset_amount: Option<CollateralAssetAmount>,
    ) -> Promise;
}
