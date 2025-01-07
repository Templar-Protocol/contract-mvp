use std::fmt::Display;

use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{env, ext_contract, json_types::U128, near, AccountId, NearToken, Promise};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[near(serializers = [json, borsh])]
pub enum FungibleAsset {
    #[default]
    Native,
    Nep141(AccountId),
}

impl FungibleAsset {
    pub fn transfer(&self, receiver_id: AccountId, amount: u128) -> Promise {
        match self {
            FungibleAsset::Native => {
                Promise::new(receiver_id).transfer(NearToken::from_yoctonear(amount))
            }
            FungibleAsset::Nep141(ref contract_id) => ext_ft_core::ext(contract_id.clone())
                .with_attached_deposit(NearToken::from_yoctonear(1))
                .ft_transfer(receiver_id, amount.into(), None),
        }
    }

    pub fn is_native(&self) -> bool {
        matches!(self, Self::Native)
    }

    pub fn is_nep141(&self) -> bool {
        matches!(self, Self::Nep141(..))
    }

    pub fn into_nep141(self) -> Option<AccountId> {
        match self {
            Self::Nep141(contract_id) => Some(contract_id),
            _ => None,
        }
    }

    pub fn current_account_balance(&self, meta: Vec<u8>) -> Promise {
        let current_account_id = env::current_account_id();
        match self {
            FungibleAsset::Native => {
                let balance = U128(env::account_balance().as_yoctonear());
                ext_fungible_asset_balance_receiver::ext(current_account_id)
                    .private_receive_fungible_asset_balance(Some(balance), meta)
            }
            FungibleAsset::Nep141(account_id) => ext_ft_core::ext(account_id.clone())
                .ft_balance_of(current_account_id.clone())
                .then(
                    ext_fungible_asset_balance_receiver::ext(current_account_id)
                        .private_receive_fungible_asset_balance(None, meta),
                ),
        }
    }
}

/// Implementation instructions:
/// - Function MUST be annotated with `#[private]`.
/// - Asset balance MUST be parsed from `balance` argument xor single promise result as `U128`.
/// - Arguments MUST be annotated with `#[serializer(borsh)]`.
#[ext_contract(ext_fungible_asset_balance_receiver)]
pub trait FungibleAssetBalanceReceiver {
    fn private_receive_fungible_asset_balance(
        &mut self,
        #[serializer(borsh)] balance: Option<U128>,
        #[serializer(borsh)] meta: Vec<u8>,
    );
}

impl Display for FungibleAsset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Native => "[native NEAR]",
                Self::Nep141(ref contract_id) => contract_id.as_str(),
            }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[must_use]
#[near(serializers = [borsh, json])]
pub struct FungibleAssetAmount {
    amount: U128,
}

impl FungibleAssetAmount {
    pub fn new(amount: u128) -> Self {
        Self {
            amount: amount.into(),
        }
    }

    pub fn split(&mut self, amount: u128) -> Option<Self> {
        self.amount.0 = self.amount.0.checked_sub(amount)?;
        Some(Self {
            amount: amount.into(),
        })
    }

    pub fn join(&mut self, other: Self) -> Option<()> {
        self.amount.0 = self.amount.0.checked_add(other.amount.0)?;
        Some(())
    }

    pub fn transfer(self, asset: &FungibleAsset, receiver_id: AccountId) -> Promise {
        asset.transfer(receiver_id, self.amount.0)
    }
}
