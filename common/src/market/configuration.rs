use near_sdk::{
    json_types::{U128, U64},
    near, AccountId,
};

use crate::{
    asset::FungibleAsset,
    borrow::BorrowPosition,
    fee::{Fee, TimeBasedFee},
    rational::Rational,
};

use super::{LiquidationSpread, OraclePriceProof};

#[derive(Clone, Debug)]
#[near]
pub struct MarketConfiguration {
    pub borrow_asset: FungibleAsset,
    pub collateral_asset: FungibleAsset,
    pub balance_oracle_account_id: AccountId,
    pub liquidator_account_id: AccountId,
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

impl MarketConfiguration {
    pub fn is_healthy(
        &self,
        borrow_position: &BorrowPosition,
        OraclePriceProof {
            collateral_asset_price,
            borrow_asset_price,
        }: OraclePriceProof,
    ) -> bool {
        let scaled_collateral_value = borrow_position.collateral_asset_deposit.0
            * collateral_asset_price.numerator()
            * borrow_asset_price.denominator()
            * self.minimum_collateral_ratio_per_borrow.denominator() as u128;
        let scaled_borrow_value = borrow_position.borrow_asset_liability.0
            * borrow_asset_price.numerator()
            * collateral_asset_price.denominator()
            * self.minimum_collateral_ratio_per_borrow.numerator() as u128;

        scaled_collateral_value >= scaled_borrow_value
    }
}
