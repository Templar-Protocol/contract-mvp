use near_sdk::{
    env,
    json_types::{U128, U64},
    near, require, serde_json, AccountId, Promise, PromiseError, PromiseOrValue, PromiseResult,
};
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::BorrowPosition,
    market::OraclePriceProof,
    supply::SupplyPosition,
};

use crate::{Contract, ContractExt};

/// Internal helpers.
impl Contract {
    pub fn execute_supply(&mut self, account_id: &AccountId, amount: BorrowAssetAmount) {
        let mut supply_position = self
            .supply_positions
            .get(account_id)
            .unwrap_or_else(|| SupplyPosition::new(env::block_height()));

        self.record_supply_position_borrow_asset_deposit(&mut supply_position, amount);

        self.supply_positions.insert(account_id, &supply_position);
    }

    pub fn execute_collateralize(&mut self, account_id: &AccountId, amount: CollateralAssetAmount) {
        let mut borrow_position = self
            .borrow_positions
            .get(account_id)
            .unwrap_or_else(|| BorrowPosition::new(env::block_height()));

        // TODO: This creates a borrow record implicitly. If we
        // require a discrete "sign-up" step, we will need to add
        // checks before this function call.
        //
        // The sign-up step would only be NFT gating or something of
        // that sort, which is just an additional pre condition check.
        // -- https://github.com/Templar-Protocol/contract-mvp/pull/6#discussion_r1923871982
        self.record_borrow_position_collateral_asset_deposit(&mut borrow_position, amount);

        self.borrow_positions.insert(account_id, &borrow_position);
    }

    /// Returns the amount that should be returned to the account.
    pub fn execute_repay(
        &mut self,
        account_id: &AccountId,
        amount: BorrowAssetAmount,
    ) -> BorrowAssetAmount {
        if let Some(mut borrow_position) = self.borrow_positions.get(account_id) {
            // TODO: This function *errors* on overpayment. Instead, add a
            // check before and only repay the maximum, then return the excess.
            //
            // Due to the slightly imprecise calculation of yield and
            // other fees, the returning of the excess should be
            // anything >1%, for example, over the total amount
            // borrowed + fees/interest.
            // -- https://github.com/Templar-Protocol/contract-mvp/pull/6#discussion_r1923876327
            self.record_borrow_position_borrow_asset_repay(&mut borrow_position, amount);

            self.borrow_positions.insert(account_id, &borrow_position);
            BorrowAssetAmount::zero()
        } else {
            // No borrow exists: just return the whole amount.
            amount
        }
    }

    pub fn execute_liquidate_initial(
        &mut self,
        account_id: &AccountId,
        amount: BorrowAssetAmount,
        oracle_price_proof: OraclePriceProof,
    ) -> CollateralAssetAmount {
        let mut borrow_position = self
            .borrow_positions
            .get(account_id)
            .unwrap_or_else(|| BorrowPosition::new(env::block_height()));

        require!(
            self.configuration
                .borrow_status(
                    &borrow_position,
                    oracle_price_proof,
                    env::block_timestamp_ms(),
                )
                .is_liquidation(),
            "Borrow position cannot be liquidated",
        );

        let minimum_acceptable_amount = self.configuration.minimum_acceptable_liquidation_amount(
            borrow_position.collateral_asset_deposit,
            oracle_price_proof,
        );

        require!(
            amount >= minimum_acceptable_amount,
            "Too little attached to liquidate",
        );

        self.record_liquidation_lock(&mut borrow_position);

        self.borrow_positions.insert(account_id, &borrow_position);

        borrow_position.collateral_asset_deposit
    }

    /// Returns the amount to return to the liquidator.
    pub fn execute_liquidate_final(
        &mut self,
        account_id: &AccountId,
        amount: BorrowAssetAmount,
        success: bool,
    ) -> BorrowAssetAmount {
        let mut borrow_position = self.borrow_positions.get(account_id).unwrap_or_else(|| {
            env::panic_str("Invariant violation: Liquidation of nonexistent position.")
        });

        if success {
            self.record_full_liquidation(&mut borrow_position, amount);
            BorrowAssetAmount::zero()
        } else {
            // Somehow transfer of collateral failed. This could mean:
            //
            // 1. Somehow the contract does not have enough collateral
            //  available. This would be indicative of a *fundamental flaw*
            //  in the contract (i.e. this should never happen).
            //
            // 2. More likely, in a multichain context, communication
            //  broke down somewhere between the signer and the remote RPC.
            //  Could be as simple as a nonce sync issue. Should just wait
            //  and try again later.
            self.record_liquidation_unlock(&mut borrow_position);
            amount
        }
    }
}

/// External helpers.
#[near]
impl Contract {
    pub fn get_total_borrow_asset_deposited_log(&self) -> Vec<(U64, U128)> {
        self.total_borrow_asset_deposited_log
            .iter()
            .map(|(block_height, total)| (block_height.into(), total.as_u128().into()))
            .collect::<Vec<_>>()
    }

    pub fn get_borrow_asset_yield_distribution_log(&self) -> Vec<(U64, U128)> {
        self.borrow_asset_yield_distribution_log
            .iter()
            .map(|(block_height, total)| (block_height.into(), total.as_u128().into()))
            .collect::<Vec<_>>()
    }

    #[private]
    pub fn return_static(&self, value: serde_json::Value) -> serde_json::Value {
        value
    }

    #[private]
    pub fn borrow_01_consume_balance_and_price(
        &mut self,
        account_id: AccountId,
        amount: BorrowAssetAmount,
        #[callback_result] current_balance: Result<BorrowAssetAmount, PromiseError>,
        #[callback_result] oracle_price_proof: Result<OraclePriceProof, PromiseError>,
    ) -> Promise {
        let current_balance = current_balance
            .unwrap_or_else(|_| env::panic_str("Failed to fetch borrow asset current balance."));
        let oracle_price_proof = oracle_price_proof
            .unwrap_or_else(|_| env::panic_str("Failed to fetch price data from oracle."));

        // Ensure we have enough funds to dispense.
        let available_to_borrow = self.get_borrow_asset_available_to_borrow(current_balance);
        require!(
            amount <= available_to_borrow,
            "Insufficient borrow asset available",
        );

        let fees = self
            .configuration
            .borrow_origination_fee
            .of(amount)
            .unwrap_or_else(|| env::panic_str("Fee calculation failed"));

        let Some(mut borrow_position) = self.borrow_positions.get(&account_id) else {
            env::panic_str("No borrower record. Please deposit collateral first.");
        };

        self.record_borrow_position_borrow_asset_in_flight_start(
            &mut borrow_position,
            amount,
            fees,
        );

        require!(
            self.configuration
                .borrow_status(
                    &borrow_position,
                    oracle_price_proof,
                    env::block_timestamp_ms(),
                )
                .is_healthy(),
            "New position would be in liquidation",
        );

        self.borrow_positions.insert(&account_id, &borrow_position);

        self.configuration
            .borrow_asset
            .transfer(account_id.clone(), amount) // TODO: Check for failure
            .then(
                Self::ext(env::current_account_id())
                    .borrow_02_after_transfer(account_id, amount, fees),
            )
    }

    #[private]
    pub fn borrow_02_after_transfer(
        &mut self,
        account_id: AccountId,
        amount: BorrowAssetAmount,
        fees: BorrowAssetAmount,
    ) {
        require!(env::promise_results_count() == 1);

        let Some(mut borrow_position) = self.borrow_positions.get(&account_id) else {
            env::panic_str("Invariant violation: borrow position does not exist after transfer.");
        };

        self.record_borrow_position_borrow_asset_in_flight_end(&mut borrow_position, amount, fees);

        match env::promise_result(0) {
            PromiseResult::Successful(_) => {
                // GREAT SUCCESS
                //
                // Borrow position has already been created: finalize
                // withdrawal record.
                self.record_borrow_position_borrow_asset_withdrawal(
                    &mut borrow_position,
                    amount,
                    fees,
                );
            }
            PromiseResult::Failed => {
                // Likely reasons for failure:
                //
                // 1. Balance oracle is out-of-date. This is kind of bad, but
                //  not necessarily catastrophic nor unrecoverable. Probably,
                //  the oracle is just lagging and will be fine if the user
                //  tries again later.
                //
                // Mitigation strategy: Revert locks & state changes (i.e. do
                // nothing else).
                //
                // 2. MPC signing failed or took too long. Need to do a bit
                //  more research to see if it is possible for the signature to
                //  still show up on chain after the promise expires.
                //
                // Mitigation strategy: Retain locks until we know the
                // signature will not be issued. Note that we can't implement
                // this strategy until we implement asset transfer for MPC
                // assets, so we IGNORE THIS CASE FOR NOW.
                //
                // TODO: Implement case 2 mitigation.
            }
        }

        self.borrow_positions.insert(&account_id, &borrow_position);
    }

    #[private]
    pub fn after_execute_next_withdrawal(&mut self, account: AccountId, amount: BorrowAssetAmount) {
        // TODO: Is this check even necessary in a #[private] function?
        require!(env::promise_results_count() == 1);

        match env::promise_result(0) {
            PromiseResult::Successful(_) => {
                // Withdrawal succeeded: remove the withdrawal request from the queue.

                // TODO: If this panics, this is BIG BAD, as it means there is
                // some way to unlock the queue while a withdrawal is in-flight.
                // So, maybe we should not *actually* panic here, but do some sort of recovery?
                let (popped_account, _) = self.withdrawal_queue.try_pop().unwrap_or_else(|| {
                    env::panic_str("Invariant violation: Withdrawal queue should have been locked.")
                });

                // This is another consistency check: that the account at the
                // head of the queue cannot change while transfers are
                // in-flight. This should be maintained by the queue itself.
                require!(
                    popped_account == account,
                    "Invariant violation: Queue shifted while locked/in-flight.",
                );
            }
            PromiseResult::Failed => {
                // Withdrawal failed: unlock the queue so they can try again.

                // This occurs when the contract does not control enough of
                // the borrow asset to fulfill the withdrawal request. That is
                // to say, it has distributed all of the funds to current
                // borrows.

                env::log_str("The withdrawal request cannot be fulfilled at this time. Please try again later.");
                self.withdrawal_queue.unlock();
                if let Some(mut supply_position) = self.supply_positions.get(&account) {
                    self.record_supply_position_borrow_asset_deposit(&mut supply_position, amount);
                    self.supply_positions.insert(&account, &supply_position);
                }
            }
        }
    }

    /// Called during liquidation process; checks whether the transfer of
    /// collateral to the liquidator was successful.
    #[private]
    pub fn after_liquidate_via_ft_transfer_call(
        &mut self,
        account_id: AccountId,
        borrow_asset_amount: BorrowAssetAmount,
    ) -> U128 {
        require!(env::promise_results_count() == 1);

        let success = matches!(env::promise_result(0), PromiseResult::Successful(_));

        let refund_to_liquidator =
            self.execute_liquidate_final(&account_id, borrow_asset_amount, success);

        refund_to_liquidator.into()
    }

    #[private]
    pub fn after_liquidate_native(
        &mut self,
        liquidator_id: AccountId,
        account_id: AccountId,
        borrow_asset_amount: BorrowAssetAmount,
    ) -> PromiseOrValue<()> {
        require!(env::promise_results_count() == 1);

        let success = matches!(env::promise_result(0), PromiseResult::Successful(_));

        let refund_to_liquidator =
            self.execute_liquidate_final(&account_id, borrow_asset_amount, success);

        if refund_to_liquidator.is_zero() {
            PromiseOrValue::Value(())
        } else {
            PromiseOrValue::Promise(
                self.configuration
                    .borrow_asset
                    .transfer(liquidator_id, refund_to_liquidator),
            )
        }
    }
}
