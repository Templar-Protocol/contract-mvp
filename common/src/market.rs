use near_sdk::json_types::{U128, U64};
use near_sdk::AccountId;

use crate::{
    asset::FungibleAsset,
    fee::{Fee, TimeBasedFee},
    rational::Rational,
};

// #[near_sdk::ext_contract(ext_market)]
pub trait MarketExternal {
    // ========================
    // MARKET GENERAL FUNCTIONS
    // ========================

    fn get_configuration(&self) -> MarketConfiguration;
    fn get_borrow_asset_metrics(&self) -> BorrowAssetMetrics;
    fn get_collateral_asset_provided(&self) -> U128;

    // TODO: Decide how to work with remote balances:

    // Option 1:
    // Balance oracle calls this function directly.
    fn report_remote_asset_balance(&mut self, address: String, asset: String, amount: U128);

    // Option 2: Balance oracle creates/maintains separate NEP-141-ish contracts that track remote
    // balances.

    fn list_borrowers(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId>;
    fn list_lenders(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId>;

    /// This function does need to retrieve a "proof-of-price" from somewhere, e.g. oracle.
    fn liquidate(&mut self, account_id: AccountId, meta: ()) -> ();

    // ==================
    // BORROWER FUNCTIONS
    // ==================

    // Required to implement NEP-141 FT token receiver to receive local fungible tokens.
    // ft_on_receive :: where msg = "repay"

    fn get_borrower_position(&self, account_id: AccountId) -> Borrow;
    /// This is just a read-only function, so we don't care about validating
    /// the provided price data.
    fn get_borrow_status(&self, account_id: AccountId, collateral_asset_price: ()) -> BorrowStatus;
    /// Works for both registered and unregistered accounts.
    fn get_deposit_address_for(&self, account_id: AccountId, collateral_asset: String) -> String;

    fn initialize_borrow(&mut self, borrow_asset_amount: U128, collateral_asset_amount: U128);
    fn borrow(&mut self, amount: U128);

    // ================
    // LENDER FUNCTIONS
    // ================
    // We assume that all borrowed assets are NEAR-local. That is to say, we
    // don't yet support lending of remote assets.

    // Required to implement NEP-141 FT token receiver to receive local fungible tokens.
    // ft_on_receive :: where msg = "provide"

    fn get_lender_position(&self, account_id: AccountId) -> Borrow;

    fn queue_withdrawal(&mut self, amount: U128);
    fn rescind_withrawal(&mut self);
    /// Auto-harvests yield.
    fn process_next_withdrawal(&mut self);

    fn harvest_yield(&mut self);

    // =================
    // REWARDS FUNCTIONS
    // =================
    fn withdraw_lender_rewards(&mut self, amount: U128);
    fn withdraw_liquidator_rewards(&mut self, amount: U128);
    fn withdraw_protocol_rewards(&mut self, amount: U128);
    // fn withdraw_insurance_rewards(&mut self, amount: U128);
}

pub enum BorrowStatus {
    Healthy,
    Liquidation,
}

/// available = floor((provided - used) * maximum_borrow_asset_usage_ratio)
pub struct BorrowAssetMetrics {
    pub used: U128,
    /// Available to be borrowed right now.
    pub available: U128,
    pub provided: U128,
}

pub struct MarketConfiguration {
    pub borrow_asset_id: FungibleAsset,
    pub collateral_asset_id: FungibleAsset,
    pub balance_oracle_account_id: AccountId,
    pub minimum_collateral_ratio_per_loan: Rational<u16>,
    /// How much of the deposited principal may be lent out (up to 100%)?
    /// This is a matter of protection for the lenders.
    /// Set to 99% for starters.
    pub maximum_borrow_asset_usage_ratio: Rational<u16>,
    /// This is paid by the borrower during repayment (or liquidation).
    pub origination_fee: Fee,
    pub apy: Rational<u16>,
    pub maximum_borrow_duration: U64,
    pub minimum_borrow_amount: U128,
    pub maximum_borrow_amount: U128,
    pub withdrawal_behavior: TimeBasedFee,
    pub liquidation_spread: LiquidationSpread,
}

pub struct LiquidationSpread {
    pub lender: U128,
    pub liquidator: U128,
    pub protocol: U128,
    // pub insurance: U128,
}

pub struct Borrow {
    pub amount_collateral: U128,
    pub amount_borrow: U128,
}
