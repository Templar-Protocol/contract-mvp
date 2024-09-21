use std::fmt::Display;

use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{near, AccountId, NearToken, Promise};

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
            FungibleAsset::Nep141(ref contract_id) => {
                ext_ft_core::ext(contract_id.clone()).ft_transfer(receiver_id, amount.into(), None)
            }
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
