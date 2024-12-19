use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_sdk::{
    collections::{TreeMap, UnorderedMap},
    env,
    json_types::{U128, U64},
    near, require, AccountId, BorshStorageKey, IntoStorageKey, PromiseOrValue,
};
use templar_common::{
    asset::FungibleAsset,
    borrow::{BorrowPosition, BorrowStatus},
    lend::LendPosition,
    market::{
        BorrowAssetMetrics, MarketConfiguration, MarketExternalInterface,
        Nep141MarketDepositMessage,
    },
    rational::Rational,
};

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    LendPositions,
    BorrowPositions,
    TotalLoanAssetDepositedLog,
    CollateralAssetFeeDistributionLog,
}

#[near]
pub struct Market {
    prefix: Vec<u8>,
    configuration: MarketConfiguration,
    /// There are two different balance records for the borrow asset. The
    /// current balance is `borrow_asset_balance = borrow_asset_deposited -
    /// <amount loaned out>`.
    borrow_asset_deposited: u128,
    /// The current amount of borrow asset under direct control of the market.
    borrow_asset_balance: u128,
    /// The current amount of collateral asset under direct control of the
    /// market.
    collateral_asset_balance: u128,
    lend_positions: UnorderedMap<AccountId, LendPosition>,
    borrow_positions: UnorderedMap<AccountId, BorrowPosition>,
    borrow_asset_deposited_log: TreeMap<u64, u128>,
    collateral_asset_fee_distribution_log: TreeMap<u64, u128>,
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
            lend_positions: UnorderedMap::new(key!(LendPositions)),
            borrow_positions: UnorderedMap::new(key!(BorrowPositions)),
            borrow_asset_deposited_log: TreeMap::new(key!(TotalLoanAssetDepositedLog)),
            collateral_asset_fee_distribution_log: TreeMap::new(key!(
                CollateralAssetFeeDistributionLog
            )),
        }
    }

    pub fn get_borrow_position(&self, account_id: &AccountId) -> Option<BorrowPosition> {
        self.borrow_positions.get(account_id)
    }

    pub fn get_lend_position(&self, account_id: &AccountId) -> Option<LendPosition> {
        self.lend_positions.get(account_id)
    }

    fn log_borrow_asset_deposited(&mut self, amount: u128) {
        let block_height = env::block_height();
        self.borrow_asset_deposited_log
            .insert(&block_height, &amount);
    }

    fn log_collateral_asset_fee_distribution(&mut self, amount: u128) {
        let block_height = env::block_height();
        let mut distributed_in_block = self
            .collateral_asset_fee_distribution_log
            .get(&block_height)
            .unwrap_or(0);
        distributed_in_block += amount;
        self.collateral_asset_fee_distribution_log
            .insert(&block_height, &distributed_in_block);
    }

    pub fn record_lend_position_borrow_asset_deposit(
        &mut self,
        account_id: &AccountId,
        amount: u128,
    ) {
        let mut lend_position = self
            .lend_positions
            .get(account_id)
            .unwrap_or_else(|| LendPosition::new(env::block_height()));

        lend_position
            .deposit_borrow_asset(amount)
            .unwrap_or_else(|| env::panic_str("Lend position borrow asset overflow"));

        self.lend_positions.insert(account_id, &lend_position);

        self.borrow_asset_deposited = self
            .borrow_asset_deposited
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited overflow"));

        self.log_borrow_asset_deposited(self.borrow_asset_deposited);
    }

    pub fn record_lend_position_borrow_asset_withdrawal(
        &mut self,
        account_id: &AccountId,
        amount: u128,
    ) {
        let mut lend_position = self
            .lend_positions
            .get(account_id)
            .unwrap_or_else(|| LendPosition::new(env::block_height()));

        lend_position
            .withdraw_borrow_asset(amount)
            .unwrap_or_else(|| env::panic_str("Lend position borrow asset underflow"));

        self.lend_positions.insert(account_id, &lend_position);

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
            .deposit_collateral_asset(amount)
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
            .withdraw_collateral_asset(amount)
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

    pub fn record_collateral_asset_fee_distribution(&mut self, amount: u128) {
        self.log_collateral_asset_fee_distribution(amount);
    }

    pub fn record_lend_position_collateral_rewards_withdrawal(
        &mut self,
        account_id: &AccountId,
        amount: u128,
    ) {
        let mut lend_position = self
            .lend_positions
            .get(account_id)
            .unwrap_or_else(|| LendPosition::new(env::block_height()));

        lend_position
            .collateral_asset_rewards
            .withdraw(amount)
            .unwrap_or_else(|| env::panic_str("Lender fee withdrawal underflow"));

        self.lend_positions.insert(account_id, &lend_position);
    }

    pub fn calculate_lend_position_rewards(
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
                .borrow_asset_deposited_log
                .get(
                    &self
                        .borrow_asset_deposited_log
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
                .minimum_collateral_ratio_per_loan
                .upcast(),
        )
    }

    pub fn record_liquidation(&mut self, account_id: &AccountId) {
        // TODO: This function is generally wrong.

        let mut borrow_position = self.borrow_positions.get(account_id).unwrap_or_default();

        let liquidated_collateral = borrow_position.collateral_asset_deposited.0;
        borrow_position.collateral_asset_deposited.0 = 0;
        let liquidated_loan = borrow_position.borrow_asset_liability.0;
        borrow_position.borrow_asset_liability.0 = 0;
        // TODO: Do we distribute the liquidated collateral as fees/rewards?
        // TODO: Do we swap the liqidated funds to the loan asset?
        self.record_collateral_asset_fee_distribution(liquidated_collateral);

        self.borrow_positions.insert(account_id, &borrow_position);

        // TODO: Do we actually want to decrease the balance here?
        // We still hold the collateral, it's just not "deposited", and it
        // should all be distributed to liquidity providers (lenders).
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

// #[near]
impl FungibleTokenReceiver for Market {
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
            Nep141MarketDepositMessage::Lend => {
                require!(
                    asset_id == self.configuration.borrow_asset,
                    "This market does not support lending with this asset",
                );

                self.record_lend_position_borrow_asset_deposit(&sender_id, amount.0);

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
        }
    }
}

// #[near]
impl MarketExternalInterface for Market {
    fn get_configuration(&self) -> MarketConfiguration {
        self.configuration.clone()
    }

    fn get_borrow_asset_metrics(&self) -> BorrowAssetMetrics {
        BorrowAssetMetrics::calculate(
            self.borrow_asset_deposited,
            self.borrow_asset_balance,
            self.configuration.maximum_borrow_asset_usage_ratio.upcast(),
        )
    }

    fn get_collateral_asset_balance(&self) -> U128 {
        self.collateral_asset_balance.into()
    }

    fn report_remote_asset_balance(&mut self, address: String, asset: String, amount: U128) {
        todo!()
    }

    fn list_borrows(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId> {
        let offset = offset.map_or(0, |o| o.0 as usize);
        let count = count.map_or(0, |c| c.0 as usize);
        self.borrow_positions
            .keys()
            .skip(offset)
            .take(count)
            .collect()
    }

    fn list_lends(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId> {
        let offset = offset.map_or(0, |o| o.0 as usize);
        let count = count.map_or(0, |c| c.0 as usize);
        self.lend_positions
            .keys()
            .skip(offset)
            .take(count)
            .collect()
    }

    fn liquidate(&mut self, account_id: AccountId, meta: ()) -> () {
        todo!()
    }

    fn get_borrow_position(&self, account_id: AccountId) -> Option<BorrowPosition> {
        self.borrow_positions.get(&account_id)
    }

    fn get_borrow_status(
        &self,
        account_id: AccountId,
        collateral_asset_price: Rational<u128>,
        borrow_asset_price: Rational<u128>,
    ) -> Option<BorrowStatus> {
        let Some(position) = self.borrow_positions.get(&account_id) else {
            return None;
        };

        if position.is_healthy(
            collateral_asset_price,
            borrow_asset_price,
            self.configuration
                .minimum_collateral_ratio_per_loan
                .upcast(),
        ) {
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

    fn borrow(
        &mut self,
        amount: U128,
        collateral_asset_price: Rational<u128>,
        borrow_asset_price: Rational<u128>,
    ) -> PromiseOrValue<()> {
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
            borrow_position.is_healthy(
                collateral_asset_price,
                borrow_asset_price,
                self.configuration
                    .minimum_collateral_ratio_per_loan
                    .upcast(),
            ),
            "Cannot borrow beyond MCR",
        );

        PromiseOrValue::Promise(
            self.configuration
                .borrow_asset
                .transfer(env::predecessor_account_id(), amount.0),
        )
    }

    fn get_lend_position(&self, account_id: AccountId) -> Option<LendPosition> {
        self.lend_positions.get(&account_id)
    }

    fn queue_withdrawal(&mut self, amount: U128) {
        todo!()
    }

    fn cancel_withrawal(&mut self) {
        todo!()
    }

    fn process_next_withdrawal(&mut self) {
        todo!()
    }

    fn harvest_yield(&mut self) {
        todo!()
    }

    fn withdraw_lend_position_rewards(&mut self, amount: U128) {
        todo!()
    }

    fn withdraw_liquidator_rewards(&mut self, amount: U128) {
        todo!()
    }

    fn withdraw_protocol_rewards(&mut self, amount: U128) {
        todo!()
    }
}
