use near_sdk::{json_types::U64, near};

use crate::asset::{
    AssetClass, BorrowAsset, BorrowAssetAmount, CollateralAssetAmount, FungibleAssetAmount,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub enum BorrowStatus {
    Healthy,
    Liquidation,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct FeeRecord<T: AssetClass> {
    pub(crate) total: FungibleAssetAmount<T>,
    pub(crate) last_updated_block_height: U64,
}

impl<T: AssetClass> FeeRecord<T> {
    pub fn new(block_height: u64) -> Self {
        Self {
            total: 0.into(),
            last_updated_block_height: U64(block_height),
        }
    }

    pub fn accumulate_fees(
        &mut self,
        additional_fees: FungibleAssetAmount<T>,
        block_height: u64,
    ) -> Option<()> {
        debug_assert!(block_height > self.last_updated_block_height.0);
        self.total.join(additional_fees)?;
        self.last_updated_block_height.0 = block_height;
        Some(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [borsh, json])]
pub struct BorrowPosition {
    pub collateral_asset_deposit: CollateralAssetAmount,
    borrow_asset_principal: BorrowAssetAmount,
    pub borrow_asset_fees: FeeRecord<BorrowAsset>,
    pub temporary_lock: BorrowAssetAmount,
}

impl BorrowPosition {
    pub fn new(block_height: u64) -> Self {
        Self {
            collateral_asset_deposit: 0.into(),
            borrow_asset_principal: 0.into(),
            borrow_asset_fees: FeeRecord::new(block_height),
            temporary_lock: 0.into(),
        }
    }

    pub fn get_borrow_asset_principal(&self) -> BorrowAssetAmount {
        self.borrow_asset_principal
    }

    pub fn get_total_borrow_asset_liability(&self) -> BorrowAssetAmount {
        let mut total = BorrowAssetAmount::zero();
        total.join(self.borrow_asset_principal);
        total.join(self.borrow_asset_fees.total);
        total.join(self.temporary_lock);
        total
    }

    pub fn exists(&self) -> bool {
        !self.collateral_asset_deposit.is_zero()
            || !self.get_total_borrow_asset_liability().is_zero()
    }

    pub fn increase_collateral_asset_deposit(
        &mut self,
        amount: CollateralAssetAmount,
    ) -> Option<()> {
        self.collateral_asset_deposit.join(amount)
    }

    pub fn decrease_collateral_asset_deposit(
        &mut self,
        amount: CollateralAssetAmount,
    ) -> Option<CollateralAssetAmount> {
        self.collateral_asset_deposit.split(amount)
    }

    pub fn increase_borrow_asset_principal(&mut self, amount: BorrowAssetAmount) -> Option<()> {
        self.borrow_asset_principal.join(amount)
    }

    pub(crate) fn reduce_borrow_asset_liability(
        &mut self,
        mut amount: BorrowAssetAmount,
    ) -> LiabilityReduction {
        // No bounds checks necessary here: the min() call prevents underflow.

        let amount_to_fees = self.borrow_asset_fees.total.min(amount);
        amount.split(amount_to_fees);
        self.borrow_asset_fees.total.split(amount_to_fees);

        let amount_to_principal = self.borrow_asset_principal.min(amount);
        amount.split(amount_to_principal);
        self.borrow_asset_principal.split(amount_to_principal);

        LiabilityReduction {
            amount_to_fees,
            amount_to_principal,
            amount_remaining: amount,
        }
    }
}

pub struct LiabilityReduction {
    pub amount_to_fees: BorrowAssetAmount,
    pub amount_to_principal: BorrowAssetAmount,
    pub amount_remaining: BorrowAssetAmount,
}
