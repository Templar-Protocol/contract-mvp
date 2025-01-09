use near_sdk::{
    json_types::{U128, U64},
    near,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum BorrowStatus {
    Healthy,
    Liquidation,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct FeeRecord {
    pub total: U128,
    pub last_updated_block_height: U64,
}

impl FeeRecord {
    pub fn new(block_height: u64) -> Self {
        Self {
            total: U128(0),
            last_updated_block_height: U64(block_height),
        }
    }

    pub fn accumulate_fees(&mut self, additional_fees: u128, block_height: u64) {
        debug_assert!(block_height > self.last_updated_block_height.0);
        // TODO: Bounds checks
        self.total.0 += additional_fees;
        self.last_updated_block_height.0 = block_height;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct BorrowPosition {
    pub collateral_asset_deposit: U128,
    borrow_asset_principal: U128,
    pub borrow_asset_fees: FeeRecord,
}

impl BorrowPosition {
    pub fn new(block_height: u64) -> Self {
        Self {
            collateral_asset_deposit: U128(0),
            borrow_asset_principal: U128(0),
            borrow_asset_fees: FeeRecord::new(block_height),
        }
    }

    #[inline]
    pub fn get_borrow_asset_principal(&self) -> u128 {
        self.borrow_asset_principal.0
    }

    #[inline]
    pub fn total_borrow_asset_liability(&self) -> u128 {
        self.borrow_asset_principal.0 + self.borrow_asset_fees.total.0
    }

    pub fn exists(&self) -> bool {
        self.collateral_asset_deposit.0 != 0 || self.total_borrow_asset_liability() != 0
    }

    pub fn increase_collateral_asset_deposit(&mut self, amount: u128) -> Option<U128> {
        self.collateral_asset_deposit.0 = self.collateral_asset_deposit.0.checked_add(amount)?;
        Some(self.collateral_asset_deposit)
    }

    pub fn decrease_collateral_asset_deposit(&mut self, amount: u128) -> Option<U128> {
        self.collateral_asset_deposit.0 = self.collateral_asset_deposit.0.checked_sub(amount)?;
        Some(self.collateral_asset_deposit)
    }

    pub fn increase_borrow_asset_principal(&mut self, amount: u128) -> Option<U128> {
        self.borrow_asset_principal.0 = self.borrow_asset_principal.0.checked_add(amount)?;
        Some(self.borrow_asset_principal)
    }

    pub(crate) fn reduce_borrow_asset_liability(&mut self, mut amount: u128) -> LiabilityReduction {
        // No bounds checks necessary here: the min() call prevents underflow.

        let amount_to_fees = self.borrow_asset_fees.total.0.min(amount);
        amount -= amount_to_fees;
        self.borrow_asset_fees.total.0 -= amount_to_fees;

        let amount_to_principal = self.borrow_asset_principal.0.min(amount);
        amount -= amount_to_principal;
        self.borrow_asset_principal.0 -= amount_to_principal;

        LiabilityReduction {
            amount_to_fees,
            amount_to_principal,
            amount_remaining: amount,
        }
    }
}

pub struct LiabilityReduction {
    pub amount_to_fees: u128,
    pub amount_to_principal: u128,
    pub amount_remaining: u128,
}
