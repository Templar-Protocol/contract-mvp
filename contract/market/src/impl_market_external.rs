use std::collections::HashMap;

use near_sdk::{env, near, require, AccountId, Promise, PromiseOrValue};
use templar_common::{
    accumulator::Accumulator,
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus},
    contract::list,
    market::{BorrowAssetMetrics, HarvestYieldMode, MarketConfiguration, MarketExternalInterface},
    oracle::pyth::OracleResponse,
    self_ext,
    snapshot::Snapshot,
    supply::{SupplyPosition, WithdrawalAttempt},
    withdrawal_queue::{
        WithdrawalQueueExecutionResult, WithdrawalQueueStatus, WithdrawalRequestStatus,
    },
};
use templar_primitives::Decimal;

use crate::{Contract, ContractExt};

#[near]
impl MarketExternalInterface for Contract {
    fn get_configuration(&self) -> MarketConfiguration {
        self.configuration.clone()
    }

    fn get_current_snapshot(&self) -> Snapshot {
        self.current_snapshot()
    }

    fn get_finalized_snapshots_len(&self) -> u32 {
        self.finalized_snapshots.len()
    }

    fn get_borrow_asset_metrics(&self) -> BorrowAssetMetrics {
        BorrowAssetMetrics {
            available: self.get_borrow_asset_available_to_borrow(),
            deposited_active: self.borrow_asset_deposited_active,
            deposited_incoming: self
                .borrow_asset_deposited_incoming
                .iter()
                .map(|incoming| (incoming.activate_at_snapshot_index, incoming.amount))
                .collect(),
            borrowed: self.borrowed(),
            paid_to_fees: self.borrow_asset_paid_to_fees,
        }
    }

    fn list_borrow_positions(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> HashMap<AccountId, BorrowPosition> {
        list(self.iter_borrow_positions(), offset, count)
    }

    fn list_finalized_snapshots(&self, offset: Option<u32>, count: Option<u32>) -> Vec<&Snapshot> {
        list(&self.finalized_snapshots, offset, count)
    }

    fn list_supply_positions(
        &self,
        offset: Option<u32>,
        count: Option<u32>,
    ) -> HashMap<AccountId, SupplyPosition> {
        list(self.iter_supply_positions(), offset, count)
    }

    fn get_borrow_position(&self, account_id: AccountId) -> Option<BorrowPosition> {
        let borrow_position = self.borrow_position_ref(account_id)?;
        Some(borrow_position.inner().clone())
    }

    fn get_borrow_position_pending_interest(
        &self,
        account_id: AccountId,
        snapshot_limit: Option<u32>,
    ) -> Option<BorrowAssetAmount> {
        let limit = snapshot_limit.unwrap_or(u32::MAX);
        let position = self.borrow_position_ref(account_id)?;
        Some(position.calculate_interest(limit).get_amount())
    }

    fn get_borrow_status(
        &self,
        account_id: AccountId,
        oracle_response: OracleResponse,
    ) -> Option<BorrowStatus> {
        let borrow_position = self.borrow_position_ref(account_id)?;

        let price_pair = self
            .configuration
            .price_oracle_configuration
            .create_price_pair(&oracle_response)
            .unwrap_or_else(|e| templar_common::panic_with_message(&e.to_string()));

        Some(borrow_position.status(&price_pair, env::block_timestamp_ms()))
    }

    fn borrow(&mut self, amount: BorrowAssetAmount) -> Promise {
        require!(!amount.is_zero(), "Borrow amount must be greater than zero");
        let account_id = env::predecessor_account_id();
        require!(
            self.borrow_position_ref(account_id.clone()).is_some(),
            "Borrow position does not exist",
        );

        self.configuration
            .price_oracle_configuration
            .retrieve_price_pair()
            .then(
                self_ext!(Self::GAS_BORROW_01_CONSUME_PRICE)
                    .borrow_01_consume_price(account_id, amount),
            )
    }

    fn withdraw_collateral(&mut self, amount: CollateralAssetAmount) -> Promise {
        let account_id = env::predecessor_account_id();

        let snapshot = self.snapshot();
        let Some(mut borrow_position) = self.borrow_position_guard(snapshot, account_id.clone())
        else {
            templar_common::panic_with_message(
                "No borrower record. Please deposit collateral first.",
            );
        };

        if borrow_position
            .inner()
            .get_total_borrow_asset_liability()
            .is_zero()
        {
            // No need to retrieve prices, since there is zero liability.
            let proof = borrow_position.accumulate_interest();
            borrow_position.record_collateral_asset_withdrawal_initial(proof, amount);
            drop(borrow_position);

            self.configuration
                .collateral_asset
                .transfer(account_id.clone(), amount)
                .then(
                    self_ext!(Self::GAS_WITHDRAW_COLLATERAL_02_FINALIZE)
                        .withdraw_collateral_02_finalize(account_id, amount),
                )
        } else {
            drop(borrow_position);
            // They still have liability, so we need to check prices.
            self.configuration
                .price_oracle_configuration
                .retrieve_price_pair()
                .then(
                    self_ext!(Self::GAS_WITHDRAW_COLLATERAL_01_CONSUME_PRICE)
                        .withdraw_collateral_01_consume_price(account_id, amount),
                )
        }
    }

    fn apply_interest(&mut self, account_id: Option<AccountId>, snapshot_limit: Option<u32>) {
        let account_id = account_id.unwrap_or_else(env::predecessor_account_id);
        let snapshot = self.snapshot();
        if let Some(mut borrow_position) = self.borrow_position_guard(snapshot, account_id) {
            borrow_position.accumulate_interest_partial(snapshot_limit.unwrap_or(u32::MAX));
        }
    }

    fn get_supply_position(&self, account_id: AccountId) -> Option<SupplyPosition> {
        let position = self.supply_position_ref(account_id)?;
        Some(position.inner().clone())
    }

    fn get_supply_position_pending_yield(
        &self,
        account_id: AccountId,
        snapshot_limit: Option<u32>,
    ) -> Option<BorrowAssetAmount> {
        let limit = snapshot_limit.unwrap_or(u32::MAX);
        let position = self.supply_position_ref(account_id)?;
        Some(position.calculate_yield(limit).get_amount())
    }

    /// If the predecessor has already entered the queue, calling this function
    /// will reset the position to the back of the queue.
    fn create_supply_withdrawal_request(&mut self, amount: BorrowAssetAmount) {
        require!(
            !amount.is_zero(),
            "Amount to withdraw must be greater than zero",
        );
        let predecessor = env::predecessor_account_id();
        let Some(supply_position) = self
            .supply_position_ref(predecessor.clone())
            .filter(|supply_position| !supply_position.total_deposit().is_zero())
        else {
            templar_common::panic_with_message("Supply position does not exist");
        };

        // We do check here, as well as during the execution.
        // This check really only ensures that the `depth` reported by
        // get_supply_withdrawal_queue_status() is realistically accurate.
        require!(
            supply_position.total_deposit() >= amount,
            "Attempt to withdraw more than current deposit",
        );
        require!(
            self.configuration.supply_withdrawal_range.contains(amount),
            "Withdrawal amount is outside of allowable range",
        );

        self.withdrawal_queue.remove(&predecessor);
        self.withdrawal_queue.insert_or_update(&predecessor, amount);
    }

    fn cancel_supply_withdrawal_request(&mut self) {
        self.withdrawal_queue.remove(&env::predecessor_account_id());
    }

    fn execute_next_supply_withdrawal_request(
        &mut self,
        batch_limit: Option<u32>,
    ) -> PromiseOrValue<WithdrawalQueueExecutionResult> {
        let batch_limit = batch_limit.unwrap_or(1);
        let mut batch = Vec::with_capacity(batch_limit.min(self.withdrawal_queue.len()) as usize);
        let snapshot_proof = self.snapshot();
        let block_timestamp_ms = env::block_timestamp_ms();
        let queue_len_start = self.withdrawal_queue.len();
        let mut depth_cleared = BorrowAssetAmount::zero();

        while let Some((account_id, requested_amount)) = self.withdrawal_queue.peek() {
            if batch.len() >= batch_limit as usize {
                break;
            }

            let withdrawal_attempt = {
                let Some(mut position_guard) =
                    self.supply_position_guard(snapshot_proof, account_id.clone())
                else {
                    // Somehow the account does not exist. This should not happen,
                    // but it is recoverable if it does.
                    self.withdrawal_queue.pop();
                    depth_cleared += requested_amount;
                    continue;
                };
                let accumulation_proof = position_guard.accumulate_yield();
                position_guard.record_withdrawal_initial(
                    accumulation_proof,
                    requested_amount,
                    block_timestamp_ms,
                )
            };

            match withdrawal_attempt {
                WithdrawalAttempt::EmptyPosition => {
                    self.withdrawal_queue.pop();
                    depth_cleared += requested_amount;
                }
                WithdrawalAttempt::NoLiquidity => {
                    break;
                }
                WithdrawalAttempt::Full(withdrawal) => {
                    batch.push(withdrawal);
                    self.withdrawal_queue.pop();
                    depth_cleared += requested_amount;
                }
                WithdrawalAttempt::Partial {
                    withdrawal,
                    remaining,
                } => {
                    batch.push(withdrawal);
                    self.withdrawal_queue.mut_head(|a| *a = remaining);
                    depth_cleared += requested_amount - remaining;
                    break;
                }
            }
        }

        let result = WithdrawalQueueExecutionResult {
            depth: depth_cleared,
            length: queue_len_start - self.withdrawal_queue.len(),
        };

        let Some(transfers) = batch
            .iter()
            .map(|resolution| {
                self.configuration
                    .borrow_asset
                    .transfer(resolution.account_id.clone(), resolution.amount_to_account)
            })
            .reduce(|a, b| a.and(b))
        else {
            return PromiseOrValue::Value(result);
        };

        PromiseOrValue::Promise(
            transfers.then(
                self_ext!(Self::GAS_EXECUTE_NEXT_SUPPLY_WITHDRAWAL_REQUEST_01_FINALIZE)
                    .execute_next_supply_withdrawal_request_01_finalize(batch, result),
            ),
        )
    }

    fn get_supply_withdrawal_request_status(
        &self,
        account_id: AccountId,
    ) -> Option<WithdrawalRequestStatus> {
        self.withdrawal_queue.get_request_status(&account_id)
    }

    fn get_supply_withdrawal_queue_status(&self) -> WithdrawalQueueStatus {
        self.withdrawal_queue.get_status()
    }

    fn harvest_yield(
        &mut self,
        account_id: Option<AccountId>,
        mode: Option<HarvestYieldMode>,
    ) -> BorrowAssetAmount {
        let mode = mode.unwrap_or_default();
        let predecessor = env::predecessor_account_id();
        let account_id = account_id.unwrap_or_else(|| predecessor.clone());

        let snapshot = self.snapshot();
        let Some(mut supply_position) = self.supply_position_guard(snapshot, account_id) else {
            return BorrowAssetAmount::zero();
        };

        match mode {
            HarvestYieldMode::SnapshotLimit(limit) => {
                supply_position.accumulate_yield_partial(limit);
            }
            HarvestYieldMode::Default => {
                supply_position.accumulate_yield();
            }
        }

        BorrowAssetAmount::zero()
    }

    fn get_last_yield_rate(&self) -> Decimal {
        self.configuration
            .supply_yield_rate_from_interest(&self.current_snapshot())
    }

    fn get_static_yield(&self, account_id: AccountId) -> Option<Accumulator<BorrowAsset>> {
        self.static_yield.get(&account_id)
    }

    fn accumulate_static_yield(
        &mut self,
        account_id: Option<AccountId>,
        snapshot_limit: Option<u32>,
    ) {
        self.market
            .accumulate_static_yield(
                &account_id.unwrap_or_else(env::predecessor_account_id),
                snapshot_limit.unwrap_or(u32::MAX),
            )
            .unwrap_or_else(|_| {
                templar_common::panic_with_message("This account does not earn static yield")
            });
    }

    fn withdraw_static_yield(&mut self, amount: Option<BorrowAssetAmount>) -> Promise {
        let predecessor = env::predecessor_account_id();
        let Some(mut yield_record) = self.static_yield.get(&predecessor) else {
            templar_common::panic_with_message("Yield record does not exist");
        };

        let available = yield_record.get_total();
        let amount = amount.unwrap_or(available);
        require!(
            amount <= available,
            "Attempt to withdraw more than accumulated static yield",
        );

        yield_record.remove(amount);

        self.static_yield.insert(&predecessor, &yield_record);
        self.borrow_asset_balance = self.borrow_asset_balance.unwrap_sub(
            amount,
            "Invariant violation: static yield withdrawal exceeds market borrow asset balance",
        );

        self.configuration
            .borrow_asset
            .transfer(predecessor.clone(), amount)
            .then(
                self_ext!(Self::GAS_WITHDRAW_STATIC_YIELD_01_FINALIZE)
                    .withdraw_static_yield_01_finalize(predecessor, amount),
            )
    }
}
