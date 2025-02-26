#![allow(clippy::needless_pass_by_value)]

use std::ops::{Deref, DerefMut};

use near_sdk::{env, json_types::U128, near, BorshStorageKey, PanicOnDefault};
use templar_common::{
    asset::ReturnNativeBalance,
    market::{Market, MarketConfiguration},
};

#[derive(BorshStorageKey)]
#[near(serializers = [borsh])]
enum StorageKey {
    Market,
}

#[derive(PanicOnDefault)]
#[near(contract_state)]
pub struct Contract {
    pub market: Market,
}

#[near]
impl Contract {
    #[init]
    pub fn new(configuration: MarketConfiguration) -> Self {
        Self {
            market: Market::new(StorageKey::Market, configuration),
        }
    }
}

impl Deref for Contract {
    type Target = Market;

    fn deref(&self) -> &Self::Target {
        &self.market
    }
}

impl DerefMut for Contract {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.market
    }
}

#[near]
impl ReturnNativeBalance for Contract {
    fn return_native_balance(&self) -> U128 {
        U128(env::account_balance().as_yoctonear())
    }
}

mod impl_ft_receiver;
mod impl_helper;
mod impl_market_external;
