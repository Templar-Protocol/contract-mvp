use near_sdk::{
    env, json_types::U128, near, require, serde_json, AccountId, Promise, PromiseOrValue,
};
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus},
    market::{BorrowAssetMetrics, MarketConfiguration, MarketExternalInterface, OraclePriceProof},
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};

use crate::{Contract, ContractExt};

#[near]
impl MarketExternalInterface for Contract {
    fn get_configuration(&self) -> MarketConfiguration {
        self.configuration.clone()
    }

    fn get_borrow_asset_metrics(
        &self,
        borrow_asset_balance: BorrowAssetAmount,
    ) -> BorrowAssetMetrics {
        BorrowAssetMetrics {
            available: self.get_borrow_asset_available_to_borrow(borrow_asset_balance),
            deposited: self.borrow_asset_deposited,
        }
    }

    fn list_borrows(&self, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId> {
        let offset = offset.map_or(0, |o| o as usize);
        let count = count.map_or(usize::MAX, |c| c as usize);
        self.borrow_positions
            .keys()
            .skip(offset)
            .take(count)
            .collect()
    }

    fn list_supplys(&self, offset: Option<u32>, count: Option<u32>) -> Vec<AccountId> {
        let offset = offset.map_or(0, |o| o as usize);
        let count = count.map_or(usize::MAX, |c| c as usize);
        self.supply_positions
            .keys()
            .skip(offset)
            .take(count)
            .collect()
    }

    fn get_borrow_position(&self, account_id: AccountId) -> Option<BorrowPosition> {
        self.borrow_positions.get(&account_id)
    }

    fn get_borrow_status(
        &self,
        account_id: AccountId,
        oracle_price_proof: OraclePriceProof,
    ) -> Option<BorrowStatus> {
        let borrow_position = self.borrow_positions.get(&account_id)?;

        Some(self.configuration.borrow_status(
            &borrow_position,
            oracle_price_proof,
            env::block_timestamp_ms(),
        ))
    }

    fn borrow(
        &mut self,
        amount: BorrowAssetAmount,
        oracle_price_proof: OraclePriceProof,
    ) -> Promise {
        require!(!amount.is_zero(), "Borrow amount must be greater than zero");
        require!(
            amount >= self.configuration.minimum_borrow_amount,
            "Borrow amount is smaller than minimum allowed",
        );
        require!(
            amount <= self.configuration.maximum_borrow_amount,
            "Borrow amount is greater than maximum allowed",
        );

        let account_id = env::predecessor_account_id();

        // -> (current asset balance, price data)
        self.configuration
            .borrow_asset
            .current_account_balance()
            .and(
                #[allow(clippy::unwrap_used)]
                // TODO: Replace with call to actual price oracle.
                Self::ext(env::current_account_id())
                    .return_static(serde_json::to_value(oracle_price_proof).unwrap()),
            )
            .then(
                Self::ext(env::current_account_id())
                    .borrow_01_consume_balance_and_price(account_id, amount),
            )
    }

    fn withdraw_collateral(
        &mut self,
        amount: U128,
        oracle_price_proof: Option<OraclePriceProof>,
    ) -> Promise {
        let amount = CollateralAssetAmount::new(amount.0);

        let account_id = env::predecessor_account_id();

        let Some(mut borrow_position) = self.borrow_positions.get(&account_id) else {
            env::panic_str("No borrower record. Please deposit collateral first.");
        };

        self.record_borrow_position_collateral_asset_withdrawal(&mut borrow_position, amount);

        if !borrow_position.get_total_borrow_asset_liability().is_zero() {
            require!(
                self.configuration.is_within_minimum_collateral_ratio(
                    &borrow_position,
                    oracle_price_proof.unwrap_or_else(|| env::panic_str("Must provide price")),
                ),
                "Borrow must still be above MCR after collateral withdrawal.",
            );
        }

        self.borrow_positions.insert(&account_id, &borrow_position);

        self.configuration
            .collateral_asset
            .transfer(account_id, amount) // TODO: Check for failure
            .then(Self::ext(env::current_account_id()).return_static(serde_json::Value::Null))
    }

    fn get_supply_position(&self, account_id: AccountId) -> Option<SupplyPosition> {
        self.supply_positions.get(&account_id)
    }

    /// If the predecessor has already entered the queue, calling this function
    /// will reset the position to the back of the queue.
    fn create_supply_withdrawal_request(&mut self, amount: U128) {
        let amount = BorrowAssetAmount::from(amount.0);
        require!(
            !amount.is_zero(),
            "Amount to withdraw must be greater than zero",
        );
        let predecessor = env::predecessor_account_id();
        if self
            .supply_positions
            .get(&predecessor)
            .filter(|supply_position| !supply_position.get_borrow_asset_deposit().is_zero())
            .is_none()
        {
            env::panic_str("Supply position does not exist");
        }

        // TODO: Check that amount is a sane value? i.e. within the amount actually deposited?
        // Probably not, since this should be checked during the actual execution of the withdrawal.
        // No sense duplicating the check, probably.
        self.withdrawal_queue.remove(&predecessor);
        self.withdrawal_queue.insert_or_update(&predecessor, amount);
    }

    fn cancel_supply_withdrawal_request(&mut self) {
        self.withdrawal_queue.remove(&env::predecessor_account_id());
    }

    fn execute_next_supply_withdrawal_request(&mut self) -> PromiseOrValue<()> {
        let Some((account_id, amount)) = self
            .try_lock_next_withdrawal_request()
            .unwrap_or_else(|e| env::panic_str(&e.to_string()))
        else {
            env::log_str("Supply position does not exist: skipping.");
            return PromiseOrValue::Value(());
        };

        PromiseOrValue::Promise(
            self.configuration
                .borrow_asset
                .transfer(account_id.clone(), amount)
                .then(
                    Self::ext(env::current_account_id())
                        .after_execute_next_withdrawal(account_id.clone(), amount),
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

    fn harvest_yield(&mut self) {
        let predecessor = env::predecessor_account_id();
        if let Some(mut supply_position) = self.supply_positions.get(&predecessor) {
            self.accumulate_yield_on_supply_position(&mut supply_position, env::block_height());
            self.supply_positions.insert(&predecessor, &supply_position);
        }
    }

    fn get_static_yield(&self, account_id: AccountId) -> Option<StaticYieldRecord> {
        self.static_yield.get(&account_id)
    }

    fn withdraw_supply_yield(&mut self, amount: Option<U128>) -> Promise {
        let predecessor = env::predecessor_account_id();
        let Some(mut supply_position) = self.supply_positions.get(&predecessor) else {
            env::panic_str("Supply position does not exist");
        };

        let amount = amount.map_or_else(
            || supply_position.borrow_asset_yield.amount.as_u128(),
            |amount| amount.0,
        );

        let withdrawn = supply_position
            .borrow_asset_yield
            .withdraw(amount)
            .unwrap_or_else(|| {
                env::panic_str("Attempt to withdraw more yield than has accumulated")
            });
        if withdrawn.is_zero() {
            env::panic_str("No rewards can be withdrawn");
        }
        self.supply_positions.insert(&predecessor, &supply_position);

        // TODO: Check for transfer success.
        self.configuration
            .borrow_asset
            .transfer(predecessor, withdrawn)
    }

    fn withdraw_static_yield(
        &mut self,
        borrow_asset_amount: Option<BorrowAssetAmount>,
        collateral_asset_amount: Option<CollateralAssetAmount>,
    ) -> Promise {
        let predecessor = env::predecessor_account_id();
        let Some(mut static_yield_record) = self.static_yield.get(&predecessor) else {
            env::panic_str("Yield record does not exist");
        };

        let (borrow_asset_amount, collateral_asset_amount) =
            if borrow_asset_amount.is_none() && collateral_asset_amount.is_none() {
                // no arguments = withdraw all
                (
                    static_yield_record.borrow_asset,
                    static_yield_record.collateral_asset,
                )
            } else {
                (
                    borrow_asset_amount.unwrap_or_default(),
                    collateral_asset_amount.unwrap_or_default(),
                )
            };

        static_yield_record
            .borrow_asset
            .split(borrow_asset_amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset yield underflow"));
        static_yield_record
            .collateral_asset
            .split(collateral_asset_amount)
            .unwrap_or_else(|| env::panic_str("Collateral asset yield underflow"));

        self.static_yield.insert(&predecessor, &static_yield_record);

        let borrow_promise = if borrow_asset_amount.is_zero() {
            None
        } else {
            Some(
                self.configuration
                    .borrow_asset
                    .transfer(predecessor.clone(), borrow_asset_amount),
            )
        };

        let collateral_promise = if collateral_asset_amount.is_zero() {
            None
        } else {
            Some(
                self.configuration
                    .collateral_asset
                    .transfer(predecessor.clone(), collateral_asset_amount),
            )
        };

        match (borrow_promise, collateral_promise) {
            (Some(b), Some(c)) => b.and(c),
            (Some(p), _) | (_, Some(p)) => p,
            _ => env::panic_str("No yield to withdraw"),
        } // TODO: Check for success
    }

    #[payable]
    fn supply_native(&mut self) {
        require!(
            self.configuration.borrow_asset.is_native(),
            "Unsupported borrow asset",
        );

        let amount = BorrowAssetAmount::from(env::attached_deposit().as_yoctonear());

        require!(!amount.is_zero(), "Deposit must be nonzero");

        self.execute_supply(&env::predecessor_account_id(), amount);
    }

    #[payable]
    fn collateralize_native(&mut self) {
        require!(
            self.configuration.collateral_asset.is_native(),
            "Unsupported collateral asset",
        );

        let amount = CollateralAssetAmount::from(env::attached_deposit().as_yoctonear());

        require!(!amount.is_zero(), "Deposit must be nonzero");

        self.execute_collateralize(&env::predecessor_account_id(), amount);
    }

    #[payable]
    fn repay_native(&mut self) -> PromiseOrValue<()> {
        require!(
            self.configuration.borrow_asset.is_native(),
            "Unsupported borrow asset",
        );

        let amount = BorrowAssetAmount::from(env::attached_deposit().as_yoctonear());

        require!(!amount.is_zero(), "Deposit must be nonzero");

        let predecessor = env::predecessor_account_id();

        let refund = self.execute_repay(&predecessor, amount);

        if refund.is_zero() {
            PromiseOrValue::Value(())
        } else {
            PromiseOrValue::Promise(
                self.configuration
                    .borrow_asset
                    .transfer(predecessor, amount),
            )
        }
    }

    #[payable]
    fn liquidate_native(
        &mut self,
        account_id: AccountId,
        oracle_price_proof: OraclePriceProof,
    ) -> Promise {
        require!(
            self.configuration.borrow_asset.is_native(),
            "Unsupported borrow asset",
        );

        let amount = BorrowAssetAmount::from(env::attached_deposit().as_yoctonear());

        require!(!amount.is_zero(), "Deposit must be nonzero");

        let liquidated_collateral =
            self.execute_liquidate_initial(&account_id, amount, oracle_price_proof);

        let liquidator_id = env::predecessor_account_id();

        self.configuration
            .collateral_asset
            .transfer(liquidator_id.clone(), liquidated_collateral)
            .then(Self::ext(env::current_account_id()).after_liquidate_native(
                liquidator_id,
                account_id,
                amount,
            ))
    }
}
