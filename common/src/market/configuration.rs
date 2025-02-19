use near_sdk::{json_types::U64, near, AccountId};

use crate::{
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAsset, FungibleAsset},
    borrow::{BorrowPosition, BorrowStatus, LiquidationReason},
    fee::{Fee, TimeBasedFee},
    rational::{Fraction, Rational},
};

use super::{OraclePriceProof, YieldWeights};

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct MarketConfiguration {
    pub borrow_asset: FungibleAsset<BorrowAsset>,
    pub collateral_asset: FungibleAsset<CollateralAsset>,
    pub balance_oracle_account_id: AccountId,
    pub minimum_collateral_ratio_per_borrow: Rational<u16>,
    /// How much of the deposited principal may be lent out (up to 100%)?
    /// This is a matter of protection for supply providers.
    /// Set to 99% for starters.
    pub maximum_borrow_asset_usage_ratio: Fraction<u16>,
    /// The origination fee is a one-time amount added to the principal of the
    /// borrow. That is to say, the origination fee is denominated in units of
    /// the borrow asset and is paid by the borrowing account during repayment
    /// (or liquidation).
    pub borrow_origination_fee: Fee<BorrowAsset>,
    pub borrow_annual_maintenance_fee: Fee<BorrowAsset>,
    pub maximum_borrow_duration_ms: Option<U64>,
    pub minimum_borrow_amount: BorrowAssetAmount,
    pub maximum_borrow_amount: BorrowAssetAmount,
    pub supply_withdrawal_fee: TimeBasedFee<CollateralAsset>,
    pub yield_weights: YieldWeights,
    /// How far below market rate to accept liquidation? This is effectively the liquidator's spread.
    ///
    /// For example, if a 100USDC borrow is (under)collateralized with $110 of
    /// NEAR, a "maximum liquidator spread" of 10% would mean that a liquidator
    /// could liquidate this borrow by sending 109USDC, netting the liquidator
    /// ($110 - $100) * 10% = $1 of NEAR.
    pub maximum_liquidator_spread: Fraction<u16>,
}

impl MarketConfiguration {
    pub fn borrow_status(
        &self,
        borrow_position: &BorrowPosition,
        oracle_price_proof: OraclePriceProof,
        block_timestamp_ms: u64,
    ) -> BorrowStatus {
        if !self.is_within_minimum_collateral_ratio(borrow_position, oracle_price_proof) {
            return BorrowStatus::Liquidation(LiquidationReason::Undercollateralization);
        }

        if !self.is_within_maximum_borrow_duration(borrow_position, block_timestamp_ms) {
            return BorrowStatus::Liquidation(LiquidationReason::Expiration);
        }

        BorrowStatus::Healthy
    }

    fn is_within_maximum_borrow_duration(
        &self,
        borrow_position: &BorrowPosition,
        block_timestamp_ms: u64,
    ) -> bool {
        if let Some(U64(maximum_duration_ms)) = self.maximum_borrow_duration_ms {
            borrow_position
                .started_at_block_timestamp_ms
                .and_then(|U64(started_at_ms)| block_timestamp_ms.checked_sub(started_at_ms))
                .map_or(true, |duration_ms| duration_ms <= maximum_duration_ms)
        } else {
            true
        }
    }

    pub fn is_within_minimum_collateral_ratio(
        &self,
        borrow_position: &BorrowPosition,
        OraclePriceProof {
            collateral_asset_price,
            borrow_asset_price,
        }: OraclePriceProof,
    ) -> bool {
        let scaled_collateral_value = borrow_position.collateral_asset_deposit.as_u128()
            * collateral_asset_price.numerator()
            * borrow_asset_price.denominator()
            * u128::from(self.minimum_collateral_ratio_per_borrow.denominator());
        let scaled_borrow_value = borrow_position.get_total_borrow_asset_liability().as_u128()
            * borrow_asset_price.numerator()
            * collateral_asset_price.denominator()
            * u128::from(self.minimum_collateral_ratio_per_borrow.numerator());

        scaled_collateral_value >= scaled_borrow_value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // #[ignore = "generate sample configuration"]
    #[test]
    pub fn generate_sample_configuration() {
        println!(
            "{{\"configuration\":{}}}",
            near_sdk::serde_json::to_string(&MarketConfiguration {
                borrow_asset: FungibleAsset::nep141("usdt.fakes.testnet".parse().unwrap()),
                collateral_asset: FungibleAsset::nep141("wrap.testnet".parse().unwrap()),
                balance_oracle_account_id: "root.testnet".parse().unwrap(),
                minimum_collateral_ratio_per_borrow: Rational::new(120, 100),
                maximum_borrow_asset_usage_ratio: Fraction::new(99, 100).unwrap(),
                borrow_origination_fee: Fee::Proportional(Rational::new(1, 100)),
                borrow_annual_maintenance_fee: Fee::zero(),
                maximum_borrow_duration_ms: None,
                minimum_borrow_amount: 1.into(),
                maximum_borrow_amount: u128::MAX.into(),
                supply_withdrawal_fee: TimeBasedFee::zero(),
                yield_weights: YieldWeights::new_with_supply_weight(8)
                    .with_static("protocol".parse().unwrap(), 1)
                    .with_static("insurance".parse().unwrap(), 1),
                maximum_liquidator_spread: Fraction::new(5, 100).unwrap(),
            })
            .unwrap()
        );
    }
}
