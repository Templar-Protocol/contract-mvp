use std::str::FromStr;

use near_sdk::{
    collections::{LazyOption, TreeMap, UnorderedMap},
    env, near, AccountId, BorshStorageKey, IntoStorageKey,
};

use crate::asset::FungibleAsset;

#[derive(BorshStorageKey)]
#[near]
enum StorageKey {
    NftGate,
    Lenders,
    Borrowers,
    TotalLoanAssetDepositedLog,
    CollateralAssetFeeDistributionLog,
}

#[near]
pub struct LenderEntry {
    pub loan_asset_deposited: u128,
    pub collateral_asset_fees_earned: u128,
    collateral_asset_fee_distribution_updated_until_block_height: u64,
}

impl LenderEntry {
    pub fn new(block_height: u64) -> Self {
        Self {
            loan_asset_deposited: 0,
            collateral_asset_fees_earned: 0,
            collateral_asset_fee_distribution_updated_until_block_height: block_height,
        }
    }
}

#[derive(Default)]
#[near]
pub struct BorrowerEntry {
    pub collateral_asset_deposited: u128,
    pub loan_asset_borrowed: u128,
}

#[near]
pub struct Vault {
    prefix: Vec<u8>,
    pub loan_asset_id: FungibleAsset,
    pub collateral_asset_id: FungibleAsset,
    pub min_collateral_ratio: (u8, u8),
    pub nft_gate: LazyOption<AccountId>,
    total_loan_asset_deposited: u128,
    total_loan_asset_borrowed: u128,
    total_collateral_asset_deposited: u128,
    lenders: UnorderedMap<AccountId, LenderEntry>,
    borrowers: UnorderedMap<AccountId, BorrowerEntry>,
    total_loan_asset_deposited_log: near_sdk::collections::TreeMap<u64, u128>,
    collateral_asset_fee_distribution_log: near_sdk::collections::TreeMap<u64, u128>,
}

impl Vault {
    pub fn new(
        prefix: impl IntoStorageKey,
        loan_asset_id: FungibleAsset,
        collateral_asset_id: FungibleAsset,
        min_collateral_ratio: (u8, u8),
        nft_gate: Option<AccountId>,
    ) -> Self {
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
            loan_asset_id,
            collateral_asset_id,
            min_collateral_ratio,
            total_loan_asset_deposited: 0,
            total_loan_asset_borrowed: 0,
            total_collateral_asset_deposited: 0,
            nft_gate: LazyOption::new(key!(NftGate), nft_gate.as_ref()),
            lenders: UnorderedMap::new(key!(Lenders)),
            borrowers: UnorderedMap::new(key!(Borrowers)),
            total_loan_asset_deposited_log: TreeMap::new(key!(TotalLoanAssetDepositedLog)),
            collateral_asset_fee_distribution_log: TreeMap::new(key!(
                CollateralAssetFeeDistributionLog
            )),
        }
    }

    pub fn get_borrower_entry(&self, borrower_id: &AccountId) -> Option<BorrowerEntry> {
        self.borrowers.get(borrower_id)
    }

    pub fn get_lender_entry(&self, lender_id: &AccountId) -> Option<LenderEntry> {
        self.lenders.get(lender_id)
    }

    fn log_total_loan_asset_deposited(&mut self, amount: u128) {
        let block_height = env::block_height();
        self.total_loan_asset_deposited_log
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
        let mut lender_entry = self
            .lenders
            .get(lender_id)
            .unwrap_or_else(|| LenderEntry::new(env::block_height()));

        lender_entry.loan_asset_deposited = lender_entry
            .loan_asset_deposited
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Loan asset deposited overflow"));

        self.lenders.insert(lender_id, &lender_entry);

        self.total_loan_asset_deposited = self
            .total_loan_asset_deposited
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Total loan asset deposited overflow"));

        self.log_total_loan_asset_deposited(self.total_loan_asset_deposited);
    }

    pub fn record_lender_withdrawal(&mut self, lender_id: &AccountId, amount: u128) {
        let mut lender_entry = self
            .lenders
            .get(lender_id)
            .unwrap_or_else(|| LenderEntry::new(env::block_height()));

        lender_entry.loan_asset_deposited = lender_entry
            .loan_asset_deposited
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Loan asset deposited underflow"));

        self.lenders.insert(lender_id, &lender_entry);

        self.total_loan_asset_deposited = self
            .total_loan_asset_deposited
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Total loan asset deposited underflow"));

        self.log_total_loan_asset_deposited(self.total_loan_asset_deposited);
    }

    pub fn record_borrower_collateral_deposit(&mut self, borrower_id: &AccountId, amount: u128) {
        let mut borrower_entry = self.borrowers.get(borrower_id).unwrap_or_default();

        borrower_entry.collateral_asset_deposited = borrower_entry
            .collateral_asset_deposited
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Borrower collateral overflow"));

        self.borrowers.insert(borrower_id, &borrower_entry);

        self.total_collateral_asset_deposited = self
            .total_collateral_asset_deposited
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Total collateral overflow"));
    }

    pub fn record_borrower_collateral_withdrawal(&mut self, borrower_id: &AccountId, amount: u128) {
        let mut borrower_entry = self.borrowers.get(borrower_id).unwrap_or_default();

        borrower_entry.collateral_asset_deposited = borrower_entry
            .collateral_asset_deposited
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Borrower collateral underflow"));

        self.borrowers.insert(borrower_id, &borrower_entry);

        self.total_collateral_asset_deposited = self
            .total_collateral_asset_deposited
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Total collateral underflow"));
    }

    pub fn record_borrower_loan_asset_borrow(&mut self, borrower_id: &AccountId, amount: u128) {
        let mut borrower_entry = self.borrowers.get(borrower_id).unwrap_or_default();

        borrower_entry.loan_asset_borrowed = borrower_entry
            .loan_asset_borrowed
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Borrower loan asset borrowed overflow"));

        self.borrowers.insert(borrower_id, &borrower_entry);

        self.total_loan_asset_borrowed = self
            .total_loan_asset_borrowed
            .checked_add(amount)
            .unwrap_or_else(|| env::panic_str("Total loan asset borrowed overflow"));
    }

    pub fn record_borrower_loan_asset_repay(&mut self, borrower_id: &AccountId, amount: u128) {
        let mut borrower_entry = self.borrowers.get(borrower_id).unwrap_or_default();

        borrower_entry.loan_asset_borrowed = borrower_entry
            .loan_asset_borrowed
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Borrower loan asset borrowed underflow"));

        self.borrowers.insert(borrower_id, &borrower_entry);

        self.total_loan_asset_borrowed = self
            .total_loan_asset_borrowed
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Total loan asset borrowed underflow"));
    }

    pub fn record_collateral_asset_fee_distribution(&mut self, amount: u128) {
        self.log_collateral_asset_fee_distribution(amount);
    }

    pub fn record_lender_fee_withdrawal(&mut self, lender_id: &AccountId, amount: u128) {
        let mut lender_entry = self
            .lenders
            .get(lender_id)
            .unwrap_or_else(|| LenderEntry::new(env::block_height()));

        lender_entry.collateral_asset_fees_earned = lender_entry
            .collateral_asset_fees_earned
            .checked_sub(amount)
            .unwrap_or_else(|| env::panic_str("Lender fee withdrawal underflow"));

        self.lenders.insert(lender_id, &lender_entry);
    }

    pub fn update_fees_earned(&mut self, lender_id: &AccountId, until_block_height: u64) {
        let Some(mut lender_entry) = self.lenders.get(lender_id) else {
            return;
        };

        let start_from_block_height = self
            .collateral_asset_fee_distribution_log
            .floor_key(&lender_entry.collateral_asset_fee_distribution_updated_until_block_height)
            .unwrap()
            - 1; // -1 because TreeMap::iter_from start is _exclusive_

        // We explicitly want to _exclude_ `until_block_height` because the
        // intended use of this method is that it will be
        // `env::block_height()`, and in this case, it would be possible for us
        // to miss some rewards if they were distributed in the same block but
        // after this function call.
        if start_from_block_height >= until_block_height {
            return;
        }

        let mut accumulated_fees_in_span = 0;
        let mut last_block_height = start_from_block_height;

        for (block_height, fees) in self
            .collateral_asset_fee_distribution_log
            .iter_from(start_from_block_height)
        {
            if block_height >= until_block_height {
                break;
            }

            let total_loan_asset_deposited_at_distribution = self
                .total_loan_asset_deposited_log
                .get(
                    &self
                        .total_loan_asset_deposited_log
                        .floor_key(&block_height)
                        .unwrap(),
                )
                .unwrap();

            // this discards fractional fees
            let portion_of_fees = fees
                .checked_mul(lender_entry.loan_asset_deposited)
                .unwrap()
                .checked_div(total_loan_asset_deposited_at_distribution)
                .unwrap();

            accumulated_fees_in_span += portion_of_fees;

            last_block_height = block_height;
        }

        lender_entry.collateral_asset_fees_earned += accumulated_fees_in_span;
        lender_entry.collateral_asset_fee_distribution_updated_until_block_height =
            last_block_height;
    }

    pub fn can_borrower_be_liquidated(
        &self,
        borrower_id: &AccountId,
        collateral_asset_multiplier: (u128, u128),
        loan_asset_multiplier: (u128, u128),
    ) -> bool {
        let Some(borrower_entry) = self.borrowers.get(borrower_id) else {
            return false;
        };

        let scaled_collateral_value = borrower_entry.collateral_asset_deposited
            * collateral_asset_multiplier.0
            * loan_asset_multiplier.1
            * self.min_collateral_ratio.1 as u128;
        let scaled_loan_value = borrower_entry.loan_asset_borrowed
            * loan_asset_multiplier.0
            * collateral_asset_multiplier.1
            * self.min_collateral_ratio.0 as u128;

        scaled_loan_value > scaled_collateral_value
    }

    pub fn record_liquidation(&mut self, borrower_id: &AccountId) {
        let mut borrower_entry = self.borrowers.get(borrower_id).unwrap_or_default();

        let liquidated_collateral = borrower_entry.collateral_asset_deposited;
        borrower_entry.collateral_asset_deposited = 0;
        let liquidated_loan = borrower_entry.loan_asset_borrowed;
        borrower_entry.loan_asset_borrowed = 0;
        // TODO: Do we distribute the liquidated collateral as fees/rewards?
        // TODO: Do we swap the liqidated funds to the loan asset?
        self.record_collateral_asset_fee_distribution(liquidated_collateral);

        self.borrowers.insert(borrower_id, &borrower_entry);

        self.total_collateral_asset_deposited = self
            .total_collateral_asset_deposited
            .checked_sub(liquidated_collateral)
            .unwrap_or_else(|| env::panic_str("Total collateral deposited underflow"));
        self.total_loan_asset_borrowed = self
            .total_loan_asset_borrowed
            .checked_sub(liquidated_loan)
            .unwrap_or_else(|| env::panic_str("Total loan asset borrowed underflow"));
    }
}

#[near(serializers = [borsh])]
pub enum Message {
    Deposit { vault_id: String },
    Collateral { vault_id: String },
}

impl std::fmt::Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deposit { vault_id } => write!(f, "deposit:{vault_id}"),
            Self::Collateral { vault_id } => write!(f, "collateral:{vault_id}"),
        }
    }
}

impl FromStr for Message {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (slug, vault_id) = s.split_once(':').ok_or(())?;
        Ok(match slug {
            "deposit" => Self::Deposit {
                vault_id: vault_id.to_string(),
            },
            "collateral" => Self::Collateral {
                vault_id: vault_id.to_string(),
            },
            _ => return Err(()),
        })
    }
}
