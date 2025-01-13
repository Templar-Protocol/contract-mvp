use near_sdk::{json_types::U64, near};

use crate::asset::{AssetClass, BorrowAsset, BorrowAssetAmount, FungibleAssetAmount};

#[derive(Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct SupplyPosition {
    borrow_asset_deposit: BorrowAssetAmount,
    pub borrow_asset_yield: YieldRecord<BorrowAsset>,
}

impl SupplyPosition {
    pub fn new(block_height: u64) -> Self {
        Self {
            borrow_asset_deposit: 0.into(),
            borrow_asset_yield: YieldRecord::new(block_height),
        }
    }

    pub fn get_borrow_asset_deposit(&self) -> BorrowAssetAmount {
        self.borrow_asset_deposit
    }

    pub fn exists(&self) -> bool {
        !self.borrow_asset_deposit.is_zero() || !self.borrow_asset_yield.amount.is_zero()
    }

    /// MUST always be paired with a yield recalculation!
    pub(crate) fn increase_borrow_asset_deposit(
        &mut self,
        amount: BorrowAssetAmount,
    ) -> Option<()> {
        self.borrow_asset_deposit.join(amount)
    }

    /// MUST always be paired with a yield recalculation!
    pub(crate) fn decrease_borrow_asset_deposit(
        &mut self,
        amount: BorrowAssetAmount,
    ) -> Option<BorrowAssetAmount> {
        self.borrow_asset_deposit.split(amount)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct YieldRecord<T: AssetClass> {
    pub amount: FungibleAssetAmount<T>,
    pub last_updated_block_height: U64,
}

impl<T: AssetClass> YieldRecord<T> {
    pub fn new(block_height: u64) -> Self {
        Self {
            amount: 0.into(),
            last_updated_block_height: block_height.into(),
        }
    }

    /// Returns the amount of yield remaining
    pub fn withdraw(&mut self, amount: FungibleAssetAmount<T>) -> Option<FungibleAssetAmount<T>> {
        self.amount.split(amount)
    }

    pub fn accumulate_yield(
        &mut self,
        additional_yield: FungibleAssetAmount<T>,
        block_height: u64,
    ) {
        debug_assert!(block_height > self.last_updated_block_height.0);
        self.amount.join(additional_yield);
        self.last_updated_block_height.0 = block_height;
    }
}
