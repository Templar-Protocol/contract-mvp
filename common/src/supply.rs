use near_sdk::{
    json_types::{U128, U64},
    near,
};

#[near(serializers = [json, borsh])]
pub struct SupplyPosition {
    pub borrow_asset_deposited: U128,
    pub borrow_asset_rewards: RewardRecord,
    pub collateral_asset_rewards: RewardRecord,
}

impl SupplyPosition {
    pub fn new(block_height: u64) -> Self {
        Self {
            borrow_asset_deposited: 0.into(),
            borrow_asset_rewards: RewardRecord::new(block_height),
            collateral_asset_rewards: RewardRecord::new(block_height),
        }
    }

    pub fn exists(&self) -> bool {
        self.borrow_asset_deposited.0 != 0
            || self.borrow_asset_rewards.amount.0 != 0
            || self.collateral_asset_rewards.amount.0 != 0
    }

    pub fn deposit_borrow_asset(&mut self, amount: u128) -> Option<U128> {
        self.borrow_asset_deposited.0 = self.borrow_asset_deposited.0.checked_add(amount)?;
        Some(self.borrow_asset_deposited)
    }

    pub fn withdraw_borrow_asset(&mut self, amount: u128) -> Option<U128> {
        self.borrow_asset_deposited.0 = self.borrow_asset_deposited.0.checked_sub(amount)?;
        Some(self.borrow_asset_deposited)
    }
}

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

    pub fn accumulate_rewards(&mut self, new_rewards: u128, block_height: u64) {
        self.amount.0 += new_rewards;
        self.last_updated_block_height.0 = block_height;
    }
}
