use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::{near_bindgen, AccountId, PanicOnDefault};

use std::collections::{HashMap, HashSet};

use crate::Loan;

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct Oracle {
    pub loans: HashMap<AccountId, Loan>,
    pub allowed_accounts: HashSet<AccountId>,
}
