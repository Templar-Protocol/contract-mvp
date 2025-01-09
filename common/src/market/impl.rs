use near_sdk::{
    collections::{TreeMap, UnorderedMap},
    env, near, require, AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::{
    borrow::BorrowPosition, market::MarketConfiguration, supply::SupplyPosition,
    withdrawal_queue::WithdrawalQueue,
};

use super::OraclePriceProof;

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    SupplyPositions,
    BorrowPositions,
    TotalBorrowAssetDepositedLog,
    BorrowAssetRewardDistributionLog,
    WithdrawalQueue,
}

#[near]
pub struct Market {
    prefix: Vec<u8>,
    pub configuration: MarketConfiguration,
    /// There are two different balance records for the borrow asset. The
    /// current balance is `borrow_asset_balance = borrow_asset_deposited -
    /// <amount loaned out>`.
    pub borrow_asset_deposited: u128,
    pub supply_positions: UnorderedMap<AccountId, SupplyPosition>,
    pub borrow_positions: UnorderedMap<AccountId, BorrowPosition>,
    pub total_borrow_asset_deposited_log: TreeMap<u64, u128>,
    pub borrow_asset_reward_distribution_log: TreeMap<u64, u128>,
    pub withdrawal_queue: WithdrawalQueue,
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
            supply_positions: UnorderedMap::new(key!(SupplyPositions)),
            borrow_positions: UnorderedMap::new(key!(BorrowPositions)),
            total_borrow_asset_deposited_log: TreeMap::new(key!(TotalBorrowAssetDepositedLog)),
            borrow_asset_reward_distribution_log: TreeMap::new(key!(
                BorrowAssetRewardDistributionLog
            )),
            withdrawal_queue: WithdrawalQueue::new(key!(WithdrawalQueue)),
        }
    }

    pub fn try_lock_next_withdrawal_request(&mut self) -> Result<Option<(AccountId, u128)>, ()> {
        let Some((account_id, requested_amount)) = self.withdrawal_queue.try_lock() else {
            // "Could not lock withdrawal queue. The queue may be empty or a withdrawal may be in-flight."
            return Err(());
        };

        let Some((amount, mut supply_position)) =
            self.supply_positions
                .get(&account_id)
                .and_then(|supply_position| {
                    // Cap withdrawal amount to deposit amount at most.
                    let amount = supply_position
                        .get_borrow_asset_deposit()
                        .min(requested_amount);

                    if amount > 0 {
                        Some((amount, supply_position))
                    } else {
                        None
                    }
                })
        else {
            // env::log_str("Supply position does not exist: skipping.");
            self.withdrawal_queue
                .try_pop()
                .unwrap_or_else(|| env::panic_str("Inconsistent state")); // we just locked the queue
            return Ok(None);
        };

        self.record_supply_position_borrow_asset_withdrawal(&mut supply_position, amount);

        Ok(Some((account_id, amount)))
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
        supply_position: &mut SupplyPosition,
        amount: u128,
    ) {
        self.accumulate_rewards_on_supply_position(supply_position, env::block_height());
        supply_position
            .increase_borrow_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset overflow"));

        self.borrow_asset_deposited = self
            .borrow_asset_deposited
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited overflow"));

        self.log_borrow_asset_deposited(self.borrow_asset_deposited);
    }

    pub fn record_supply_position_borrow_asset_withdrawal(
        &mut self,
        supply_position: &mut SupplyPosition,
        amount: u128,
    ) {
        self.accumulate_rewards_on_supply_position(supply_position, env::block_height());
        supply_position
            .decrease_borrow_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset underflow"));

        self.borrow_asset_deposited = self
            .borrow_asset_deposited
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited underflow"));

        self.log_borrow_asset_deposited(self.borrow_asset_deposited);
    }

    pub fn record_borrow_position_collateral_asset_deposit(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: u128,
    ) {
        borrow_position
            .increase_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset overflow"));
    }

    pub fn record_borrow_position_collateral_asset_withdrawal(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: u128,
    ) {
        borrow_position
            .decrease_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset underflow"));
    }

    pub fn record_borrow_position_borrow_asset_withdrawal(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: u128,
        fees: u128,
    ) {
        // TODO: Recalculate fees here as well!
        borrow_position
            .borrow_asset_fees
            .accumulate_fees(fees, env::block_height());
        borrow_position
            .increase_borrow_asset_principal(amount)
            .unwrap_or_else(|| env::panic_str("Increase borrow asset principal overflow"));
    }

    pub fn record_borrow_position_borrow_asset_repay(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: u128,
    ) {
        let liability_reduction = borrow_position.reduce_borrow_asset_liability(amount);

        require!(
            liability_reduction.amount_remaining == 0,
            "Overpayment not supported",
        );

        self.record_borrow_asset_reward_distribution(liability_reduction.amount_to_fees);
    }

    /// In order for rewards calculations to be accurate, this function MUST
    /// BE CALLED every time a supply position's deposit changes. This
    /// requirement is largely met by virtue of the fact that
    /// `SupplyPosition->borrow_asset_deposit` is a private field and can only
    /// be modified via `Self::record_supply_position_*` methods.
    pub fn accumulate_rewards_on_supply_position(
        &self,
        supply_position: &mut SupplyPosition,
        until_block_height: u64,
    ) {
        let (accumulated, last_block_height) = self.calculate_supply_position_rewards(
            &self.borrow_asset_reward_distribution_log,
            supply_position
                .borrow_asset_rewards
                .last_updated_block_height
                .0,
            supply_position.get_borrow_asset_deposit(),
            until_block_height,
        );

        supply_position
            .borrow_asset_rewards
            .accumulate_rewards(accumulated, last_block_height);
    }

    pub fn calculate_supply_position_rewards(
        &self,
        reward_distribution_log: &TreeMap<u64, u128>,
        last_updated_block_height: u64,
        borrow_asset_deposited_during_interval: u128,
        until_block_height: u64,
    ) -> (u128, u64) {
        let start_from_block_height = reward_distribution_log
            .floor_key(&last_updated_block_height)
            .map(|i| i - 1) // -1 because TreeMap::iter_from start is _exclusive_
            .unwrap_or(0);

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
                .checked_mul(borrow_asset_deposited_during_interval)
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
        oracle_price_proof: OraclePriceProof,
    ) -> bool {
        let Some(borrow_position) = self.borrow_positions.get(account_id) else {
            return false;
        };

        !self
            .configuration
            .is_healthy(&borrow_position, oracle_price_proof)
    }

    pub fn record_full_liquidation(
        &mut self,
        borrow_position: &mut BorrowPosition,
        recovered_borrow_asset_amount: u128,
    ) {
        if let Some(margin) =
            recovered_borrow_asset_amount.checked_sub(borrow_position.get_borrow_asset_principal())
        {
            // distribute rewards
            self.record_borrow_asset_reward_distribution(margin);
        } else {
            // we took a loss
            // TODO: some sort of recovery for suppliers
            todo!("Took a loss during liquidation");
        }
    }
}
