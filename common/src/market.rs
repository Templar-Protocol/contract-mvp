use near_sdk::json_types::{U128, U64};
use near_sdk::{near, AccountId};

use crate::{
    asset::FungibleAsset,
    fee::{Fee, TimeBasedFee},
    rational::Rational,
};

// #[near_sdk::ext_contract(ext_market)]
pub trait MarketExternalInterface {
    // ========================
    // MARKET GENERAL FUNCTIONS
    // ========================

    fn get_configuration(&self) -> MarketConfiguration;
    fn get_borrow_asset_metrics(&self) -> BorrowAssetMetrics;
    fn get_collateral_asset_provided(&self) -> U128;

    // TODO: Decide how to work with remote balances:

    // Option 1:
    // Balance oracle calls this function directly.
    fn report_remote_asset_balance(&mut self, address: String, asset: String, amount: U128);

    // Option 2: Balance oracle creates/maintains separate NEP-141-ish contracts that track remote
    // balances.

    fn list_borrowers(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId>;
    fn list_lenders(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId>;

    /// This function does need to retrieve a "proof-of-price" from somewhere, e.g. oracle.
    fn liquidate(&mut self, account_id: AccountId, meta: ()) -> ();

    // ==================
    // BORROWER FUNCTIONS
    // ==================

    // Required to implement NEP-141 FT token receiver to receive local fungible tokens.
    // ft_on_receive :: where msg = "repay"

    fn get_borrower_position(&self, account_id: AccountId) -> BorrowerPosition;
    /// This is just a read-only function, so we don't care about validating
    /// the provided price data.
    fn get_borrow_status(&self, account_id: AccountId, collateral_asset_price: ()) -> BorrowStatus;
    /// Works for both registered and unregistered accounts.
    fn get_deposit_address_for(&self, account_id: AccountId, collateral_asset: String) -> String;

    fn initialize_borrow(&mut self, borrow_asset_amount: U128, collateral_asset_amount: U128);
    fn borrow(&mut self, amount: U128);

    // ================
    // LENDER FUNCTIONS
    // ================
    // We assume that all borrowed assets are NEAR-local. That is to say, we
    // don't yet support lending of remote assets.

    // Required to implement NEP-141 FT token receiver to receive local fungible tokens.
    // ft_on_receive :: where msg = "provide"

    fn get_lender_position(&self, account_id: AccountId) -> LenderPosition;

    fn queue_withdrawal(&mut self, amount: U128);
    fn rescind_withrawal(&mut self);
    /// Auto-harvests yield.
    fn process_next_withdrawal(&mut self);

    fn harvest_yield(&mut self);

    // =================
    // REWARDS FUNCTIONS
    // =================
    fn withdraw_lender_rewards(&mut self, amount: U128);
    fn withdraw_liquidator_rewards(&mut self, amount: U128);
    fn withdraw_protocol_rewards(&mut self, amount: U128);
    // fn withdraw_insurance_rewards(&mut self, amount: U128);
}

pub enum BorrowStatus {
    Healthy,
    Liquidation,
}

/// available = floor((provided - used) * maximum_borrow_asset_usage_ratio)
pub struct BorrowAssetMetrics {
    pub used: U128,
    /// Available to be borrowed right now.
    pub available: U128,
    pub provided: U128,
}

#[near]
pub struct MarketConfiguration {
    pub borrow_asset: FungibleAsset,
    pub collateral_asset: FungibleAsset,
    pub balance_oracle_account_id: AccountId,
    pub minimum_collateral_ratio_per_loan: Rational<u16>,
    /// How much of the deposited principal may be lent out (up to 100%)?
    /// This is a matter of protection for the lenders.
    /// Set to 99% for starters.
    pub maximum_borrow_asset_usage_ratio: Rational<u16>,
    /// The origination fee is a one-time amount added to the principal of the
    /// loan. That is to say, the origination fee is denominated in units of
    /// the loan asset and is paid by the borrower during repayment (or
    /// liquidation).
    pub origination_fee: Fee,
    pub annual_maintenance_fee: Fee,
    pub maximum_borrow_duration: U64,
    pub minimum_borrow_amount: U128,
    pub maximum_borrow_amount: U128,
    pub withdrawal_behavior: TimeBasedFee,
    pub liquidation_spread: LiquidationSpread,
}

#[near]
pub struct LiquidationSpread {
    pub lender: U128,
    pub liquidator: U128,
    pub protocol: U128,
    // pub insurance: U128,
}

#[near]
pub struct RewardRecord {
    pub amount: U128,
    pub last_updated_block_height: U64,
}

impl RewardRecord {
    pub fn new(block_height: u64) -> Self {
        Self {
            amount: 0.into(),
            last_updated_block_height: block_height.into(),
        }
    }

    /// Returns the amount of rewards remaining
    pub fn withdraw(&mut self, amount: u128) -> Option<U128> {
        self.amount.0 = self.amount.0.checked_sub(amount)?;
        Some(self.amount)
    }

    pub fn accumulate_rewards(&mut self, new_rewards: u128, block_height: u64) {
        self.amount.0 += new_rewards;
        self.last_updated_block_height.0 = block_height;
    }
}

#[near]
pub struct LenderPosition {
    pub borrow_asset_deposited: U128,
    pub borrow_asset_rewards: RewardRecord,
    pub collateral_asset_rewards: RewardRecord,
}

impl LenderPosition {
    pub fn new(block_height: u64) -> Self {
        Self {
            borrow_asset_deposited: 0.into(),
            borrow_asset_rewards: RewardRecord::new(block_height),
            collateral_asset_rewards: RewardRecord::new(block_height),
        }
    }

    pub fn increase_deposit(&mut self, amount: u128) -> Option<U128> {
        self.borrow_asset_deposited.0 = self.borrow_asset_deposited.0.checked_add(amount)?;
        Some(self.borrow_asset_deposited)
    }

    pub fn decrease_deposit(&mut self, amount: u128) -> Option<U128> {
        self.borrow_asset_deposited.0 = self.borrow_asset_deposited.0.checked_sub(amount)?;
        Some(self.borrow_asset_deposited)
    }
}

#[derive(Default)]
#[near]
pub struct BorrowerPosition {
    pub collateral_asset_deposited: U128,
    pub borrow_asset_withdrawn: U128,
}

impl BorrowerPosition {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn increase_collateral(&mut self, amount: u128) -> Option<U128> {
        self.collateral_asset_deposited.0 =
            self.collateral_asset_deposited.0.checked_add(amount)?;
        Some(self.collateral_asset_deposited)
    }

    pub fn decrease_collateral(&mut self, amount: u128) -> Option<U128> {
        self.collateral_asset_deposited.0 =
            self.collateral_asset_deposited.0.checked_sub(amount)?;
        Some(self.collateral_asset_deposited)
    }

    pub fn withdraw(&mut self, amount: u128) -> Option<U128> {
        self.borrow_asset_withdrawn.0 = self.borrow_asset_withdrawn.0.checked_add(amount)?;
        Some(self.borrow_asset_withdrawn)
    }

    pub fn repay(&mut self, amount: u128) -> Option<U128> {
        self.borrow_asset_withdrawn.0 = self.borrow_asset_withdrawn.0.checked_sub(amount)?;
        Some(self.borrow_asset_withdrawn)
    }
}
