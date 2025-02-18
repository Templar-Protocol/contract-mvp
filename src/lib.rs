use std::{
    ops::{Deref, DerefMut},
    usize,
};

use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::{
    env,
    json_types::{U128, U64},
    near, require, serde_json, AccountId, BorshStorageKey, PanicOnDefault, Promise, PromiseError,
    PromiseOrValue, PromiseResult,
};
use templar_common::{
    asset::{BorrowAssetAmount, CollateralAssetAmount},
    borrow::{BorrowPosition, BorrowStatus},
    market::{
        BorrowAssetMetrics, LiquidateMsg, Market, MarketConfiguration, MarketExternalInterface,
        Nep141MarketDepositMessage, OraclePriceProof,
    },
    static_yield::StaticYieldRecord,
    supply::SupplyPosition,
    withdrawal_queue::{WithdrawalQueueStatus, WithdrawalRequestStatus},
};

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
enum StorageKey {
    Market,
}

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    pub market: Market,
}

#[near]
impl Contract {
    #[init]
    pub fn new(configuration: MarketConfiguration) -> Self {
        Self {
            market: Market::new(StorageKey::Market, configuration),
        }
    }
}

impl Deref for Contract {
    type Target = Market;

    fn deref(&self) -> &Self::Target {
        &self.market
    }
}

impl DerefMut for Contract {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.market
    }
}

#[near]
impl FungibleTokenReceiver for Contract {
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        let msg = near_sdk::serde_json::from_str::<Nep141MarketDepositMessage>(&msg)
            .unwrap_or_else(|_| env::panic_str("Invalid ft_on_transfer msg"));

        let asset_id = env::predecessor_account_id();

        let use_borrow_asset = || {
            if !self.configuration.borrow_asset.is_nep141(&asset_id) {
                env::panic_str("Unsupported borrow asset");
            }

            BorrowAssetAmount::new(amount.0)
        };

        let use_collateral_asset = || {
            if !self.configuration.collateral_asset.is_nep141(&asset_id) {
                env::panic_str("Unsupported collateral asset");
            }

            CollateralAssetAmount::new(amount.0)
        };

        match msg {
            Nep141MarketDepositMessage::Supply => {
                let amount = use_borrow_asset();

                let mut supply_position = self
                    .supply_positions
                    .get(&sender_id)
                    .unwrap_or_else(|| SupplyPosition::new(env::block_height()));

                self.record_supply_position_borrow_asset_deposit(&mut supply_position, amount);

                self.supply_positions.insert(&sender_id, &supply_position);

                PromiseOrValue::Value(U128(0))
            }
            Nep141MarketDepositMessage::Collateralize => {
                let amount = use_collateral_asset();

                let mut borrow_position = self
                    .borrow_positions
                    .get(&sender_id)
                    .unwrap_or_else(|| BorrowPosition::new(env::block_height()));

                // TODO: This creates a borrow record implicitly. If we
                // require a discrete "sign-up" step, we will need to add
                // checks before this function call.
                //
                // The sign-up step would only be NFT gating or something of
                // that sort, which is just an additional pre condition check.
                // -- https://github.com/Templar-Protocol/contract-mvp/pull/6#discussion_r1923871982
                self.record_borrow_position_collateral_asset_deposit(&mut borrow_position, amount);

                self.borrow_positions.insert(&sender_id, &borrow_position);

                PromiseOrValue::Value(U128(0))
            }
            Nep141MarketDepositMessage::Repay => {
                let amount = use_borrow_asset();

                if let Some(mut borrow_position) = self.borrow_positions.get(&sender_id) {
                    // TODO: This function *errors* on overpayment. Instead, add a
                    // check before and only repay the maximum, then return the excess.
                    //
                    // Due to the slightly imprecise calculation of yield and
                    // other fees, the returning of the excess should be
                    // anything >1%, for example, over the total amount
                    // borrowed + fees/interest.
                    // -- https://github.com/Templar-Protocol/contract-mvp/pull/6#discussion_r1923876327
                    self.record_borrow_position_borrow_asset_repay(&mut borrow_position, amount);

                    self.borrow_positions.insert(&sender_id, &borrow_position);
                    PromiseOrValue::Value(U128(0))
                } else {
                    // No borrow exists: just return the whole amount.
                    PromiseOrValue::Value(amount.as_u128().into())
                }
            }
            Nep141MarketDepositMessage::Liquidate(LiquidateMsg {
                account_id,
                oracle_price_proof,
            }) => {
                let amount = use_borrow_asset();

                let mut borrow_position = self
                    .borrow_positions
                    .get(&account_id)
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

                // minimum_acceptable_amount = collateral_amount * (1 - maximum_liquidator_spread) * collateral_price / borrow_price
                let minimum_acceptable_amount: BorrowAssetAmount = self
                    .configuration
                    .maximum_liquidator_spread
                    .complement()
                    .upcast::<u128>()
                    .checked_mul(oracle_price_proof.collateral_asset_price)
                    .and_then(|x| x.checked_div(oracle_price_proof.borrow_asset_price))
                    .and_then(|x| {
                        x.checked_scalar_mul(borrow_position.collateral_asset_deposit.as_u128())
                    })
                    .and_then(|x| x.ceil())
                    .unwrap() // TODO: Eliminate .unwrap()
                    .into();

                require!(
                    amount >= minimum_acceptable_amount,
                    "Too little attached to liquidate",
                );

                self.record_liquidation_lock(&mut borrow_position);

                self.borrow_positions.insert(&account_id, &borrow_position);

                PromiseOrValue::Promise(
                    self.configuration
                        .collateral_asset
                        .transfer(sender_id, borrow_position.collateral_asset_deposit)
                        .then(
                            Self::ext(env::current_account_id())
                                .after_liquidate(account_id, amount),
                        ),
                )
            }
        }
    }
}

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

    fn report_remote_asset_balance(&mut self, address: String, asset: String, amount: U128) {
        todo!()
    }

    fn list_borrows(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId> {
        let offset = offset.map_or(0, |o| o.0 as usize);
        let count = count.map_or(usize::MAX, |c| c.0 as usize);
        self.borrow_positions
            .keys()
            .skip(offset)
            .take(count)
            .collect()
    }

    fn list_supplys(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId> {
        let offset = offset.map_or(0, |o| o.0 as usize);
        let count = count.map_or(usize::MAX, |c| c.0 as usize);
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
        let Some(borrow_position) = self.borrow_positions.get(&account_id) else {
            return None;
        };

        Some(self.configuration.borrow_status(
            &borrow_position,
            oracle_price_proof,
            env::block_timestamp_ms(),
        ))
    }

    fn get_collateral_asset_deposit_address_for(
        &self,
        account_id: AccountId,
        collateral_asset: String,
    ) -> String {
        todo!()
    }

    fn initialize_borrow(&mut self, borrow_asset_amount: U128, collateral_asset_amount: U128) {
        todo!()
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
            )
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
        let Some((account_id, amount)) = self.try_lock_next_withdrawal_request().unwrap_or_else(|_| {
            env::panic_str("Could not lock withdrawal queue. The queue may be empty or a withdrawal may be in-flight.")
        }) else {
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
        self.withdrawal_queue.get_request_status(account_id)
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

        let withdrawn = supply_position.borrow_asset_yield.withdraw(amount).unwrap();
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

        let borrow_promise = if !borrow_asset_amount.is_zero() {
            Some(
                self.configuration
                    .borrow_asset
                    .transfer(predecessor.clone(), borrow_asset_amount),
            )
        } else {
            None
        };

        let collateral_promise = if !collateral_asset_amount.is_zero() {
            Some(
                self.configuration
                    .collateral_asset
                    .transfer(predecessor.clone(), collateral_asset_amount),
            )
        } else {
            None
        };

        match (borrow_promise, collateral_promise) {
            (Some(b), Some(c)) => b.and(c),
            (Some(p), _) | (_, Some(p)) => p,
            _ => env::panic_str("No yield to withdraw"),
        } // TODO: Check for success
    }
}

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
    pub fn after_liquidate(
        &mut self,
        account_id: AccountId,
        borrow_asset_amount: BorrowAssetAmount,
    ) -> U128 {
        require!(env::promise_results_count() == 1);

        let mut borrow_position = self.borrow_positions.get(&account_id).unwrap_or_else(|| {
            env::panic_str("Invariant violation: Liquidation of nonexistent position.")
        });

        match env::promise_result(0) {
            PromiseResult::Successful(_) => {
                self.record_full_liquidation(&mut borrow_position, borrow_asset_amount);
                U128(0)
            }
            PromiseResult::Failed => {
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
                U128(borrow_asset_amount.as_u128())
            }
        }
    }
}
