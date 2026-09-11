use std::{
    fmt::{Debug, Display},
    marker::PhantomData,
};

use near_contract_standards::fungible_token::core::ext_ft_core;
#[cfg(all(not(target_arch = "wasm32"), feature = "rpc"))]
use near_primitives::action::FunctionCallAction;
use near_sdk::{
    env,
    json_types::U128,
    near,
    serde_json::{self, json},
    AccountId, AccountIdRef, Gas, NearToken, Promise,
};
use templar_primitives::number::Decimal;

use crate::panic_with_message;

/// Assets may be configuread as one of the supported asset types.
///
/// The following asset contract standards are supported:
///
/// - [NEP-141 Fungible Token (FT)](https://nomicon.io/Standards/Tokens/FungibleToken/Core)
/// - [NEP-245 Multi-Token (MT)](https://nomicon.io/Standards/Tokens/MultiToken/Core)
///
/// ---
///
/// Assets can be constructed using associated functions:
///
/// ```
/// let my_ft = FungibleAsset::<BorrowAsset>::nep141("contract_id".parse().unwrap());
/// let my_mt = FungibleAsset::<CollateralAsset>::nep245(
///     "contract_id".parse().unwrap(),
///     "token_id".to_string(),
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
pub struct FungibleAsset<T: AssetClass> {
    // Necessary because there is no clean way to use PhantomData<T> in an enum.
    // https://internals.rust-lang.org/t/type-parameter-not-used-on-enums/13342
    #[serde(skip)]
    #[borsh(skip)]
    discriminant: PhantomData<T>,
    #[serde(flatten)]
    kind: FungibleAssetKind,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [json, borsh])]
enum FungibleAssetKind {
    Nep141(AccountId),
    Nep245 {
        contract_id: AccountId,
        token_id: String,
    },
}

impl<T: AssetClass> FungibleAsset<T> {
    /// Gas for simple transfers (`ft_transfer`)
    pub const GAS_FT_TRANSFER: Gas = Gas::from_tgas(6);

    /// Gas for simple NEP-245 transfers (`mt_transfer`)
    pub const GAS_MT_TRANSFER: Gas = Gas::from_tgas(7);

    /// Gas for `transfer_call` operations (includes callback to receiver)
    /// NEP-141 `ft_transfer_call`: Transfer + receiver callback execution
    /// Needs extra gas for the receiver contract logic (e.g., market liquidation)
    pub const GAS_FT_TRANSFER_CALL: Gas = Gas::from_tgas(100);

    /// Gas for NEP-245 `mt_transfer_call` operations
    /// NEAR Intents `mt_transfer_call`: Transfer + receiver callback + collateral transfer back
    pub const GAS_MT_TRANSFER_CALL: Gas = Gas::from_tgas(150);

    #[allow(clippy::missing_panics_doc, clippy::unwrap_used)]
    pub fn transfer(&self, receiver_id: AccountId, amount: FungibleAssetAmount<T>) -> Promise {
        match self.kind {
            FungibleAssetKind::Nep141(ref contract_id) => ext_ft_core::ext(contract_id.clone())
                .with_static_gas(Self::GAS_FT_TRANSFER)
                .with_attached_deposit(NearToken::from_yoctonear(1))
                .ft_transfer(receiver_id, u128::from(amount).into(), None),
            FungibleAssetKind::Nep245 {
                ref contract_id,
                ref token_id,
            } => Promise::new(contract_id.clone()).function_call(
                "mt_transfer".to_string(),
                serde_json::to_vec(&json!({
                   "receiver_id": receiver_id,
                   "token_id": token_id,
                   "amount": amount,
                }))
                .unwrap(),
                NearToken::from_yoctonear(1),
                Self::GAS_MT_TRANSFER,
            ),
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "rpc"))]
    pub fn transfer_call_method_name(&self) -> &str {
        match self.kind {
            FungibleAssetKind::Nep141(_) => "ft_transfer_call",
            FungibleAssetKind::Nep245 { .. } => "mt_transfer_call",
        }
    }

    #[allow(clippy::missing_panics_doc, clippy::unwrap_used)]
    pub fn transfer_call(
        &self,
        receiver_id: &AccountId,
        amount: FungibleAssetAmount<T>,
        msg: Option<&str>,
    ) -> Promise {
        let msg = msg.unwrap_or_default().to_string();
        match self.kind {
            FungibleAssetKind::Nep141(ref contract_id) => ext_ft_core::ext(contract_id.clone())
                .with_static_gas(Self::GAS_FT_TRANSFER)
                .with_attached_deposit(NearToken::from_yoctonear(1))
                .ft_transfer_call(receiver_id.clone(), u128::from(amount).into(), None, msg),
            FungibleAssetKind::Nep245 {
                ref contract_id,
                ref token_id,
            } => Promise::new(contract_id.clone()).function_call(
                "mt_transfer_call".to_string(),
                serde_json::to_vec(&json!({
                   "receiver_id": receiver_id,
                   "token_id": token_id,
                   "amount": amount,
                   "msg": msg,
                }))
                .unwrap(),
                NearToken::from_yoctonear(1),
                Self::GAS_MT_TRANSFER,
            ),
        }
    }

    /// Creates a simple `ft_transfer` action (no callback).
    #[cfg(all(not(target_arch = "wasm32"), feature = "rpc"))]
    pub fn transfer_action(
        &self,
        receiver_id: &AccountId,
        amount: FungibleAssetAmount<T>,
    ) -> FunctionCallAction {
        let (method_name, args, gas) = match self.kind {
            FungibleAssetKind::Nep141(_) => (
                "ft_transfer",
                json!({
                    "receiver_id": receiver_id,
                    "amount": u128::from(amount).to_string(),
                }),
                Self::GAS_FT_TRANSFER,
            ),
            FungibleAssetKind::Nep245 { ref token_id, .. } => (
                "mt_transfer",
                json!({
                    "receiver_id": receiver_id,
                    "token_id": token_id,
                    "amount": u128::from(amount).to_string(),
                }),
                Self::GAS_MT_TRANSFER,
            ),
        };

        FunctionCallAction {
            method_name: method_name.to_string(),
            #[allow(clippy::unwrap_used)]
            args: serde_json::to_vec(&args).unwrap(),
            gas: near_primitives::gas::Gas::from_gas(gas.as_gas()),
            deposit: NearToken::from_yoctonear(1), // 1 yoctoNEAR for security
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "rpc"))]
    pub fn transfer_call_action(
        &self,
        receiver_id: &AccountId,
        amount: FungibleAssetAmount<T>,
        msg: &str,
    ) -> FunctionCallAction {
        let (args, gas) = match self.kind {
            FungibleAssetKind::Nep141(_) => (
                json!({
                    "receiver_id": receiver_id,
                    "amount": u128::from(amount).to_string(),
                    "msg": msg,
                }),
                Self::GAS_FT_TRANSFER_CALL,
            ),
            FungibleAssetKind::Nep245 { ref token_id, .. } => (
                json!({
                    "receiver_id": receiver_id,
                    "token_id": token_id,
                    "amount": u128::from(amount).to_string(),
                    "msg": msg,
                }),
                Self::GAS_MT_TRANSFER_CALL,
            ),
        };

        FunctionCallAction {
            method_name: self.transfer_call_method_name().to_string(),
            #[allow(
                clippy::unwrap_used,
                reason = "All of the types have infallible serialization"
            )]
            args: serde_json::to_vec(&args).unwrap(),
            gas: near_primitives::gas::Gas::from_gas(gas.as_gas()),
            deposit: NearToken::from_yoctonear(1),
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "rpc"))]
    pub fn balance_of_action(&self, account_id: &AccountId) -> FunctionCallAction {
        let (method_name, args) = match self.kind {
            FungibleAssetKind::Nep141(_) => (
                "ft_balance_of",
                json!({
                    "account_id": account_id,
                }),
            ),
            FungibleAssetKind::Nep245 { ref token_id, .. } => (
                "mt_balance_of",
                json!({
                    "account_id": account_id,
                    "token_id": token_id,
                }),
            ),
        };

        FunctionCallAction {
            method_name: method_name.to_string(),
            #[allow(
                clippy::unwrap_used,
                reason = "All of the types have infallible serialization"
            )]
            args: serde_json::to_vec(&args).unwrap(),
            gas: near_primitives::gas::Gas::from_teragas(3),
            deposit: NearToken::ZERO,
        }
    }

    pub fn nep141(contract_id: AccountId) -> Self {
        Self {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep141(contract_id),
        }
    }

    pub fn nep245(contract_id: AccountId, token_id: String) -> Self {
        Self {
            discriminant: PhantomData,
            kind: FungibleAssetKind::Nep245 {
                contract_id,
                token_id,
            },
        }
    }

    pub fn is_nep141(&self, account_id: &AccountId) -> bool {
        matches!(self.kind, FungibleAssetKind::Nep141(ref contract_id) if contract_id == account_id)
    }

    pub fn into_nep141(self) -> Option<AccountId> {
        match self.kind {
            FungibleAssetKind::Nep141(contract_id) => Some(contract_id),
            FungibleAssetKind::Nep245 { .. } => None,
        }
    }

    pub fn is_nep245(&self, account_id: &AccountId, token_id: &str) -> bool {
        let t = token_id;
        matches!(self.kind, FungibleAssetKind::Nep245 { ref contract_id, ref token_id } if contract_id == account_id && token_id == t)
    }

    pub fn into_nep245(self) -> Option<(AccountId, String)> {
        match self.kind {
            FungibleAssetKind::Nep245 {
                contract_id,
                token_id,
            } => Some((contract_id, token_id)),
            FungibleAssetKind::Nep141(_) => None,
        }
    }

    /// The NEP-245 token id, or `None` for an NEP-141 fungible token. Combined
    /// with [`Self::contract_id`], this identifies the token without consuming
    /// the asset or exposing its representation.
    pub fn nep245_token_id(&self) -> Option<&str> {
        match self.kind {
            FungibleAssetKind::Nep245 { ref token_id, .. } => Some(token_id),
            FungibleAssetKind::Nep141(_) => None,
        }
    }

    #[allow(clippy::missing_panics_doc, clippy::unwrap_used)]
    pub fn current_account_balance(&self) -> Promise {
        let current_account_id = env::current_account_id();
        match self.kind {
            FungibleAssetKind::Nep141(ref account_id) => {
                ext_ft_core::ext(account_id.clone()).ft_balance_of(current_account_id.clone())
            }
            FungibleAssetKind::Nep245 {
                ref contract_id,
                ref token_id,
            } => Promise::new(contract_id.clone()).function_call(
                "mt_balance_of".to_string(),
                serde_json::to_vec(&json!({
                    "account_id": current_account_id,
                    "token_id": token_id,
                }))
                .unwrap(),
                NearToken::from_millinear(0),
                Gas::from_tgas(4),
            ),
        }
    }

    pub fn coerce<U: AssetClass>(self) -> FungibleAsset<U> {
        FungibleAsset {
            discriminant: PhantomData,
            kind: self.kind,
        }
    }

    pub fn contract_id(&self) -> &AccountIdRef {
        match self.kind {
            FungibleAssetKind::Nep141(ref account_id) => account_id,
            FungibleAssetKind::Nep245 {
                ref contract_id, ..
            } => contract_id,
        }
    }
}

impl<T: AssetClass> Display for FungibleAsset<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            FungibleAssetKind::Nep141(ref contract_id) => {
                write!(f, "nep141:{contract_id}")
            }
            FungibleAssetKind::Nep245 {
                ref contract_id,
                ref token_id,
            } => write!(f, "nep245:{contract_id}:{token_id}"),
        }
    }
}

impl<T: AssetClass> std::str::FromStr for FungibleAsset<T> {
    type Err = FungibleAssetParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Use splitn to limit splits - important for NEP-245 where token_id can contain colons
        // e.g., "nep245:intents.near:nep141:btc.omft.near" should split into 3 parts max
        let parts: Vec<&str> = s.splitn(3, ':').collect();

        match parts.as_slice() {
            ["nep141", contract_id] => {
                let account_id = contract_id
                    .parse::<AccountId>()
                    .map_err(|e| FungibleAssetParseError::InvalidAccountId(e.to_string()))?;
                Ok(FungibleAsset::nep141(account_id))
            }
            ["nep245", contract_id, token_id] => {
                let account_id = contract_id
                    .parse::<AccountId>()
                    .map_err(|e| FungibleAssetParseError::InvalidAccountId(e.to_string()))?;

                if token_id.is_empty() {
                    return Err(FungibleAssetParseError::EmptyTokenId);
                }

                Ok(FungibleAsset::nep245(account_id, (*token_id).to_string()))
            }
            _ => Err(FungibleAssetParseError::InvalidFormat),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FungibleAssetParseError {
    #[error(
        "Invalid format. Expected 'nep141:<contract_id>' or 'nep245:<contract_id>:<token_id>'"
    )]
    InvalidFormat,
    #[error("Invalid account ID: {0}")]
    InvalidAccountId(String),
    #[error("Token ID cannot be empty for NEP-245 assets")]
    EmptyTokenId,
}

mod sealed {
    pub trait Sealed {}
}
pub trait AssetClass: sealed::Sealed + Copy + Clone + Send + Sync + std::fmt::Debug {}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [borsh, json])]
pub struct CollateralAsset;
impl sealed::Sealed for CollateralAsset {}
impl AssetClass for CollateralAsset {}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [borsh, json])]
pub struct BorrowAsset;
impl sealed::Sealed for BorrowAsset {}
impl AssetClass for BorrowAsset {}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [borsh, json])]
#[serde(from = "U128", into = "U128")]
#[cfg_attr(not(target_arch = "wasm32"), schemars(transparent))]
pub struct FungibleAssetAmount<T: AssetClass> {
    #[cfg_attr(not(target_arch = "wasm32"), schemars(with = "U128"))]
    amount: U128,
    #[borsh(skip)]
    discriminant: PhantomData<T>,
}

impl<T: AssetClass> Debug for FungibleAssetAmount<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}<{}>", self.amount.0, std::any::type_name::<T>())
    }
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
            amount: U128(amount),
            discriminant: PhantomData,
        }
    }

    pub const fn zero() -> Self {
        Self {
            amount: U128(0),
            discriminant: PhantomData,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.amount.0 == 0
    }

    #[must_use]
    pub fn unwrap_add(self, other: impl Into<Self>, message: &str) -> Self {
        Self {
            amount: self
                .amount
                .0
                .checked_add(other.into().amount.0)
                .unwrap_or_else(|| panic_with_message(&format!("Arithmetic overflow: {message}")))
                .into(),
            ..self
        }
    }

    #[must_use]
    pub fn saturating_add(self, other: impl Into<Self>) -> Self {
        Self {
            amount: self.amount.0.saturating_add(other.into().amount.0).into(),
            ..self
        }
    }

    #[must_use]
    pub fn checked_add(self, other: impl Into<Self>) -> Option<Self> {
        Some(Self {
            amount: self.amount.0.checked_add(other.into().amount.0)?.into(),
            ..self
        })
    }

    #[must_use]
    pub fn unwrap_sub(self, other: impl Into<Self>, message: &str) -> Self {
        Self {
            amount: self
                .amount
                .0
                .checked_sub(other.into().amount.0)
                .unwrap_or_else(|| panic_with_message(&format!("Arithmetic underflow: {message}")))
                .into(),
            ..self
        }
    }

    #[must_use]
    pub fn saturating_sub(self, other: impl Into<Self>) -> Self {
        Self {
            amount: self.amount.0.saturating_sub(other.into().amount.0).into(),
            ..self
        }
    }

    #[must_use]
    pub fn checked_sub(self, other: impl Into<Self>) -> Option<Self> {
        Some(Self {
            amount: self.amount.0.checked_sub(other.into().amount.0)?.into(),
            ..self
        })
    }
}

impl<T: AssetClass> From<FungibleAssetAmount<T>> for Decimal {
    fn from(value: FungibleAssetAmount<T>) -> Self {
        value.amount.0.into()
    }
}

impl<T: AssetClass> From<FungibleAssetAmount<T>> for u128 {
    fn from(value: FungibleAssetAmount<T>) -> Self {
        value.amount.0
    }
}

impl<T: AssetClass> std::fmt::Display for FungibleAssetAmount<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.amount.0)
    }
}

impl<T: AssetClass, R: Into<Self>> std::ops::Add<R> for FungibleAssetAmount<T> {
    type Output = Self;

    fn add(self, rhs: R) -> Self::Output {
        Self {
            amount: U128(self.amount.0 + rhs.into().amount.0),
            ..self
        }
    }
}

impl<T: AssetClass, R: Into<Self>> std::ops::AddAssign<R> for FungibleAssetAmount<T> {
    fn add_assign(&mut self, rhs: R) {
        self.amount.0 += rhs.into().amount.0;
    }
}

impl<T: AssetClass, R: Into<Self>> std::ops::Sub<R> for FungibleAssetAmount<T> {
    type Output = Self;

    fn sub(self, rhs: R) -> Self::Output {
        Self {
            amount: U128(self.amount.0 - rhs.into().amount.0),
            ..self
        }
    }
}

impl<T: AssetClass, R: Into<Self>> std::ops::SubAssign<R> for FungibleAssetAmount<T> {
    fn sub_assign(&mut self, rhs: R) {
        self.amount.0 -= rhs.into().amount.0;
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
    #[test]
    fn amount_schema_matches_serde_wire_format() {
        let schema = serde_json::to_value(schemars::schema_for!(BorrowAssetAmount))
            .expect("amount schema serializes");
        let validator = jsonschema::draft7::new(&schema).expect("amount schema is Draft 7");
        let amount = BorrowAssetAmount::new(100);
        let serialized = serde_json::to_value(amount).expect("amount serializes");

        assert_eq!(serialized, serde_json::json!("100"));
        validator
            .validate(&serialized)
            .expect("serialized amount is valid under its schema");
        assert_eq!(
            serde_json::from_value::<BorrowAssetAmount>(serialized).expect("amount deserializes"),
            amount
        );
    }

    #[test]
    fn checked_add() {
        let v = BorrowAssetAmount::new(0).checked_add(BorrowAssetAmount::new(0));
        assert_eq!(v, Some(BorrowAssetAmount::new(0)));
        let v = BorrowAssetAmount::new(0).checked_add(BorrowAssetAmount::new(100));
        assert_eq!(v, Some(BorrowAssetAmount::new(100)));
        let v = BorrowAssetAmount::new(100).checked_add(BorrowAssetAmount::new(0));
        assert_eq!(v, Some(BorrowAssetAmount::new(100)));
        let v = BorrowAssetAmount::new(100).checked_add(BorrowAssetAmount::new(100));
        assert_eq!(v, Some(BorrowAssetAmount::new(200)));
        let v = BorrowAssetAmount::new(1).checked_add(BorrowAssetAmount::new(u128::MAX));
        assert_eq!(v, None);
    }

    #[test]
    fn checked_sub() {
        let v = BorrowAssetAmount::new(0).checked_sub(BorrowAssetAmount::new(0));
        assert_eq!(v, Some(BorrowAssetAmount::new(0)));
        let v = BorrowAssetAmount::new(0).checked_sub(BorrowAssetAmount::new(100));
        assert_eq!(v, None);
        let v = BorrowAssetAmount::new(100).checked_sub(BorrowAssetAmount::new(0));
        assert_eq!(v, Some(BorrowAssetAmount::new(100)));
        let v = BorrowAssetAmount::new(100).checked_sub(BorrowAssetAmount::new(100));
        assert_eq!(v, Some(BorrowAssetAmount::new(0)));
        let v = BorrowAssetAmount::new(1).checked_sub(BorrowAssetAmount::new(u128::MAX - 33));
        assert_eq!(v, None);
    }

    #[test]
    fn saturating_add() {
        let v = BorrowAssetAmount::new(0).saturating_add(BorrowAssetAmount::new(0));
        assert_eq!(v, BorrowAssetAmount::new(0));
        let v = BorrowAssetAmount::new(0).saturating_add(BorrowAssetAmount::new(100));
        assert_eq!(v, BorrowAssetAmount::new(100));
        let v = BorrowAssetAmount::new(100).saturating_add(BorrowAssetAmount::new(0));
        assert_eq!(v, BorrowAssetAmount::new(100));
        let v = BorrowAssetAmount::new(100).saturating_add(BorrowAssetAmount::new(100));
        assert_eq!(v, BorrowAssetAmount::new(200));
        let v = BorrowAssetAmount::new(100).saturating_add(BorrowAssetAmount::new(u128::MAX - 33));
        assert_eq!(v, BorrowAssetAmount::new(u128::MAX));
    }

    #[test]
    fn saturating_sub() {
        let v = BorrowAssetAmount::new(0).saturating_sub(BorrowAssetAmount::new(0));
        assert_eq!(v, BorrowAssetAmount::new(0));
        let v = BorrowAssetAmount::new(0).saturating_sub(BorrowAssetAmount::new(100));
        assert_eq!(v, BorrowAssetAmount::new(0));
        let v = BorrowAssetAmount::new(100).saturating_sub(BorrowAssetAmount::new(0));
        assert_eq!(v, BorrowAssetAmount::new(100));
        let v = BorrowAssetAmount::new(100).saturating_sub(BorrowAssetAmount::new(100));
        assert_eq!(v, BorrowAssetAmount::new(0));
        let v = BorrowAssetAmount::new(100).saturating_sub(BorrowAssetAmount::new(u128::MAX - 33));
        assert_eq!(v, BorrowAssetAmount::new(0));
    }

    #[test]
    #[should_panic = "overflow"]
    fn overflow_unwrap_add() {
        let _ =
            BorrowAssetAmount::new(100).unwrap_add(BorrowAssetAmount::new(u128::MAX), "overflow");
    }

    #[test]
    #[should_panic = "overflow"]
    fn overflow_unwrap_sub() {
        let _ =
            BorrowAssetAmount::new(100).unwrap_sub(BorrowAssetAmount::new(u128::MAX), "overflow");
    }

    #[test]
    #[should_panic = "attempt to add with overflow"]
    fn overflow_add() {
        let _ = BorrowAssetAmount::new(u128::MAX) + BorrowAssetAmount::new(1);
    }

    #[test]
    #[should_panic = "attempt to add with overflow"]
    fn overflow_add_assign() {
        let mut v = BorrowAssetAmount::new(u128::MAX);
        v += BorrowAssetAmount::new(1);
    }

    #[test]
    #[should_panic = "attempt to subtract with overflow"]
    fn overflow_sub() {
        let _ = BorrowAssetAmount::new(0) - BorrowAssetAmount::new(1);
    }

    #[test]
    #[should_panic = "attempt to subtract with overflow"]
    fn overflow_sub_assign() {
        let mut v = BorrowAssetAmount::new(1);
        v -= BorrowAssetAmount::new(u128::MAX);
    }
}

#[derive(Clone, Debug)]
#[near(serializers = [json])]
pub enum ReturnStyle {
    Nep141FtTransferCall,
    Nep245MtTransferCall,
}

impl ReturnStyle {
    pub fn serialize(&self, amount: FungibleAssetAmount<impl AssetClass>) -> serde_json::Value {
        match self {
            Self::Nep141FtTransferCall => serde_json::json!(amount),
            Self::Nep245MtTransferCall => serde_json::json!([amount]),
        }
    }
}
