use near_sdk::{
    collections::{TreeMap, UnorderedMap},
    env, near, AccountId, BorshStorageKey, IntoStorageKey,
};
use templar_common::{
    market::{BorrowerPosition, LenderPosition, MarketConfiguration},
    rational::Rational,
};

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    Lenders,
    Borrowers,
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
    lenders: UnorderedMap<AccountId, LenderPosition>,
    borrowers: UnorderedMap<AccountId, BorrowerPosition>,
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
            lenders: UnorderedMap::new(key!(Lenders)),
            borrowers: UnorderedMap::new(key!(Borrowers)),
            borrow_asset_deposited_log: TreeMap::new(key!(TotalLoanAssetDepositedLog)),
            collateral_asset_fee_distribution_log: TreeMap::new(key!(
                CollateralAssetFeeDistributionLog
            )),
        }
    }

    pub fn get_borrower_position(&self, borrower_id: &AccountId) -> Option<BorrowerPosition> {
        self.borrowers.get(borrower_id)
    }

    pub fn get_lender_position(&self, lender_id: &AccountId) -> Option<LenderPosition> {
        self.lenders.get(lender_id)
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

    pub fn record_lender_deposit(&mut self, lender_id: &AccountId, amount: u128) {
        let mut lender_position = self
            .lenders
            .get(lender_id)
            .unwrap_or_else(|| LenderPosition::new(env::block_height()));

        lender_position
            .increase_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Loan asset deposited overflow"));

        self.lenders.insert(lender_id, &lender_position);

        self.borrow_asset_deposited = self
            .borrow_asset_deposited
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited overflow"));

        self.log_borrow_asset_deposited(self.borrow_asset_deposited);
    }

    pub fn record_lender_withdrawal(&mut self, lender_id: &AccountId, amount: u128) {
        let mut lender_position = self
            .lenders
            .get(lender_id)
            .unwrap_or_else(|| LenderPosition::new(env::block_height()));

        lender_position
            .decrease_deposit(amount)
            .unwrap_or_else(|| env::panic_str("Loan asset deposited underflow"));

        self.lenders.insert(lender_id, &lender_position);

        self.borrow_asset_deposited = self
            .borrow_asset_deposited
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset deposited underflow"));

        self.log_borrow_asset_deposited(self.borrow_asset_deposited);
    }

    pub fn record_borrower_collateral_deposit(&mut self, borrower_id: &AccountId, amount: u128) {
        let mut borrower_position = self.borrowers.get(borrower_id).unwrap_or_default();

        borrower_position
            .increase_collateral(amount)
            .unwrap_or_else(|| env::panic_str("Borrower collateral overflow"));

        self.borrowers.insert(borrower_id, &borrower_position);

        self.collateral_asset_balance = self
            .collateral_asset_balance
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Collateral asset balance overflow"));
    }

    pub fn record_borrower_collateral_withdrawal(&mut self, borrower_id: &AccountId, amount: u128) {
        let mut borrower_position = self.borrowers.get(borrower_id).unwrap_or_default();

        borrower_position
            .decrease_collateral(amount)
            .unwrap_or_else(|| env::panic_str("Borrower collateral underflow"));

        self.borrowers.insert(borrower_id, &borrower_position);

        self.collateral_asset_balance = self
            .collateral_asset_balance
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Collateral asset balance underflow"));
    }

    pub fn record_borrower_loan_asset_withdrawal(&mut self, borrower_id: &AccountId, amount: u128) {
        let mut borrower_position = self.borrowers.get(borrower_id).unwrap_or_default();

        borrower_position
            .withdraw(amount)
            .unwrap_or_else(|| env::panic_str("Borrower loan asset withdrawn overflow"));

        self.borrowers.insert(borrower_id, &borrower_position);

        self.borrow_asset_balance = self
            .borrow_asset_balance
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Borrow asset balance underflow"));
    }

    pub fn record_borrower_loan_asset_repay(&mut self, borrower_id: &AccountId, amount: u128) {
        let mut borrower_position = self.borrowers.get(borrower_id).unwrap_or_default();

        borrower_position
            .repay(amount)
            .unwrap_or_else(|| env::panic_str("Borrower loan asset borrowed underflow"));

        self.borrowers.insert(borrower_id, &borrower_position);

        self.borrow_asset_balance = self
            .borrow_asset_balance
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Total loan asset borrowed underflow"));
    }

    pub fn record_collateral_asset_fee_distribution(&mut self, amount: u128) {
        self.log_collateral_asset_fee_distribution(amount);
    }

    pub fn record_lender_collateral_rewards_withdrawal(
        &mut self,
        lender_id: &AccountId,
        amount: u128,
    ) {
        let mut lender_position = self
            .lenders
            .get(lender_id)
            .unwrap_or_else(|| LenderPosition::new(env::block_height()));

        lender_position
            .collateral_asset_rewards
            .withdraw(amount)
            .unwrap_or_else(|| env::panic_str("Lender fee withdrawal underflow"));

        self.lenders.insert(lender_id, &lender_position);
    }

    pub fn calculate_lender_rewards(
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

    pub fn can_borrower_be_liquidated(
        &self,
        borrower_id: &AccountId,
        collateral_asset_multiplier: Rational<u128>,
        borrow_asset_multiplier: Rational<u128>,
    ) -> bool {
        let Some(borrower_position) = self.borrowers.get(borrower_id) else {
            return false;
        };

        let scaled_collateral_value = borrower_position.collateral_asset_deposited.0
            * collateral_asset_multiplier.numerator()
            * borrow_asset_multiplier.denominator()
            * self
                .configuration
                .minimum_collateral_ratio_per_loan
                .denominator() as u128;
        let scaled_loan_value = borrower_position.borrow_asset_withdrawn.0
            * borrow_asset_multiplier.numerator()
            * collateral_asset_multiplier.denominator()
            * self
                .configuration
                .minimum_collateral_ratio_per_loan
                .numerator() as u128;

        scaled_loan_value > scaled_collateral_value
    }

    pub fn record_liquidation(&mut self, borrower_id: &AccountId) {
        // TODO: This function is generally wrong.

        let mut borrower_position = self.borrowers.get(borrower_id).unwrap_or_default();

        let liquidated_collateral = borrower_position.collateral_asset_deposited.0;
        borrower_position.collateral_asset_deposited.0 = 0;
        let liquidated_loan = borrower_position.borrow_asset_withdrawn.0;
        borrower_position.borrow_asset_withdrawn.0 = 0;
        // TODO: Do we distribute the liquidated collateral as fees/rewards?
        // TODO: Do we swap the liqidated funds to the loan asset?
        self.record_collateral_asset_fee_distribution(liquidated_collateral);

        self.borrowers.insert(borrower_id, &borrower_position);

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
