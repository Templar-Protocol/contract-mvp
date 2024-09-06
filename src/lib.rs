use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::collections::{LazyOption, LookupMap, UnorderedMap};
use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{
    env, near_bindgen, AccountId, BorshStorageKey, PanicOnDefault, Promise, PromiseOrValue,
    NearToken, Gas,
};

use near_contract_standards::fungible_token::Balance;
use near_contract_standards::fungible_token::receiver::FungibleTokenReceiver;
use near_contract_standards::non_fungible_token::{Token, TokenId, NonFungibleToken, NonFungibleTokenEnumeration};
use near_contract_standards::non_fungible_token::core::{NonFungibleTokenCore, NonFungibleTokenResolver};
use near_contract_standards::non_fungible_token::approval::NonFungibleTokenApproval;
use near_contract_standards::non_fungible_token::metadata::{
    NFTContractMetadata, NonFungibleTokenMetadataProvider, TokenMetadata, NFT_METADATA_SPEC,
};

use near_sdk::serde_json;


const NFT_MINT_FEE: u128 = 1_000_000_000_000_000_000_000_000; // 1 NEAR

#[derive(BorshSerialize, BorshStorageKey)]
enum StorageKey {
    NonFungibleToken,
    Metadata,
    TokenMetadata,
    Enumeration,
    Approval,
    Vaults,
    NftCollections,
    Invites,
}

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, PanicOnDefault)]
pub struct TemplarProtocol {
    vaults: LookupMap<String, Vault>,
    nft_collections: UnorderedMap<String, AccountId>,
    invites: LookupMap<String, String>,
    nft: NonFungibleToken,  // allows for easy integration w/ NFT functionality provided by near-contract-standards
    metadata: LazyOption<NFTContractMetadata>,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct Vault {
    collateral_asset: AccountId,
    stablecoin: AccountId,
    collateral_balance: u128,
    stablecoin_balance: u128,
    loans: Vec<AccountId>,  // TODO: use UnorderedSet - using Vec for now for compatibility with NEAR SDK 5.3 serializer
    min_collateral_ratio: u64,
    debt: Balance,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct Loan {
    collateral_amount: u128,
    borrowed_amount: u128,
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
        Self {
            vaults: LookupMap::new(StorageKey::Vaults),
            nft_collections: UnorderedMap::new(StorageKey::NftCollections),
            invites: LookupMap::new(StorageKey::Invites),
            nft: NonFungibleToken::new(
                StorageKey::NonFungibleToken,
                env::current_account_id(),
                Some(StorageKey::TokenMetadata),
                Some(StorageKey::Enumeration),
                Some(StorageKey::Approval),
            ),
            metadata: LazyOption::new(StorageKey::Metadata, Some(&metadata)),
        }
    }

    #[payable]
    pub fn create_vault(&mut self, nft_collection: AccountId, collateral_asset: AccountId, stablecoin: AccountId, min_collateral_ratio: u64) {
        assert!(env::attached_deposit() >= NearToken::from_yoctonear(NFT_MINT_FEE), "Not enough deposit to create vault");
        let vault_id = format!("{}:{}", collateral_asset, stablecoin);
        assert!(!self.vaults.contains_key(&vault_id), "Vault already exists");

        let vault = Vault {
            collateral_asset,
            stablecoin,
            collateral_balance: 0,
            stablecoin_balance: 0,
            loans: Vec::new(),
            min_collateral_ratio,
            debt: 0,
        };

        self.vaults.insert(&vault_id.to_string(), &vault);
        self.nft_collections.insert(&nft_collection.to_string(), &nft_collection);
    }

    #[payable]
    pub fn deposit_stablecoin(&mut self, vault_id: String, amount: U128) -> Promise {
        let vault = self.vaults.get(&vault_id).expect("Vault not found");
        let nft_collection = self.get_nft_collection_for_vault(&vault_id);
        assert!(self.owns_nft(&env::predecessor_account_id(), nft_collection.as_str()), "Not authorized");

        Promise::new(vault.stablecoin.clone()).function_call(
            "ft_transfer_call".to_string(),
            serde_json::to_vec(&serde_json::json!({
                "receiver_id": env::current_account_id(),
                "amount": amount,
                "msg": format!("deposit:{}", vault_id)
            })).unwrap(),
            NearToken::from_yoctonear(1),
            Gas::from_tgas(10),
        )
    }

    pub fn borrow(&mut self, vault_id: String, collateral_amount: U128, borrow_amount: U128) -> Promise {
        let mut vault = self.vaults.get(&vault_id).expect("Vault not found");
        let nft_collection = self.get_nft_collection_for_vault(&vault_id);
        assert!(self.owns_nft(&env::predecessor_account_id(), nft_collection.as_str()), "Not authorized");

        assert!(vault.stablecoin_balance >= borrow_amount.0, "Insufficient liquidity");

        vault.collateral_balance += collateral_amount.0;
        vault.stablecoin_balance -= borrow_amount.0;
        vault.loans.push(env::predecessor_account_id());

        self.vaults.insert(&vault_id, &vault);

        Promise::new(vault.collateral_asset.clone()).function_call(
            "ft_transfer_call".to_string(),
            serde_json::to_vec(&serde_json::json!({
                "receiver_id": env::current_account_id(),
                "amount": collateral_amount,
                "msg": format!("collateral:{}", vault_id)
            })).unwrap(),
            NearToken::from_yoctonear(1),
            Gas::from_tgas(10),
        ).then(
            Promise::new(vault.stablecoin.clone()).function_call(
                "ft_transfer".to_string(),
                serde_json::to_vec(&serde_json::json!({
                    "receiver_id": env::predecessor_account_id(),
                    "amount": borrow_amount,
                })).unwrap(),
                NearToken::from_yoctonear(1),
                Gas::from_tgas(10),
            )
        )
    }

    pub fn create_invite(&mut self, nft_collection: AccountId) -> String {
        assert!(self.nft_collections.get(&nft_collection.to_string()).is_some(), "NFT collection not found");
        let invite_code = hex::encode(env::sha256(&env::random_seed()));
        self.invites.insert(&invite_code, &nft_collection.to_string());
        invite_code
    }

    #[payable]
    pub fn mint_nft(&mut self, invite_code: String, token_metadata: TokenMetadata) -> Token {
        assert!(env::attached_deposit() >= NearToken::from_yoctonear(NFT_MINT_FEE), "Not enough deposit to mint NFT");
        let nft_collection = self.invites.get(&invite_code).expect("Invalid invite code");
        assert!(!self.owns_nft(&env::predecessor_account_id(), &nft_collection), "Already a member");

        let token_id = (self.nft.nft_total_supply().0 + 1).to_string();
        self.nft.internal_mint(token_id.clone(), env::predecessor_account_id(), Some(token_metadata));

        self.invites.remove(&invite_code);

        self.nft.nft_token(token_id).unwrap()
    }

    fn get_nft_collection_for_vault(&self, vault_id: &str) -> AccountId {
        for (_collection, account_id) in self.nft_collections.iter() {
            if self.vaults.get(&vault_id.to_string()).is_some() {
                return account_id;
            }
        }
        env::panic_str("No NFT collection found for this vault");
    }

    fn owns_nft(&self, account_id: &AccountId, nft_collection: &str) -> bool {
        self.nft_collections.get(&nft_collection.to_string()).expect("NFT collection not found") == *account_id
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

impl TemplarProtocol {
    fn handle_deposit(&mut self, vault_id: &str, sender_id: AccountId, amount: U128) -> PromiseOrValue<U128> {
        let mut vault = self.vaults.get(&vault_id.to_string()).expect("Vault not found");
        vault.stablecoin_balance += amount.0;
        self.vaults.insert(&vault_id.to_string(), &vault);
        PromiseOrValue::Value(U128(0))
    }

    fn handle_collateral(&mut self, vault_id: &str, sender_id: AccountId, amount: U128) -> PromiseOrValue<U128> {
        let mut vault = self.vaults.get(&vault_id.to_string()).expect("Vault not found");
        vault.collateral_balance += amount.0;
        self.vaults.insert(&vault_id.to_string(), &vault);
        PromiseOrValue::Value(U128(0))
    }
}

// Implement required traits
impl NonFungibleTokenCore for TemplarProtocol {
    fn nft_transfer(
        &mut self,
        receiver_id: AccountId,
        token_id: TokenId,
        approval_id: Option<u64>,
        memo: Option<String>,
    ) {
        self.nft.nft_transfer(receiver_id, token_id, approval_id, memo)
    }

    fn nft_transfer_call(
        &mut self,
        receiver_id: AccountId,
        token_id: TokenId,
        approval_id: Option<u64>,
        memo: Option<String>,
        msg: String,
    ) -> PromiseOrValue<bool> {
        self.nft.nft_transfer_call(receiver_id, token_id, approval_id, memo, msg)
    }

    fn nft_token(&self, token_id: TokenId) -> Option<Token> {
        self.nft.nft_token(token_id)
    }
}

impl NonFungibleTokenEnumeration for TemplarProtocol {
    fn nft_total_supply(&self) -> U128 {
        self.nft.nft_total_supply()
    }

    fn nft_tokens(&self, from_index: Option<U128>, limit: Option<u64>) -> Vec<Token> {
        self.nft.nft_tokens(from_index, limit)
    }

    fn nft_supply_for_owner(&self, account_id: AccountId) -> U128 {
        self.nft.nft_supply_for_owner(account_id)
    }

    fn nft_tokens_for_owner(&self, account_id: AccountId, from_index: Option<U128>, limit: Option<u64>) -> Vec<Token> {
        self.nft.nft_tokens_for_owner(account_id, from_index, limit)
    }
}

#[near_bindgen]
impl NonFungibleTokenResolver for TemplarProtocol {
    #[private]
    fn nft_resolve_transfer(
        &mut self,
        previous_owner_id: AccountId,
        receiver_id: AccountId,
        token_id: TokenId,
        approved_account_ids: Option<std::collections::HashMap<AccountId, u64>>,
    ) -> bool {
        self.nft.nft_resolve_transfer(previous_owner_id, receiver_id, token_id, approved_account_ids)
    }
}

#[near_bindgen]
impl NonFungibleTokenApproval for TemplarProtocol {
    fn nft_approve(
        &mut self,
        token_id: TokenId,
        account_id: AccountId,
        msg: Option<String>,
    ) -> Option<Promise> {
        self.nft.nft_approve(token_id, account_id, msg)
    }

    fn nft_is_approved(
        &self,
        token_id: TokenId,
        approved_account_id: AccountId,
        approval_id: Option<u64>,
    ) -> bool {
        self.nft.nft_is_approved(token_id, approved_account_id, approval_id)
    }

    fn nft_revoke(&mut self, token_id: TokenId, account_id: AccountId) {
        self.nft.nft_revoke(token_id, account_id)
    }

    fn nft_revoke_all(&mut self, token_id: TokenId) {
        self.nft.nft_revoke_all(token_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::{accounts, VMContextBuilder};
    use near_sdk::testing_env;

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
        testing_env!(context.attached_deposit(NearToken::from_yoctonear(NFT_MINT_FEE)).build());
        let mut contract = TemplarProtocol::new();

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

        context.attached_deposit(NearToken::from_yoctonear(NFT_MINT_FEE));
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