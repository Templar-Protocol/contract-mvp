use near_sdk::{
    borsh::{self, BorshDeserialize, BorshSerialize},
    serde::{Deserialize, Serialize},
};

use crate::{
    asset::{CollateralAssetBalance, LoanAssetBalance, OracleCanonicalValuation},
    Fraction,
};

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Copy)]
#[serde(crate = "near_sdk::serde")]
pub struct Loan {
    pub collateral: CollateralAssetBalance, // NOTE: this only works with NEAR as collateral currency
    pub borrowed: LoanAssetBalance,
    pub minimum_collateral_ratio: Fraction,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct LoanStatus {
    pub borrowed_amount: LoanAssetBalance,
    pub borrowed_valuation: OracleCanonicalValuation,
    pub collateral_amount: CollateralAssetBalance,
    pub collateral_valuation: OracleCanonicalValuation,
    pub minimum_collateral_ratio: Fraction,
    pub is_undercollateralized: bool,
}

impl LoanStatus {
    pub fn total_max_borrowable_valuation(&self) -> OracleCanonicalValuation {
        self.minimum_collateral_ratio.denominator.0 * self.collateral_valuation
            / self.minimum_collateral_ratio.numerator.0
    }
}
