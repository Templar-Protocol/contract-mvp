use std::str::FromStr;

use near_contract_standards::fungible_token::Balance;
use near_sdk::{near, AccountId, AccountIdRef};

#[near(serializers = [borsh])]
pub struct Loan {
    pub collateral_amount: u128,
    pub borrowed_amount: u128,
    pub timestamp: u64,
    pub borrower: AccountId,
}

#[near(serializers = [borsh])]
pub struct Deposit {
    pub amount: u128,
    pub timestamp: u64,
    pub lender: AccountId,
}

// TODO: add vault id to struct, nft id and update all methods that use vault id
#[near(serializers = [borsh])]
pub struct Vault {
    pub collateral_asset: AccountId,
    pub stablecoin_asset: AccountId,
    pub collateral_balance: u128,
    pub stablecoin_balance: u128,
    pub loans: Vec<Loan>, // TODO: use UnorderedSet - using Vec for now for compatibility with NEAR SDK 5.3 serializer
    pub deposits: Vec<Deposit>, // TODO: use UnorderedSet - using Vec for now for compatibility with NEAR SDK 5.3 serializer
    pub min_collateral_ratio: u64,
    pub debt: Balance,
}

impl Vault {
    pub fn new(
        collateral_asset: AccountId,
        stablecoin_asset: AccountId,
        min_collateral_ratio: u64,
    ) -> (String, Self) {
        let id = construct_id(&collateral_asset, &stablecoin_asset);
        (
            id,
            Self {
                collateral_asset,
                stablecoin_asset,
                collateral_balance: 0,
                stablecoin_balance: 0,
                loans: Vec::new(),
                deposits: Vec::new(),
                min_collateral_ratio,
                debt: 0,
            },
        )
    }
}

pub fn construct_id(collateral_asset: &AccountIdRef, stablecoin_asset: &AccountIdRef) -> String {
    format!("{collateral_asset}:{stablecoin_asset}")
}

#[near(serializers = [borsh])]
pub enum Message {
    Deposit { vault_id: String },
    Collateral { vault_id: String },
}

impl std::fmt::Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deposit { vault_id } => write!(f, "deposit:{vault_id}"),
            Self::Collateral { vault_id } => write!(f, "collateral:{vault_id}"),
        }
    }
}

impl FromStr for Message {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (slug, vault_id) = s.split_once(':').ok_or(())?;
        Ok(match slug {
            "deposit" => Self::Deposit {
                vault_id: vault_id.to_string(),
            },
            "collateral" => Self::Collateral {
                vault_id: vault_id.to_string(),
            },
            _ => return Err(()),
        })
    }
}
