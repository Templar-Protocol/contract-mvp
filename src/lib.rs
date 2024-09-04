use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::collections::LazyOption;
use near_sdk::json_types::U128;
use near_sdk::serde::{Serialize, Deserialize};
use near_sdk::store::{LookupMap, UnorderedSet};
use near_sdk::{env, ext_contract, near_bindgen, AccountId, Balance, Promise, Gas, PromiseOrValue};
use near_contract_standards::non_fungible_token::{Token, TokenId};
use near_contract_standards::non_fungible_token::metadata::{
    NFTContractMetadata, NonFungibleTokenMetadataProvider, TokenMetadata, NFT_METADATA_SPEC,
};
use near_contract_standards::non_fungible_token::NonFungibleToken;
use near_contract_standards::non_fungible_token::mint::NonFungibleTokenMint;
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;

near_sdk::setup_alloc!();

const NFT_MINT_FEE: Balance = 1_000_000_000_000_000_000_000_000; // 1 NEAR
const GAS_FOR_NFT_MINT: Gas = Gas::from_gas(50_000_000_000_000);
const GAS_FOR_FT_TRANSFER: Gas = Gas::from_gas(10_000_000_000_000);

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub struct TemplarProtocol {
    vaults: LookupMap<String, Vault>,
    nft_collections: LookupMap<String, AccountId>,
    invites: LookupMap<String, String>,
    nft: NonFungibleToken,
    metadata: LazyOption<NFTContractMetadata>,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub struct Vault {
    collateral_asset: AccountId,
    stablecoin: AccountId,
    collateral_balance: Balance,
    stablecoin_balance: Balance,
    loans: LookupMap<AccountId, Loan>,
    min_collateral_ratio: u64,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub struct Loan {
    collateral_amount: Balance,
    borrowed_amount: Balance,
}

impl Default for TemplarProtocol {
    fn default() -> Self {
        Self {
            vaults: LookupMap::new(b"v"),
            nft_collections: LookupMap::new(b"n"),
            invites: LookupMap::new(b"i"),
            nft: NonFungibleToken::new(
                b"nft".to_vec(),
                env::current_account_id(),
                Some(env::current_account_id()),
            ),
            metadata: LazyOption::new(b"m", None),
        }
    }
}

#[near_bindgen]
impl TemplarProtocol {
    #[init]
    pub fn new() -> Self {
        assert!(!env::state_exists(), "Already initialized");
        let metadata = NFTContractMetadata {
            spec: NFT_METADATA_SPEC.to_string(),
            name: "Templar Protocol NFT".to_string(),
            symbol: "TEMPLAR".to_string(),
            icon: None,
            base_uri: None,
            reference: None,
            reference_hash: None,
        };
        let mut this = Self::default();
        this.metadata.set(&metadata);
        this
    }

    #[payable]
    pub fn create_vault(&mut self, nft_collection: AccountId, collateral_asset: AccountId, stablecoin: AccountId, min_collateral_ratio: u64) {
        assert!(env::attached_deposit() >= NFT_MINT_FEE, "Not enough deposit to create vault");
        let vault_id = format!("{}:{}", collateral_asset, stablecoin);
        assert!(!self.vaults.contains_key(&vault_id), "Vault already exists");

        let vault = Vault {
            collateral_asset,
            stablecoin,
            collateral_balance: 0,
            stablecoin_balance: 0,
            loans: LookupMap::new(vault_id.as_bytes()),
            min_collateral_ratio,
        };

        self.vaults.insert(&vault_id, &vault);
        self.nft_collections.insert(&nft_collection.to_string(), &nft_collection);
    }

    #[payable]
    pub fn deposit_stablecoin(&mut self, vault_id: String, amount: U128) {
        let mut vault = self.vaults.get(&vault_id).expect("Vault not found");
        let nft_collection = self.get_nft_collection_for_vault(&vault_id);
        assert!(self.owns_nft(&env::predecessor_account_id(), &nft_collection), "Not authorized");

        ext_fungible_token::ft_transfer_call(
            env::current_account_id(),
            amount,
            None,
            "Deposit to Templar Protocol".to_string(),
            &vault.stablecoin,
            1,
            GAS_FOR_FT_TRANSFER,
        )
        .then(ext_self::callback_deposit_stablecoin(
            vault_id,
            amount,
            env::predecessor_account_id(),
            &env::current_account_id(),
            0,
            GAS_FOR_FT_TRANSFER,
        ));
    }

    pub fn borrow(&mut self, vault_id: String, collateral_amount: U128, borrow_amount: U128) {
        let mut vault = self.vaults.get(&vault_id).expect("Vault not found");
        let nft_collection = self.get_nft_collection_for_vault(&vault_id);
        assert!(self.owns_nft(&env::predecessor_account_id(), &nft_collection), "Not authorized");

        assert!(vault.stablecoin_balance >= borrow_amount.0, "Insufficient liquidity");

        let mut loan = vault.loans.get(&env::predecessor_account_id()).unwrap_or(Loan {
            collateral_amount: 0,
            borrowed_amount: 0,
        });

        let new_collateral = loan.collateral_amount + collateral_amount.0;
        let new_borrowed = loan.borrowed_amount + borrow_amount.0;

        assert!(
            new_collateral * 100 / new_borrowed >= vault.min_collateral_ratio as u128,
            "Collateral ratio too low"
        );

        loan.collateral_amount = new_collateral;
        loan.borrowed_amount = new_borrowed;

        vault.collateral_balance += collateral_amount.0;
        vault.stablecoin_balance -= borrow_amount.0;

        vault.loans.insert(&env::predecessor_account_id(), &loan);
        self.vaults.insert(&vault_id, &vault);

        ext_fungible_token::ft_transfer_call(
            env::current_account_id(),
            collateral_amount,
            None,
            "Collateral deposit to Templar Protocol".to_string(),
            &vault.collateral_asset,
            1,
            GAS_FOR_FT_TRANSFER,
        )
        .then(ext_fungible_token::ft_transfer(
            env::predecessor_account_id(),
            borrow_amount,
            None,
            &vault.stablecoin,
            1,
            GAS_FOR_FT_TRANSFER,
        ));
    }

    pub fn create_invite(&mut self, nft_collection: AccountId) -> String {
        assert!(self.nft_collections.contains_key(&nft_collection.to_string()), "NFT collection not found");
        let invite_code = env::sha256(env::random_seed().as_slice());
        let invite_code = hex::encode(&invite_code);
        self.invites.insert(&invite_code, &nft_collection.to_string());
        invite_code
    }

    #[payable]
    pub fn mint_nft(&mut self, invite_code: String, token_metadata: TokenMetadata) -> Token {
        assert!(env::attached_deposit() >= NFT_MINT_FEE, "Not enough deposit to mint NFT");
        let nft_collection = self.invites.get(&invite_code).expect("Invalid invite code");
        assert!(!self.owns_nft(&env::predecessor_account_id(), &nft_collection), "Already a member");

        let token_id = (self.nft.token_metadata_by_id.len() + 1).to_string();
        self.nft.internal_mint(token_id.clone(), env::predecessor_account_id(), Some(token_metadata));

        self.invites.remove(&invite_code);

        self.nft.token_metadata_by_id.get(&token_id).unwrap()
    }

    fn get_nft_collection_for_vault(&self, vault_id: &str) -> AccountId {
        for (collection, account_id) in self.nft_collections.iter() {
            if self.vaults.get(vault_id).is_some() {
                return account_id;
            }
        }
        env::panic_str("No NFT collection found for this vault");
    }

    fn owns_nft(&self, account_id: &AccountId, nft_collection: &str) -> bool {
        let tokens = self.nft.tokens_per_owner.get(account_id);
        tokens.is_some() && !tokens.unwrap().is_empty()
    }
}

#[near_bindgen]
impl FungibleTokenReceiver for TemplarProtocol {
    fn ft_on_transfer(&mut self, sender_id: AccountId, amount: U128, msg: String) -> PromiseOrValue<U128> {
        let parts: Vec<&str> = msg.split(':').collect();
        assert!(parts.len() == 2, "Invalid message format");
        let action = parts[0];
        let vault_id = parts[1];

        match action {
            "deposit" => self.handle_deposit(vault_id, sender_id, amount),
            "collateral" => self.handle_collateral(vault_id, sender_id, amount),
            _ => env::panic_str("Invalid action"),
        }
    }
}

#[near_bindgen]
impl NonFungibleTokenMetadataProvider for TemplarProtocol {
    fn nft_metadata(&self) -> NFTContractMetadata {
        self.metadata.get().unwrap()
    }
}

#[near_bindgen]
impl NonFungibleTokenMint for TemplarProtocol {
    #[payable]
    fn nft_mint(&mut self, token_id: TokenId, token_owner_id: AccountId, token_metadata: TokenMetadata) -> Token {
        assert!(env::attached_deposit() >= NFT_MINT_FEE, "Not enough deposit to mint NFT");
        self.nft.internal_mint(token_id, token_owner_id, Some(token_metadata))
    }
}

#[ext_contract(ext_fungible_token)]
trait FungibleToken {
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>);
    fn ft_transfer_call(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>, msg: String) -> PromiseOrValue<U128>;
}

#[ext_contract(ext_self)]
trait ExtSelf {
    fn callback_deposit_stablecoin(&mut self, vault_id: String, amount: U128, sender_id: AccountId);
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::{accounts, VMContextBuilder};
    use near_sdk::{testing_env, MockedBlockchain};

    fn get_context(predecessor_account_id: AccountId) -> VMContextBuilder {
        let mut builder = VMContextBuilder::new();
        builder
            .current_account_id(accounts(0))
            .signer_account_id(predecessor_account_id.clone())
            .predecessor_account_id(predecessor_account_id);
        builder
    }

    #[test]
    fn test_create_vault() {
        let mut context = get_context(accounts(1));
        testing_env!(context.build());
        let mut contract = TemplarProtocol::new();

        context.attached_deposit(NFT_MINT_FEE);
        testing_env!(context.build());

        contract.create_vault(accounts(2), accounts(3), accounts(4), 150);
        assert!(contract.vaults.contains_key(&format!("{}:{}", accounts(3), accounts(4))));
    }

    #[test]
    #[should_panic(expected = "Not enough deposit to create vault")]
    fn test_create_vault_without_deposit() {
        let mut context = get_context(accounts(1));
        testing_env!(context.build());
        let mut contract = TemplarProtocol::new();

        contract.create_vault(accounts(2), accounts(3), accounts(4), 150);
    }

    #[test]
    fn test_create_invite() {
        let mut context = get_context(accounts(1));
        testing_env!(context.build());
        let mut contract = TemplarProtocol::new();

        context.attached_deposit(NFT_MINT_FEE);
        testing_env!(context.build());

        contract.create_vault(accounts(2), accounts(3), accounts(4), 150);
        let invite_code = contract.create_invite(accounts(2));
        assert!(contract.invites.contains_key(&invite_code));
    }

    #[test]
    #[should_panic(expected = "NFT collection not found")]
    fn test_create_invite_invalid_collection() {
        let mut context = get_context(accounts(1));
        testing_env!(context.build());
        let mut contract = TemplarProtocol::new();

        contract.create_invite(accounts(2));
    }

    // Add more tests for other functions
}