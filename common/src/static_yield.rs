use near_sdk::near;

use crate::asset::{BorrowAssetAmount, CollateralAssetAmount};

#[derive(Default, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct StaticYieldRecord {
    pub collateral_asset: CollateralAssetAmount,
    pub borrow_asset: BorrowAssetAmount,
}
