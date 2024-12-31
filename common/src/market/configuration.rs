use near_sdk::{
    json_types::{U128, U64},
    near, AccountId,
};

use crate::{
    asset::FungibleAsset,
    fee::{Fee, TimeBasedFee},
    rational::Rational,
};

use super::LiquidationSpread;

#[derive(Clone, Debug)]
#[near]
pub struct MarketConfiguration {
    pub borrow_asset: FungibleAsset,
    pub collateral_asset: FungibleAsset,
    pub balance_oracle_account_id: AccountId,
    pub minimum_collateral_ratio_per_borrow: Rational<u16>,
    /// How much of the deposited principal may be lent out (up to 100%)?
    /// This is a matter of protection for supply providers.
    /// Set to 99% for starters.
    pub maximum_borrow_asset_usage_ratio: Rational<u16>,
    /// The origination fee is a one-time amount added to the principal of the
    /// borrow. That is to say, the origination fee is denominated in units of
    /// the borrow asset and is paid by the borrowing account during repayment
    /// (or liquidation).
    pub origination_fee: Fee,
    pub annual_maintenance_fee: Fee,
    pub maximum_borrow_duration: Option<U64>,
    pub minimum_borrow_amount: U128,
    pub maximum_borrow_amount: U128,
    pub withdrawal_fee: TimeBasedFee,
    pub liquidation_spread: LiquidationSpread,
}
