use near_sdk::{json_types::U64, near, AccountId};

use crate::{
    asset::{BorrowAsset, BorrowAssetAmount, CollateralAsset, FungibleAsset},
    borrow::BorrowPosition,
    fee::{Fee, TimeBasedFee},
    rational::Rational,
};

use super::{LiquidationSpread, OraclePriceProof};

#[derive(Clone, Debug)]
#[near(serializers = [json, borsh])]
pub struct MarketConfiguration {
    pub borrow_asset: FungibleAsset<BorrowAsset>,
    pub collateral_asset: FungibleAsset<CollateralAsset>,
    pub balance_oracle_account_id: AccountId,
    pub liquidator_account_id: AccountId,
    pub minimum_collateral_ratio_per_borrow: Rational<u16>,
    /// How much of the deposited principal may be lent out (up to 100%)?
    /// This is a matter of protection for supply providers.
    /// Set to 99% for starters.
    pub maximum_borrow_asset_usage_ratio: Rational<u16>,
    /// The origination fee is a one-time amount added to the principal of the
    /// borrow. That is to say, the origination fee is denominated in units of
    /// the borrow asset and is paid by the borrowing account during repayment
    /// (or liquidation).
    pub borrow_origination_fee: Fee<BorrowAsset>,
    pub borrow_annual_maintenance_fee: Fee<BorrowAsset>,
    pub maximum_borrow_duration: Option<U64>,
    pub minimum_borrow_amount: BorrowAssetAmount,
    pub maximum_borrow_amount: BorrowAssetAmount,
    pub withdrawal_fee: TimeBasedFee<CollateralAsset>,
    pub liquidation_spread: LiquidationSpread,
    // TODO: how much below market rate to accept? For liquidator spread.
}

impl MarketConfiguration {
    pub fn is_healthy(
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
            * self.minimum_collateral_ratio_per_borrow.denominator() as u128;
        let scaled_borrow_value = borrow_position.get_total_borrow_asset_liability().as_u128()
            * borrow_asset_price.numerator()
            * collateral_asset_price.denominator()
            * self.minimum_collateral_ratio_per_borrow.numerator() as u128;

        scaled_collateral_value >= scaled_borrow_value
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        asset::FungibleAsset,
        fee::{Fee, TimeBasedFee, TimeBasedFeeFunction},
        market::{LiquidationSpread, MarketConfiguration},
        rational::Rational,
    };

    // {"configuration":{"borrow_asset":{"Nep141":"usdt.fakes.testnet"},"collateral_asset":{"Nep141":"wrap.testnet"},"balance_oracle_account_id":"root.testnet","liquidator_account_id":"templar-in-training.testnet","minimum_collateral_ratio_per_borrow":[6,5],"maximum_borrow_asset_usage_ratio":[99,100],"origination_fee":{"Proportional":[1,100]},"annual_maintenance_fee":{"Flat":"0"},"maximum_borrow_duration":null,"minimum_borrow_amount":"1","maximum_borrow_amount":"340282366920938463463374607431768211455","withdrawal_fee":{"fee":{"Flat":"0"},"duration":"0","behavior":"Fixed"},"liquidation_spread":{"supply_position":"6","liquidator":"1","protocol":"1"}}}

    // #[ignore = "generate sample configuration"]
    #[test]
    pub fn generate_sample_configuration() {
        println!(
            "{{\"configuration\":{}}}",
            near_sdk::serde_json::to_string(&MarketConfiguration {
                borrow_asset: FungibleAsset::nep141("usdt.fakes.testnet".parse().unwrap()),
                collateral_asset: FungibleAsset::nep141("wrap.testnet".parse().unwrap()),
                balance_oracle_account_id: "root.testnet".parse().unwrap(),
                liquidator_account_id: "templar-in-training.testnet".parse().unwrap(),
                minimum_collateral_ratio_per_borrow: Rational::new(120, 100),
                maximum_borrow_asset_usage_ratio: Rational::new(99, 100),
                borrow_origination_fee: Fee::Proportional(Rational::new(1, 100)),
                borrow_annual_maintenance_fee: Fee::Flat(0.into()),
                maximum_borrow_duration: None,
                minimum_borrow_amount: 1.into(),
                maximum_borrow_amount: u128::MAX.into(),
                withdrawal_fee: TimeBasedFee {
                    fee: Fee::Flat(0.into()),
                    duration: 0.into(),
                    behavior: TimeBasedFeeFunction::Fixed,
                },
                liquidation_spread: LiquidationSpread {
                    supply_position: 6.into(),
                    liquidator: 1.into(),
                    protocol: 1.into(),
                },
            })
            .unwrap()
        );
    }
}
