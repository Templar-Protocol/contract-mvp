use near_sdk::{
    collections::{TreeMap, UnorderedMap},
    env, near, require, AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::{
    borrow::BorrowPosition, market::MarketConfiguration, rational::Rational, supply::SupplyPosition,
};

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    SupplyPositions,
    BorrowPositions,
    TotalBorrowAssetDepositedLog,
    BorrowAssetRewardDistributionLog,
}

#[near]
pub struct Market {
    prefix: Vec<u8>,
    pub configuration: MarketConfiguration,
    /// There are two different balance records for the borrow asset. The
    /// current balance is `borrow_asset_balance = borrow_asset_deposited -
    /// <amount loaned out>`.
    pub borrow_asset_deposited: u128,
    /// The current amount of borrow asset under direct control of the market.
    pub borrow_asset_balance: u128,
    /// The current amount of collateral asset under direct control of the
    /// market.
    pub collateral_asset_balance: u128,
    pub supply_positions: UnorderedMap<AccountId, SupplyPosition>,
    pub borrow_positions: UnorderedMap<AccountId, BorrowPosition>,
    pub total_borrow_asset_deposited_log: TreeMap<u64, u128>,
    pub borrow_asset_reward_distribution_log: TreeMap<u64, u128>,
}

impl Market {
    pub fn new(prefix: impl IntoStorageKey, configuration: MarketConfiguration) -> Self {
        let prefix = prefix.into_storage_key();
        macro_rules! key {
            ($key: ident) => {
                [
                    prefix.as_slice(),
                    StorageKey::$key.into_storage_key().as_slice(),
                ]
                .concat()
            };
        }
        Self {
            prefix: prefix.clone(),
            configuration,
            borrow_asset_deposited: 0,
            borrow_asset_balance: 0,
            collateral_asset_balance: 0,
            supply_positions: UnorderedMap::new(key!(SupplyPositions)),
            borrow_positions: UnorderedMap::new(key!(BorrowPositions)),
            total_borrow_asset_deposited_log: TreeMap::new(key!(TotalBorrowAssetDepositedLog)),
            borrow_asset_reward_distribution_log: TreeMap::new(key!(
                BorrowAssetRewardDistributionLog
            )),
        }
    }

    pub fn get_borrow_position(&self, account_id: &AccountId) -> Option<BorrowPosition> {
        self.borrow_positions.get(account_id)
    }

    pub fn get_supply_position(&self, account_id: &AccountId) -> Option<SupplyPosition> {
        self.supply_positions.get(account_id)
    }

    fn log_borrow_asset_deposited(&mut self, amount: u128) {
        let block_height = env::block_height();
        self.total_borrow_asset_deposited_log
            .insert(&block_height, &amount);
    }

    fn record_borrow_asset_reward_distribution(&mut self, amount: u128) {
        let block_height = env::block_height();
        let mut distributed_in_block = self
            .borrow_asset_reward_distribution_log
            .get(&block_height)
            .unwrap_or(0);
        distributed_in_block += amount;
        self.borrow_asset_reward_distribution_log
            .insert(&block_height, &distributed_in_block);
    }

    pub fn record_supply_position_borrow_asset_deposit(
        &mut self,
        account_id: &AccountId,
        amount: u128,
    ) {
        let mut supply_position = self
            .supply_positions
            .get(account_id)
            .unwrap_or_else(|| SupplyPosition::new(env::block_height()));

        supply_position
            .deposit_borrow_asset(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset overflow"));

        self.supply_positions.insert(account_id, &supply_position);

        self.borrow_asset_deposited = self
            .borrow_asset_deposited
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited overflow"));

        self.log_borrow_asset_deposited(self.borrow_asset_deposited);
    }

    pub fn record_supply_position_borrow_asset_withdrawal(
        &mut self,
        account_id: &AccountId,
        amount: u128,
    ) {
        let mut supply_position = self
            .supply_positions
            .get(account_id)
            .unwrap_or_else(|| SupplyPosition::new(env::block_height()));

        supply_position
            .withdraw_borrow_asset(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset underflow"));

        self.supply_positions.insert(account_id, &supply_position);

        self.borrow_asset_deposited = self
            .borrow_asset_deposited
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited underflow"));

        self.log_borrow_asset_deposited(self.borrow_asset_deposited);
    }

    pub fn record_borrow_position_collateral_asset_deposit(
        &mut self,
        account_id: &AccountId,
        amount: u128,
    ) {
        let mut borrow_position = self.borrow_positions.get(account_id).unwrap_or_default();

        borrow_position
            .increase_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset overflow"));

        self.borrow_positions.insert(account_id, &borrow_position);

        self.collateral_asset_balance = self
            .collateral_asset_balance
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Collateral asset balance overflow"));
    }

    pub fn record_borrow_position_collateral_asset_withdrawal(
        &mut self,
        account_id: &AccountId,
        amount: u128,
    ) {
        let mut borrow_position = self.borrow_positions.get(account_id).unwrap_or_default();

        borrow_position
            .decrease_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset underflow"));

        self.borrow_positions.insert(account_id, &borrow_position);

        self.collateral_asset_balance = self
            .collateral_asset_balance
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Collateral asset balance underflow"));
    }

    pub fn record_borrow_position_borrow_asset_withdrawal(
        &mut self,
        account_id: &AccountId,
        liable_amount: u128,
        dispersed_amount: u128,
    ) -> BorrowPosition {
        let mut borrow_position = self.borrow_positions.get(account_id).unwrap_or_default();

        borrow_position
            .increase_borrow_asset_liability(liable_amount)
            .unwrap_or_else(|| env::panic_str("Borrow position borrow asset liability overflow"));

        self.borrow_positions.insert(account_id, &borrow_position);

        self.borrow_asset_balance = self
            .borrow_asset_balance
            .checked_sub(dispersed_amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset balance underflow"));

        borrow_position
    }

    pub fn record_borrow_position_borrow_asset_repay(
        &mut self,
        account_id: &AccountId,
        amount: u128,
    ) {
        let mut borrow_position = self.borrow_positions.get(account_id).unwrap_or_default();

        borrow_position
            .decrease_borrow_asset_liability(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position borrow asset liability underflow"));

        self.borrow_positions.insert(account_id, &borrow_position);

        self.borrow_asset_balance = self
            .borrow_asset_balance
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Total loan asset borrowed underflow"));
    }

    pub fn record_supply_position_collateral_rewards_withdrawal(
        &mut self,
        account_id: &AccountId,
        amount: u128,
    ) {
        let mut supply_position = self
            .supply_positions
            .get(account_id)
            .unwrap_or_else(|| SupplyPosition::new(env::block_height()));

        supply_position
            .collateral_asset_rewards
            .withdraw(amount)
            .unwrap_or_else(|| {
                env::panic_str("Supply position collateral asset withdrawal underflow")
            });

        self.supply_positions.insert(account_id, &supply_position);
    }

    pub fn calculate_supply_position_rewards(
        &self,
        reward_distribution_log: &TreeMap<u64, u128>,
        last_updated_block_height: u64,
        deposit_during_interval: u128,
        until_block_height: u64,
    ) -> (u128, u64) {
        let start_from_block_height = reward_distribution_log
            .floor_key(&last_updated_block_height)
            .unwrap()
            - 1; // -1 because TreeMap::iter_from start is _exclusive_

        // We explicitly want to _exclude_ `until_block_height` because the
        // intended use of this method is that it will be
        // `env::block_height()`, and in this case, it would be possible for us
        // to miss some rewards if they were distributed in the same block but
        // after this function call.
        if start_from_block_height >= until_block_height {
            return (0, last_updated_block_height);
        }

        let mut accumulated_fees_in_span = 0;
        let mut last_block_height = start_from_block_height;

        for (block_height, fees) in reward_distribution_log.iter_from(start_from_block_height) {
            if block_height >= until_block_height {
                break;
            }

            let total_loan_asset_deposited_at_distribution = self
                .total_borrow_asset_deposited_log
                .get(
                    &self
                        .total_borrow_asset_deposited_log
                        .floor_key(&block_height)
                        .unwrap(),
                )
                .unwrap();

            // this discards fractional fees
            let portion_of_fees = fees
                .checked_mul(deposit_during_interval)
                .unwrap()
                .checked_div(total_loan_asset_deposited_at_distribution)
                .unwrap();

            accumulated_fees_in_span += portion_of_fees;

            last_block_height = block_height;
        }

        (accumulated_fees_in_span, last_block_height)
    }

    pub fn can_borrow_position_be_liquidated(
        &self,
        account_id: &AccountId,
        collateral_asset_price: Rational<u128>,
        borrow_asset_price: Rational<u128>,
    ) -> bool {
        let Some(borrow_position) = self.borrow_positions.get(account_id) else {
            return false;
        };

        !borrow_position.is_healthy(
            collateral_asset_price,
            borrow_asset_price,
            self.configuration
                .minimum_collateral_ratio_per_borrow
                .upcast(),
        )
    }

    pub fn record_full_liquidation(
        &mut self,
        account_id: &AccountId,
        recovered_borrow_asset_amount: u128,
    ) {
        let mut borrow_position = self.borrow_positions.get(account_id).unwrap_or_default();

        borrow_position.zero_out_collateral_asset_deposit();

        if let Some(margin) =
            recovered_borrow_asset_amount.checked_sub(borrow_position.borrow_asset_liability.0)
        {
            // distribute rewards
            self.record_borrow_asset_reward_distribution(margin);
        } else {
            // we took a loss
            borrow_position
                .decrease_borrow_asset_liability(recovered_borrow_asset_amount)
                .unwrap();
        }
        // TODO: Update totals.

        self.borrow_positions.insert(account_id, &borrow_position);

        // TODO: Do we actually want to decrease the balance here?
        // We still hold the collateral, it's just not "deposited", and it
        // should all be distributed to liquidity providers.
        // Probably this problem will go away once we perform a real
        // liquidation (i.e. sale of collateral assets).
        // self.collateral_asset_balance = self
        //     .collateral_asset_balance
        //     .checked_sub(liquidated_collateral)
        //     .unwrap_or_else(|| env::panic_str("Total collateral deposited underflow"));
        // self.borrow_asset_balance = self
        //     .borrow_asset_balance
        //     .checked_sub(liquidated_loan)
        //     .unwrap_or_else(|| env::panic_str("Total loan asset borrowed underflow"));
    }
}
