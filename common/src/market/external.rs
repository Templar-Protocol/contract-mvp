use near_sdk::{
    json_types::{U128, U64},
    AccountId, PromiseOrValue,
};

use crate::{
    borrow::{BorrowPosition, BorrowStatus},
    rational::Rational,
    supply::SupplyPosition,
};

use super::{BorrowAssetMetrics, MarketConfiguration, OraclePriceProof};

// #[near_sdk::ext_contract(ext_market)]
pub trait MarketExternalInterface {
    // ========================
    // MARKET GENERAL FUNCTIONS
    // ========================

    fn get_configuration(&self) -> MarketConfiguration;
    fn get_borrow_asset_metrics(&self) -> BorrowAssetMetrics;
    fn get_collateral_asset_balance(&self) -> U128;

    // TODO: Decide how to work with remote balances:

    // Option 1:
    // Balance oracle calls this function directly.
    fn report_remote_asset_balance(&mut self, address: String, asset: String, amount: U128);

    // Option 2: Balance oracle creates/maintains separate NEP-141-ish contracts that track remote
    // balances.

    fn list_borrows(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId>;
    fn list_supplys(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId>;

    /// This function does need to retrieve a "proof-of-price" from somewhere, e.g. oracle.
    // fn liquidate(&mut self, account_id: AccountId, meta: ()) -> ();

    // ==================
    // BORROW FUNCTIONS
    // ==================

    // Required to implement NEP-141 FT token receiver to receive local fungible tokens.
    // ft_on_receive :: where msg = collateralize
    // ft_on_receive :: where msg = repay

    fn get_borrow_position(&self, account_id: AccountId) -> Option<BorrowPosition>;
    /// This is just a read-only function, so we don't care about validating
    /// the provided price data.
    fn get_borrow_status(
        &self,
        account_id: AccountId,
        oracle_price_proof: OraclePriceProof,
    ) -> Option<BorrowStatus>;
    /// Works for both registered and unregistered accounts.
    fn get_collateral_asset_deposit_address_for(
        &self,
        account_id: AccountId,
        collateral_asset: String,
    ) -> String;

    fn initialize_borrow(&mut self, borrow_asset_amount: U128, collateral_asset_amount: U128);
    fn borrow(&mut self, amount: U128, oracle_price_proof: OraclePriceProof) -> PromiseOrValue<()>;

    // ================
    // SUPPLY FUNCTIONS
    // ================
    // We assume that all borrowed assets are NEAR-local. That is to say, we
    // don't yet support supplying of remote assets.

    // Required to implement NEP-141 FT token receiver to receive local fungible tokens.
    // ft_on_receive :: where msg = supply

    fn get_supply_position(&self, account_id: AccountId) -> Option<SupplyPosition>;

    fn queue_withdrawal(&mut self, amount: U128);
    fn cancel_withrawal(&mut self);
    /// Auto-harvests yield.
    fn process_next_withdrawal(&mut self);

    fn harvest_yield(&mut self);

    // =================
    // REWARDS FUNCTIONS
    // =================
    fn withdraw_supply_position_rewards(&mut self, amount: U128);
    fn withdraw_liquidator_rewards(&mut self, amount: U128);
    fn withdraw_protocol_rewards(&mut self, amount: U128);
    // fn withdraw_insurance_rewards(&mut self, amount: U128);
}
