use near_sdk::json_types::{U128, U64};
use near_sdk::{near, AccountId, PromiseOrValue};

use crate::{
    asset::FungibleAsset,
    borrow::{BorrowPosition, BorrowStatus},
    fee::{Fee, TimeBasedFee},
    lend::LendPosition,
    rational::Rational,
};

// #[near_sdk::ext_contract(ext_market)]
pub trait MarketExternalInterface {
    // ========================
    // MARKET GENERAL FUNCTIONS
    // ========================

    fn get_configuration(&self) -> MarketConfiguration;
    fn get_borrow_asset_metrics(&self) -> BorrowAssetMetrics;
    fn get_collateral_asset_balance(&self) -> U128;

    // TODO: Decide how to work with remote balances:

    // Option 1:
    // Balance oracle calls this function directly.
    fn report_remote_asset_balance(&mut self, address: String, asset: String, amount: U128);

    // Option 2: Balance oracle creates/maintains separate NEP-141-ish contracts that track remote
    // balances.

    fn list_borrows(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId>;
    fn list_lends(&self, offset: Option<U64>, count: Option<U64>) -> Vec<AccountId>;

    /// This function does need to retrieve a "proof-of-price" from somewhere, e.g. oracle.
    fn liquidate(&mut self, account_id: AccountId, meta: ()) -> ();

    // ==================
    // BORROWER FUNCTIONS
    // ==================

    // Required to implement NEP-141 FT token receiver to receive local fungible tokens.
    // ft_on_receive :: where msg = collateralize
    // ft_on_receive :: where msg = repay

    fn get_borrow_position(&self, account_id: AccountId) -> Option<BorrowPosition>;
    /// This is just a read-only function, so we don't care about validating
    /// the provided price data.
    fn get_borrow_status(
        &self,
        account_id: AccountId,
        collateral_asset_price: Rational<u128>,
        borrow_asset_price: Rational<u128>,
    ) -> Option<BorrowStatus>;
    /// Works for both registered and unregistered accounts.
    fn get_collateral_asset_deposit_address_for(
        &self,
        account_id: AccountId,
        collateral_asset: String,
    ) -> String;

    fn initialize_borrow(&mut self, borrow_asset_amount: U128, collateral_asset_amount: U128);
    fn borrow(
        &mut self,
        amount: U128,
        collateral_asset_price: Rational<u128>,
        borrow_asset_price: Rational<u128>,
    ) -> PromiseOrValue<()>;

    // ================
    // LENDER FUNCTIONS
    // ================
    // We assume that all borrowed assets are NEAR-local. That is to say, we
    // don't yet support lending of remote assets.

    // Required to implement NEP-141 FT token receiver to receive local fungible tokens.
    // ft_on_receive :: where msg = lend

    fn get_lend_position(&self, account_id: AccountId) -> Option<LendPosition>;

    fn queue_withdrawal(&mut self, amount: U128);
    fn cancel_withrawal(&mut self);
    /// Auto-harvests yield.
    fn process_next_withdrawal(&mut self);

    fn harvest_yield(&mut self);

    // =================
    // REWARDS FUNCTIONS
    // =================
    fn withdraw_lend_position_rewards(&mut self, amount: U128);
    fn withdraw_liquidator_rewards(&mut self, amount: U128);
    fn withdraw_protocol_rewards(&mut self, amount: U128);
    // fn withdraw_insurance_rewards(&mut self, amount: U128);
}

/// Borrow asset metrics are related as follows:
///
/// ```
/// available = floor(deposited * maximum_borrow_asset_usage_ratio) - used
/// used = deposited - balance
/// ```
#[derive(Clone, Debug)]
#[near]
pub struct BorrowAssetMetrics {
    pub used: U128,
    /// Available to be borrowed right now.
    pub available: U128,
    pub deposited: U128,
}

impl BorrowAssetMetrics {
    pub fn calculate(deposited: u128, balance: u128, maximum_usage_ratio: Rational<u128>) -> Self {
        assert!(deposited >= balance);

        let used = deposited - balance;

        let available = maximum_usage_ratio
            .checked_scalar_mul(deposited)
            .and_then(|x| x.floor())
            .and_then(|x| x.checked_sub(used))
            .unwrap_or(0);

        Self {
            available: available.into(),
            deposited: deposited.into(),
            used: used.into(),
        }
    }
}

#[test]
fn test_available_formula() {
    let maximum_usage_ratio = Rational::<u128>::new(90, 100);
    let deposited = 10_000_u128;
    let balance = 5_000_u128;

    let metrics = BorrowAssetMetrics::calculate(deposited, balance, maximum_usage_ratio.upcast());

    assert_eq!(metrics.available.0, 4_000);
    assert_eq!(metrics.used.0, 5_000);
    assert_eq!(metrics.deposited.0, 10_000);
}

#[derive(Clone, Debug)]
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
    pub withdrawal_fee: TimeBasedFee,
    pub liquidation_spread: LiquidationSpread,
}

#[derive(Clone, Debug)]
#[near]
pub struct LiquidationSpread {
    pub lend_position: U128,
    pub liquidator: U128,
    pub protocol: U128,
    // pub insurance: U128,
}

#[near(serializers = [borsh, json])]
pub enum Nep141MarketDepositMessage {
    Lend,
    Collateralize,
    Repay,
}
