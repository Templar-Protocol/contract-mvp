use near_sdk::{json_types::U128, near};

use crate::rational::Rational;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near]
pub enum BorrowStatus {
    Healthy,
    Liquidation,
}

#[derive(Default)]
#[near]
pub struct BorrowPosition {
    pub collateral_asset_deposited: U128,
    pub borrow_asset_liability: U128,
}

impl BorrowPosition {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn exists(&self) -> bool {
        self.collateral_asset_deposited.0 != 0 || self.borrow_asset_liability.0 != 0
    }

    pub fn is_healthy(
        &self,
        collateral_asset_price: Rational<u128>,
        borrow_asset_price: Rational<u128>,
        minimum_collateral_ratio: Rational<u128>,
    ) -> bool {
        let scaled_collateral_value = self.collateral_asset_deposited.0
            * collateral_asset_price.numerator()
            * borrow_asset_price.denominator()
            * minimum_collateral_ratio.denominator();
        let scaled_loan_value = self.borrow_asset_liability.0
            * borrow_asset_price.numerator()
            * collateral_asset_price.denominator()
            * minimum_collateral_ratio.numerator();

        scaled_collateral_value >= scaled_loan_value
    }

    pub fn deposit_collateral_asset(&mut self, amount: u128) -> Option<U128> {
        self.collateral_asset_deposited.0 =
            self.collateral_asset_deposited.0.checked_add(amount)?;
        Some(self.collateral_asset_deposited)
    }

    pub fn withdraw_collateral_asset(&mut self, amount: u128) -> Option<U128> {
        self.collateral_asset_deposited.0 =
            self.collateral_asset_deposited.0.checked_sub(amount)?;
        Some(self.collateral_asset_deposited)
    }

    pub fn increase_borrow_asset_liability(&mut self, amount: u128) -> Option<U128> {
        self.borrow_asset_liability.0 = self.borrow_asset_liability.0.checked_add(amount)?;
        Some(self.borrow_asset_liability)
    }

    pub fn decrease_borrow_asset_liability(&mut self, amount: u128) -> Option<U128> {
        self.borrow_asset_liability.0 = self.borrow_asset_liability.0.checked_sub(amount)?;
        Some(self.borrow_asset_liability)
    }
}
