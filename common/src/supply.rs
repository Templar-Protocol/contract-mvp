use near_sdk::{
    json_types::{U128, U64},
    near,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct SupplyPosition {
    borrow_asset_deposit: U128,
    pub borrow_asset_rewards: RewardRecord,
}

impl SupplyPosition {
    pub fn new(block_height: u64) -> Self {
        Self {
            borrow_asset_deposit: 0.into(),
            borrow_asset_rewards: RewardRecord::new(block_height),
        }
    }

    pub fn get_borrow_asset_deposit(&self) -> u128 {
        self.borrow_asset_deposit.0
    }

    pub fn exists(&self) -> bool {
        self.borrow_asset_deposit.0 != 0 || self.borrow_asset_rewards.amount.0 != 0
    }

    /// MUST always be paired with a rewards recalculation!
    pub(crate) fn increase_borrow_asset_deposit(&mut self, amount: u128) -> Option<U128> {
        self.borrow_asset_deposit.0 = self.borrow_asset_deposit.0.checked_add(amount)?;
        Some(self.borrow_asset_deposit)
    }

    /// MUST always be paired with a rewards recalculation!
    pub(crate) fn decrease_borrow_asset_deposit(&mut self, amount: u128) -> Option<U128> {
        self.borrow_asset_deposit.0 = self.borrow_asset_deposit.0.checked_sub(amount)?;
        Some(self.borrow_asset_deposit)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub struct RewardRecord {
    pub amount: U128,
    pub last_updated_block_height: U64,
}

impl RewardRecord {
    pub fn new(block_height: u64) -> Self {
        Self {
            amount: 0.into(),
            last_updated_block_height: block_height.into(),
        }
    }

    /// Returns the amount of rewards remaining
    pub fn withdraw(&mut self, amount: u128) -> Option<U128> {
        self.amount.0 = self.amount.0.checked_sub(amount)?;
        Some(self.amount)
    }

    pub fn accumulate_rewards(&mut self, additional_rewards: u128, block_height: u64) {
        debug_assert!(block_height > self.last_updated_block_height.0);
        self.amount.0 += additional_rewards;
        self.last_updated_block_height.0 = block_height;
    }
}
