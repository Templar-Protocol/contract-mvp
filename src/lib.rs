use std::{
    ops::{Deref, DerefMut},
    usize,
};

use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::{
    env,
    json_types::{U128, U64},
    near, require, serde_json, AccountId, BorshStorageKey, PanicOnDefault, PromiseError,
    PromiseOrValue,
};
use templar_common::{
    asset::FungibleAsset,
    borrow::{BorrowPosition, BorrowStatus},
    market::{
        BorrowAssetMetrics, LiquidateMsg, Market, MarketConfiguration, MarketExternalInterface,
        Nep141MarketDepositMessage, OraclePriceProof,
    },
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

        let asset_id = FungibleAsset::Nep141(env::predecessor_account_id());

        match msg {
            Nep141MarketDepositMessage::Supply => {
                require!(
                    asset_id == self.configuration.borrow_asset,
                    "This market does not support supplying with this asset",
                );

                self.record_supply_position_borrow_asset_deposit(&sender_id, amount.0);

                PromiseOrValue::Value(U128(0))
            }
            Nep141MarketDepositMessage::Collateralize => {
                require!(
                    asset_id == self.configuration.collateral_asset,
                    "This market does not support collateralization with this asset",
                );

                // TODO: This creates a borrow record implicitly. If we
                // require a discrete "sign-up" step, we will need to add
                // checks before this function call.
                self.record_borrow_position_collateral_asset_deposit(&sender_id, amount.0);

                PromiseOrValue::Value(U128(0))
            }
            Nep141MarketDepositMessage::Repay => {
                require!(
                    asset_id == self.configuration.borrow_asset,
                    "This market does not support repayment with this asset",
                );

                // TODO: This function *errors* on overpayment. Instead, add a
                // check before and only repay the maximum, then return the excess.
                self.record_borrow_position_borrow_asset_repay(&sender_id, amount.0);

                PromiseOrValue::Value(U128(0))
            }
            Nep141MarketDepositMessage::Liquidate(LiquidateMsg {
                account_id,
                oracle_price_proof,
            }) => {
                require!(
                    asset_id == self.configuration.borrow_asset,
                    "This market does not support liquidation with this asset",
                );
                require!(
                    sender_id == self.configuration.liquidator_account_id,
                    "Account not authorized to perform liquidations",
                );

                let borrow_position = self
                    .market
                    .get_borrow_position(&account_id)
                    .unwrap_or_default();

                require!(
                    !self
                        .configuration
                        .is_healthy(&borrow_position, oracle_price_proof),
                    "Borrow position cannot be liquidated at this price",
                );

                // TODO: Do we need to check the value of the amount recovered?
                // We have the price data available in `oracle_price_proof`...
                self.record_full_liquidation(&account_id, amount.0);

                // TODO: (cont'd from above) This would allow us to calculate
                // the amount that "should" be recovered and refund the
                // liquidator any excess.
                PromiseOrValue::Value(U128(0))
            }
        }
    }
}

#[near]
impl MarketExternalInterface for Contract {
    fn get_configuration(&self) -> MarketConfiguration {
        self.configuration.clone()
    }

    fn get_borrow_asset_metrics(&self, borrow_asset_balance: U128) -> BorrowAssetMetrics {
        BorrowAssetMetrics::calculate(
            self.borrow_asset_deposited,
            borrow_asset_balance.0,
            self.configuration.maximum_borrow_asset_usage_ratio.upcast(),
        )
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

        if self
            .configuration
            .is_healthy(&borrow_position, oracle_price_proof)
        {
            Some(BorrowStatus::Healthy)
        } else {
            Some(BorrowStatus::Liquidation)
        }
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

    fn borrow(&mut self, amount: U128, oracle_price_proof: OraclePriceProof) -> PromiseOrValue<()> {
        require!(amount.0 > 0, "Borrow amount must be greater than zero");

        let account_id = env::predecessor_account_id();

        // Apply origination fee during borrow by increasing liability during repayment.
        // liable amount = amount to borrow + fee
        let liable_amount = self
            .configuration
            .origination_fee
            .of(amount.0)
            .and_then(|fee| amount.0.checked_add(fee))
            .unwrap_or_else(|| env::panic_str("Fee calculation failed"));

        let borrow_position = self.record_borrow_position_borrow_asset_withdrawal(
            &account_id,
            liable_amount,
            amount.0,
        );

        require!(
            self.configuration
                .is_healthy(&borrow_position, oracle_price_proof),
            "Cannot borrow beyond MCR",
        );

        PromiseOrValue::Promise(
            self.configuration
                .borrow_asset
                .transfer(env::predecessor_account_id(), amount.0)
                .then(Self::ext(env::current_account_id()).return_static(serde_json::Value::Null)),
        )
    }

    fn get_supply_position(&self, account_id: AccountId) -> Option<SupplyPosition> {
        self.supply_positions.get(&account_id)
    }

    /// If the predecessor has already entered the queue, calling this function
    /// will reset the position to the back of the queue.
    fn create_supply_withdrawal_request(&mut self, amount: U128) {
        require!(amount.0 > 0, "Amount to withdraw must be greater than zero");
        let predecessor = env::predecessor_account_id();
        if self
            .supply_positions
            .get(&predecessor)
            .filter(|supply_position| supply_position.borrow_asset_deposited.0 > 0)
            .is_none()
        {
            env::panic_str("Supply position does not exist");
        }

        // TODO: Check that amount is a sane value? i.e. within the amount actually deposited?
        // Probably not, since this should be checked during the actual execution of the withdrawal.
        // No sense duplicating the check, probably.
        self.withdrawal_queue.remove(&predecessor);
        self.withdrawal_queue
            .insert_or_update(&predecessor, amount.0);
    }

    fn cancel_supply_withdrawal_request(&mut self) {
        self.withdrawal_queue.remove(&env::predecessor_account_id());
    }

    fn execute_next_supply_withdrawal_request(&mut self) -> PromiseOrValue<()> {
        let Some((account_id, requested_amount)) = self.withdrawal_queue.try_lock() else {
            env::panic_str("Could not lock withdrawal queue. The queue may be empty or a withdrawal may be in-flight.")
        };

        let Some((amount, mut supply_position)) =
            self.supply_positions
                .get(&account_id)
                .and_then(|supply_position| {
                    let amount = supply_position
                        .borrow_asset_deposited
                        .0
                        .min(requested_amount);
                    if amount > 0 {
                        Some((amount, supply_position))
                    } else {
                        None
                    }
                })
        else {
            env::log_str("Supply position does not exist: skipping.");
            self.withdrawal_queue
                .try_pop()
                .unwrap_or_else(|| env::panic_str("Inconsistent state")); // we just locked the queue
            return PromiseOrValue::Value(());
        };

        supply_position
            .withdraw_borrow_asset(amount)
            .unwrap_or_else(|| env::panic_str("Inconsistent state"));
        self.supply_positions.insert(&account_id, &supply_position);

        self.record_supply_position_borrow_asset_withdrawal(&account_id, amount);

        PromiseOrValue::Promise(
            self.configuration
                .borrow_asset
                .transfer(account_id.clone(), amount)
                .then(
                    Self::ext(env::current_account_id())
                        .after_execute_next_withdrawal(account_id, amount),
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
        todo!()
    }

    fn withdraw_supply_position_rewards(&mut self, amount: U128) {
        todo!()
    }

    fn withdraw_liquidator_rewards(&mut self, amount: U128) {
        todo!()
    }

    fn withdraw_protocol_rewards(&mut self, amount: U128) {
        todo!()
    }
}

#[near]
impl Contract {
    #[private]
    pub fn return_static(&self, value: serde_json::Value) -> serde_json::Value {
        value
    }

    #[private]
    pub fn after_execute_next_withdrawal(
        &mut self,
        account: AccountId,
        amount: u128,
        #[callback_result] result: Result<Vec<u8>, PromiseError>,
    ) {
        match result {
            Ok(_) => {
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
            Err(_) => {
                // Withdrawal failed: unlock the queue so they can try again.

                // This occurs when the contract does not control enough of
                // the borrow asset to fulfill the withdrawal request. That is
                // to say, it has distributed all of the funds to current
                // borrows.

                env::log_str("The withdrawal request cannot be fulfilled at this time. Please try again later.");
                self.withdrawal_queue.unlock();
                if let Some(mut supply_position) = self.supply_positions.get(&account) {
                    supply_position.deposit_borrow_asset(amount);
                    self.supply_positions.insert(&account, &supply_position);
                }
            }
        }
    }
}
