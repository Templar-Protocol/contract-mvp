use near_sdk::{json_types::U128, near, AccountId, PanicOnDefault};
use near_sdk_contract_tools::ft::*;

#[derive(PanicOnDefault, FungibleToken)]
#[near(contract_state)]
pub struct Contract {}

#[near]
impl Contract {
    #[init]
    pub fn new(name: String, symbol: String, owner_id: AccountId, supply: U128) -> Self {
        let mut contract = Self {};

        Nep148Controller::set_metadata(&mut contract, &ContractMetadata::new(name, symbol, 24));
        Nep141Controller::mint(&mut contract, &Nep141Mint::new(supply.0, owner_id)).unwrap();

        contract
    }
}