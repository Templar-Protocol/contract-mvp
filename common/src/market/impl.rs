use std::{u128, u16};

use near_sdk::{
    collections::{LookupMap, TreeMap, UnorderedMap},
    env, near, require, AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::{
    asset::{AssetClass, BorrowAssetAmount, CollateralAssetAmount, FungibleAssetAmount},
    borrow::BorrowPosition,
    market::MarketConfiguration,
    rational::Rational,
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::WithdrawalQueue,
};

use super::OraclePriceProof;

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    SupplyPositions,
    BorrowPositions,
    TotalBorrowAssetDepositedLog,
    BorrowAssetYieldDistributionLog,
    WithdrawalQueue,
    StaticYield,
}

#[near]
pub struct Market {
    prefix: Vec<u8>,
    pub configuration: MarketConfiguration,
    pub borrow_asset_deposited: BorrowAssetAmount,
    pub supply_positions: UnorderedMap<AccountId, SupplyPosition>,
    pub borrow_positions: UnorderedMap<AccountId, BorrowPosition>,
    pub total_borrow_asset_deposited_log: TreeMap<u64, BorrowAssetAmount>,
    pub borrow_asset_yield_distribution_log: TreeMap<u64, BorrowAssetAmount>,
    pub withdrawal_queue: WithdrawalQueue,
    pub static_yield: LookupMap<AccountId, StaticYieldRecord>,
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
            borrow_asset_deposited: 0.into(),
            supply_positions: UnorderedMap::new(key!(SupplyPositions)),
            borrow_positions: UnorderedMap::new(key!(BorrowPositions)),
            total_borrow_asset_deposited_log: TreeMap::new(key!(TotalBorrowAssetDepositedLog)),
            borrow_asset_yield_distribution_log: TreeMap::new(key!(
                BorrowAssetYieldDistributionLog
            )),
            withdrawal_queue: WithdrawalQueue::new(key!(WithdrawalQueue)),
            static_yield: LookupMap::new(key!(StaticYield)),
        }
    }

    pub fn try_lock_next_withdrawal_request(
        &mut self,
    ) -> Result<Option<(AccountId, BorrowAssetAmount)>, ()> {
        let Some((account_id, requested_amount)) = self.withdrawal_queue.try_lock() else {
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

                    if amount.is_zero() {
                        None
                    } else {
                        Some((amount, supply_position))
                    }
                })
        else {
            // The amount that the entry is eligible to withdraw is zero, so skip it.
            self.withdrawal_queue
                .try_pop()
                .unwrap_or_else(|| env::panic_str("Inconsistent state")); // we just locked the queue
            return Ok(None);
        };

        self.record_supply_position_borrow_asset_withdrawal(&mut supply_position, amount);

        Ok(Some((account_id, amount)))
    }

    fn log_borrow_asset_deposited(&mut self, amount: BorrowAssetAmount) {
        let block_height = env::block_height();
        self.total_borrow_asset_deposited_log
            .insert(&block_height, &amount);
    }

    fn record_borrow_asset_yield_distribution(&mut self, mut amount: BorrowAssetAmount) {
        // Sanity.
        if amount.is_zero() {
            return;
        }

        // First, static yield.

        let total_weight = u16::from(self.configuration.yield_weights.total_weight()) as u128;
        let total_amount = amount.as_u128();
        if total_weight != 0 {
            for (account_id, share) in self.configuration.yield_weights.r#static.iter() {
                let portion = amount
                    .split(
                        // Safety:
                        // total_weight is guaranteed >0 and <=u16::MAX
                        // share is guaranteed <=u16::MAX
                        // Therefore, as long as total_amount <= u128::MAX / u16::MAX, this will never overflow.
                        // u128::MAX / u16::MAX == 5192376087906286159508272029171713 (0x10001000100010001000100010001)
                        // With 24 decimals, that's about 5,192,376,087 tokens.
                        // TODO: Fix.
                        total_amount
                            .checked_mul(*share as u128)
                            .unwrap() // TODO: This one might panic.
                        / total_weight, // This will never panic: is never div0
                    )
                    // Safety:
                    // Guaranteed share <= total_weight
                    // Guaranteed sum(shares) == total_weight
                    // Guaranteed sum(floor(total_amount * share / total_weight) for each share in shares) <= total_amount
                    // Therefore this should never panic.
                    .unwrap();

                let mut yield_record = self.static_yield.get(account_id).unwrap_or_default();
                // Assuming borrow_asset is implemented correctly:
                // this only panics if the circulating supply is somehow >u128::MAX
                // and we have somehow obtained >u128::MAX amount.
                // TODO: Include warning somewhere about tokens with >u128::MAX supply.
                //
                // Otherwise, borrow_asset is implemented incorrectly.
                // TODO: If that is the case, how to deal?
                yield_record.borrow_asset.join(portion).unwrap();
                self.static_yield.insert(account_id, &yield_record);
            }
        }

        // Next, dynamic (supply-based) yield.

        let block_height = env::block_height();
        let mut distributed_in_block = self
            .borrow_asset_yield_distribution_log
            .get(&block_height)
            .unwrap_or(0.into());
        distributed_in_block.join(amount);
        self.borrow_asset_yield_distribution_log
            .insert(&block_height, &distributed_in_block);
    }

    pub fn record_supply_position_borrow_asset_deposit(
        &mut self,
        supply_position: &mut SupplyPosition,
        amount: BorrowAssetAmount,
    ) {
        self.accumulate_yield_on_supply_position(supply_position, env::block_height());
        supply_position
            .increase_borrow_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset overflow"));

        self.borrow_asset_deposited
            .join(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited overflow"));

        self.log_borrow_asset_deposited(self.borrow_asset_deposited);
    }

    pub fn record_supply_position_borrow_asset_withdrawal(
        &mut self,
        supply_position: &mut SupplyPosition,
        amount: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        self.accumulate_yield_on_supply_position(supply_position, env::block_height());
        let withdrawn = supply_position
            .decrease_borrow_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Supply position borrow asset underflow"));

        self.borrow_asset_deposited
            .split(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited underflow"));

        self.log_borrow_asset_deposited(self.borrow_asset_deposited);

        withdrawn
    }

    pub fn record_borrow_position_collateral_asset_deposit(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: CollateralAssetAmount,
    ) {
        borrow_position
            .increase_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset overflow"));
    }

    pub fn record_borrow_position_collateral_asset_withdrawal(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: CollateralAssetAmount,
    ) {
        borrow_position
            .decrease_collateral_asset_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Borrow position collateral asset underflow"));
    }

    pub fn record_borrow_position_borrow_asset_withdrawal(
        &mut self,
        borrow_position: &mut BorrowPosition,
        amount: BorrowAssetAmount,
        fees: BorrowAssetAmount,
    ) {
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
        amount: BorrowAssetAmount,
    ) {
        let liability_reduction = borrow_position.reduce_borrow_asset_liability(amount);

        require!(
            liability_reduction.amount_remaining.is_zero(),
            "Overpayment not supported",
        );

        self.record_borrow_asset_yield_distribution(liability_reduction.amount_to_fees);
    }

    /// In order for yield calculations to be accurate, this function MUST
    /// BE CALLED every time a supply position's deposit changes. This
    /// requirement is largely met by virtue of the fact that
    /// `SupplyPosition->borrow_asset_deposit` is a private field and can only
    /// be modified via `Self::record_supply_position_*` methods.
    pub fn accumulate_yield_on_supply_position(
        &self,
        supply_position: &mut SupplyPosition,
        until_block_height: u64,
    ) {
        let (accumulated, last_block_height) = self.calculate_supply_position_yield(
            &self.borrow_asset_yield_distribution_log,
            supply_position
                .borrow_asset_yield
                .last_updated_block_height
                .0,
            supply_position.get_borrow_asset_deposit(),
            until_block_height,
        );

        supply_position
            .borrow_asset_yield
            .accumulate_yield(accumulated, last_block_height);
    }

    pub fn calculate_supply_position_yield<T: AssetClass>(
        &self,
        yield_distribution_log: &TreeMap<u64, FungibleAssetAmount<T>>,
        last_updated_block_height: u64,
        borrow_asset_deposited_during_interval: BorrowAssetAmount,
        until_block_height: u64,
    ) -> (FungibleAssetAmount<T>, u64) {
        let start_from_block_height = yield_distribution_log
            .floor_key(&last_updated_block_height)
            .map(|i| i - 1) // -1 because TreeMap::iter_from start is _exclusive_
            .unwrap_or(0);

        // We explicitly want to _exclude_ `until_block_height` because the
        // intended use of this method is that it will be
        // `env::block_height()`, and in this case, it would be possible for us
        // to miss some yield if they were distributed in the same block but
        // after this function call.
        if start_from_block_height >= until_block_height {
            return (0.into(), last_updated_block_height);
        }

        let mut accumulated_fees_in_span = FungibleAssetAmount::<T>::zero();
        let mut last_block_height = start_from_block_height;

        for (block_height, fees) in yield_distribution_log.iter_from(start_from_block_height) {
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
            let share = Rational::new(
                borrow_asset_deposited_during_interval.as_u128(),
                total_loan_asset_deposited_at_distribution.as_u128(),
            );
            let portion_of_fees = share
                .checked_scalar_mul(fees.as_u128())
                .unwrap()
                .floor()
                .unwrap()
                .into();

            accumulated_fees_in_span.join(portion_of_fees);

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
        mut recovered_amount: BorrowAssetAmount,
    ) {
        if recovered_amount
            .split(borrow_position.get_borrow_asset_principal())
            .is_some()
        {
            // distribute yield
            self.record_borrow_asset_yield_distribution(recovered_amount);
        } else {
            // we took a loss
            // TODO: some sort of recovery for suppliers
            todo!("Took a loss during liquidation");
        }
    }
}
