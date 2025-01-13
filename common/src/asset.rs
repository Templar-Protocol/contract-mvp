use std::{fmt::Display, marker::PhantomData};

use near_contract_standards::fungible_token::core::ext_ft_core;
use near_sdk::{env, ext_contract, json_types::U128, near, AccountId, NearToken, Promise};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub struct FungibleAsset<T: AssetClass> {
    #[serde(skip)]
    #[borsh(skip)]
    discriminant: PhantomData<T>,
    #[serde(flatten)]
    kind: FungibleAssetKind,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
enum FungibleAssetKind {
    Native,
    Nep141(AccountId),
}

impl<T: AssetClass> FungibleAsset<T> {
    pub fn transfer(&self, receiver_id: AccountId, amount: FungibleAssetAmount<T>) -> Promise {
        match self.kind {
            FungibleAssetKind::Native => {
                Promise::new(receiver_id).transfer(NearToken::from_yoctonear(amount.as_u128()))
            }
            FungibleAssetKind::Nep141(ref contract_id) => ext_ft_core::ext(contract_id.clone())
                .with_attached_deposit(NearToken::from_yoctonear(1))
                .ft_transfer(receiver_id, amount.as_u128().into(), None),
        }
    }

    pub fn native() -> Self {
        Self {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Native,
        }
    }

    pub fn nep141(contract_id: AccountId) -> Self {
        Self {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep141(contract_id),
        }
    }

    pub fn is_native(&self) -> bool {
        matches!(self.kind, FungibleAssetKind::Native)
    }

    pub fn is_nep141(&self, account_id: &AccountId) -> bool {
        if let FungibleAssetKind::Nep141(ref contract_id) = self.kind {
            contract_id == account_id
        } else {
            false
        }
    }

    pub fn into_nep141(self) -> Option<AccountId> {
        match self.kind {
            FungibleAssetKind::Nep141(contract_id) => Some(contract_id),
            _ => None,
        }
    }

    pub fn current_account_balance(&self, meta: Vec<u8>) -> Promise {
        let current_account_id = env::current_account_id();
        match self.kind {
            FungibleAssetKind::Native => {
                let balance = U128(env::account_balance().as_yoctonear());
                ext_fungible_asset_balance_receiver::ext(current_account_id)
                    .private_receive_fungible_asset_balance(Some(balance), meta)
            }
            FungibleAssetKind::Nep141(ref account_id) => ext_ft_core::ext(account_id.clone())
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

impl<T: AssetClass> Display for FungibleAsset<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self.kind {
                FungibleAssetKind::Native => "[native NEAR]",
                FungibleAssetKind::Nep141(ref contract_id) => contract_id.as_str(),
            }
        )
    }
}

mod sealed {
    pub trait Sealed {}
}
pub trait AssetClass: sealed::Sealed + Copy + Clone {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct CollateralAsset;
impl sealed::Sealed for CollateralAsset {}
impl AssetClass for CollateralAsset {}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
pub struct BorrowAsset;
impl sealed::Sealed for BorrowAsset {}
impl AssetClass for BorrowAsset {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
#[serde(from = "U128", into = "U128")]
pub struct FungibleAssetAmount<T: AssetClass> {
    amount: U128,
    #[borsh(skip)]
    discriminant: PhantomData<T>,
}

impl<T: AssetClass> Default for FungibleAssetAmount<T> {
    fn default() -> Self {
        Self::zero()
    }
}

impl<T: AssetClass> From<U128> for FungibleAssetAmount<T> {
    fn from(amount: U128) -> Self {
        Self {
            amount,
            discriminant: PhantomData,
        }
    }
}

impl<T: AssetClass> From<FungibleAssetAmount<T>> for U128 {
    fn from(value: FungibleAssetAmount<T>) -> Self {
        value.amount
    }
}

impl<T: AssetClass> From<u128> for FungibleAssetAmount<T> {
    fn from(value: u128) -> Self {
        Self::new(value)
    }
}

impl<T: AssetClass> FungibleAssetAmount<T> {
    pub fn new(amount: u128) -> Self {
        Self {
            amount: amount.into(),
            discriminant: PhantomData,
        }
    }

    pub fn zero() -> Self {
        Self {
            amount: 0.into(),
            discriminant: PhantomData,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.amount.0 == 0
    }

    pub fn as_u128(&self) -> u128 {
        self.amount.0
    }

    pub fn split(&mut self, amount: impl Into<Self>) -> Option<Self> {
        let a = amount.into();
        self.amount.0 = self.amount.0.checked_sub(a.amount.0)?;
        Some(a)
    }

    pub fn join(&mut self, other: Self) -> Option<()> {
        self.amount.0 = self.amount.0.checked_add(other.amount.0)?;
        Some(())
    }
}

pub type BorrowAssetAmount = FungibleAssetAmount<BorrowAsset>;
pub type CollateralAssetAmount = FungibleAssetAmount<CollateralAsset>;

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::serde_json;

    #[test]
    fn serialization() {
        let amount = BorrowAssetAmount::new(100);
        let serialized = serde_json::to_string(&amount).unwrap();
        assert_eq!(serialized, "\"100\"");
        let deserialized: BorrowAssetAmount = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, amount);
    }
}
