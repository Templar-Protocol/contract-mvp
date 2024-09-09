use near_contract_standards::{
    fungible_token::{core::ext_ft_core, receiver::FungibleTokenReceiver, Balance},
    non_fungible_token::{
        approval::NonFungibleTokenApproval,
        core::{NonFungibleTokenCore, NonFungibleTokenResolver},
        metadata::{
            NFTContractMetadata, NonFungibleTokenMetadataProvider, TokenMetadata, NFT_METADATA_SPEC,
        },
        NonFungibleToken, NonFungibleTokenEnumeration, Token, TokenId,
    },
};
use near_sdk::{
    collections::{LazyOption, LookupMap, UnorderedMap},
    env,
    json_types::U128,
    near, require, AccountId, BorshStorageKey, Gas, NearToken, Promise, PromiseOrValue,
};

const NFT_MINT_FEE: NearToken = NearToken::from_near(1);

#[derive(BorshStorageKey, Debug, Clone, Copy, Hash)]
#[near(serializers = [borsh])]
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

#[near(contract_state)]
pub struct TemplarProtocol {
    vaults: LookupMap<String, Vault>,
    nft_collections: UnorderedMap<String, AccountId>,
    invites: LookupMap<String, String>,
    nft: NonFungibleToken, // allows for easy integration w/ NFT functionality provided by near-contract-standards
    metadata: LazyOption<NFTContractMetadata>,
}

#[near]
pub struct Vault {
    collateral_asset: AccountId,
    stablecoin: AccountId,
    collateral_balance: u128,
    stablecoin_balance: u128,
    loans: Vec<AccountId>, // TODO: use UnorderedSet - using Vec for now for compatibility with NEAR SDK 5.3 serializer
    min_collateral_ratio: u64,
    debt: Balance,
}

#[near]
pub struct Loan {
    collateral_amount: u128,
    borrowed_amount: u128,
}

#[near]
impl TemplarProtocol {
    #[init]
    pub fn new() -> Self {
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
    pub fn create_vault(
        &mut self,
        nft_collection: AccountId,
        collateral_asset: AccountId,
        stablecoin: AccountId,
        min_collateral_ratio: u64,
    ) -> (String, TokenId) {
        require!(
            env::attached_deposit() >= NFT_MINT_FEE,
            "Not enough deposit to create vault",
        );
        let vault_id = format!("{}:{}", collateral_asset, stablecoin);
        require!(
            !self.vaults.get(&vault_id).is_some(),
            "Vault already exists",
        );

        let vault = Vault {
            collateral_asset,
            stablecoin,
            collateral_balance: 0,
            stablecoin_balance: 0,
            loans: Vec::new(),
            min_collateral_ratio,
            debt: 0,
        };

        self.vaults.insert(&vault_id, &vault);
        self.nft_collections
            .insert(&nft_collection.to_string(), &nft_collection);

        // Mint an NFT for vault access
        let token_id = (self.nft.nft_total_supply().0 + 1).to_string();
        let token_metadata = TokenMetadata {
            title: Some(format!("Vault Access NFT: {}", vault_id)),
            description: Some("Grants access to a Templar Protocol vault".to_string()),
            media: None,
            media_hash: None,
            copies: Some(1),
            issued_at: None,
            expires_at: None,
            starts_at: None,
            updated_at: None,
            extra: None,
            reference: None,
            reference_hash: None,
        };
        self.nft.internal_mint(
            token_id.clone(),
            env::predecessor_account_id(),
            Some(token_metadata),
        );

        (vault_id, token_id)
    }

    #[payable]
    pub fn deposit_stablecoin(&mut self, vault_id: String, amount: U128) {
        let nft_collection = self.get_nft_collection_for_vault(&vault_id);
        require!(
            self.owns_nft(&env::predecessor_account_id(), &nft_collection.to_string()),
            "Not authorized",
        );

        // Instead of calling ft_transfer_call, we just log the intent
        // The actual transfer should be initiated by the user
        env::log_str(&format!("Deposit intent: {} tokens to vault {}", amount.0, vault_id));
    }

    pub fn borrow(
        &mut self,
        vault_id: String,
        collateral_amount: U128,
        borrow_amount: U128,
    ) -> Promise {
        let mut vault = self.vaults.get(&vault_id).expect("Vault not found");
        let nft_collection = self.get_nft_collection_for_vault(&vault_id);
        require!(
            self.owns_nft(&env::predecessor_account_id(), &nft_collection.to_string()),
            "Not authorized",
        );

        require!(
            vault.stablecoin_balance >= borrow_amount.0,
            "Insufficient liquidity",
        );

        vault.collateral_balance += collateral_amount.0;
        vault.stablecoin_balance -= borrow_amount.0;
        vault.loans.push(env::predecessor_account_id());

        self.vaults.insert(&vault_id, &vault);

        ext_ft_core::ext(vault.collateral_asset.clone())
            .with_attached_deposit(NearToken::from_yoctonear(1))
            .with_static_gas(Gas::from_tgas(10))
            .ft_transfer_call(
                env::current_account_id(),
                collateral_amount,
                None,
                format!("collateral:{vault_id}"),
            )
            .then(
                ext_ft_core::ext(vault.stablecoin.clone())
                    .with_attached_deposit(NearToken::from_yoctonear(1))
                    .with_static_gas(Gas::from_tgas(10))
                    .ft_transfer(env::predecessor_account_id(), borrow_amount, None),
            )
    }

    pub fn create_invite(&mut self, nft_collection: AccountId) -> String {
        require!(
            self.nft_collections
                .get(&nft_collection.to_string())
                .is_some(),
            "NFT collection not found",
        );
        let invite_code = hex::encode(env::sha256(&env::random_seed()));
        self.invites
            .insert(&invite_code, &nft_collection.to_string());
        invite_code
    }

    #[payable]
    pub fn mint_nft(&mut self, invite_code: String, token_metadata: TokenMetadata) -> Token {
        require!(
            env::attached_deposit() >= NFT_MINT_FEE,
            "Not enough deposit to mint NFT",
        );
        let nft_collection = self.invites.get(&invite_code).expect("Invalid invite code");
        require!(
            !self.owns_nft(&env::predecessor_account_id(), &nft_collection),
            "Already a member",
        );

        let token_id = (self.nft.nft_total_supply().0 + 1).to_string();
        self.nft.internal_mint(
            token_id.clone(),
            env::predecessor_account_id(),
            Some(token_metadata),
        );

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

    fn owns_nft(&self, account_id: &AccountId, _nft_collection: &str) -> bool {
        if let Some(tokens_per_owner) = &self.nft.tokens_per_owner {
            tokens_per_owner
                .get(account_id)
                .map_or(false, |tokens| !tokens.is_empty())
        } else {
            false
        }
    }
}

#[near]
impl FungibleTokenReceiver for TemplarProtocol {
    fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        let parts: Vec<&str> = msg.split(':').collect();
        require!(parts.len() == 2, "Invalid message format");
        let action = parts[0];
        let vault_id = parts[1];

        match action {
            "deposit" => self.handle_deposit(vault_id, sender_id, amount),
            "collateral" => self.handle_collateral(vault_id, sender_id, amount),
            _ => env::panic_str("Invalid action"),
        }
    }
}

#[near]
impl NonFungibleTokenMetadataProvider for TemplarProtocol {
    fn nft_metadata(&self) -> NFTContractMetadata {
        self.metadata.get().unwrap()
    }
}

impl TemplarProtocol {
    fn handle_deposit(
        &mut self,
        vault_id: &str,
        sender_id: AccountId,
        amount: U128,
    ) -> PromiseOrValue<U128> {
        let mut vault = self
            .vaults
            .get(&vault_id.to_string())
            .expect("Vault not found");
        let nft_collection = self.get_nft_collection_for_vault(vault_id);
        require!(
            self.owns_nft(&sender_id, &nft_collection.to_string()),
            "Not authorized",
        );

        vault.stablecoin_balance += amount.0;
        self.vaults.insert(&vault_id.to_string(), &vault);
        PromiseOrValue::Value(U128(0))
    }

    fn handle_collateral(
        &mut self,
        vault_id: &str,
        _sender_id: AccountId,
        amount: U128,
    ) -> PromiseOrValue<U128> {
        let mut vault = self
            .vaults
            .get(&vault_id.to_string())
            .expect("Vault not found");
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
        self.nft
            .nft_transfer(receiver_id, token_id, approval_id, memo)
    }

    fn nft_transfer_call(
        &mut self,
        receiver_id: AccountId,
        token_id: TokenId,
        approval_id: Option<u64>,
        memo: Option<String>,
        msg: String,
    ) -> PromiseOrValue<bool> {
        self.nft
            .nft_transfer_call(receiver_id, token_id, approval_id, memo, msg)
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

    fn nft_tokens_for_owner(
        &self,
        account_id: AccountId,
        from_index: Option<U128>,
        limit: Option<u64>,
    ) -> Vec<Token> {
        self.nft.nft_tokens_for_owner(account_id, from_index, limit)
    }
}

#[near]
impl NonFungibleTokenResolver for TemplarProtocol {
    #[private]
    fn nft_resolve_transfer(
        &mut self,
        previous_owner_id: AccountId,
        receiver_id: AccountId,
        token_id: TokenId,
        approved_account_ids: Option<std::collections::HashMap<AccountId, u64>>,
    ) -> bool {
        self.nft.nft_resolve_transfer(
            previous_owner_id,
            receiver_id,
            token_id,
            approved_account_ids,
        )
    }
}

#[near]
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
        self.nft
            .nft_is_approved(token_id, approved_account_id, approval_id)
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
    use near_sdk::json_types::U128;
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
        testing_env!(context.attached_deposit(NFT_MINT_FEE).build());
        let mut contract = TemplarProtocol::new();

        let (vault_id, token_id) = contract.create_vault(accounts(2), accounts(3), accounts(4), 150);
        assert!(contract.vaults.get(&vault_id).is_some());
        assert_eq!(vault_id, format!("{}:{}", accounts(3), accounts(4)));
        assert_eq!(token_id, "1");
    }

    #[test]
    #[should_panic(expected = "Not enough deposit to create vault")]
    fn test_create_vault_without_deposit() {
        let context = get_context(accounts(1));
        testing_env!(context.build());
        let mut contract = TemplarProtocol::new();

        contract.create_vault(accounts(2), accounts(3), accounts(4), 150);
    }

    #[test]
    fn test_create_invite() {
        let mut context = get_context(accounts(1));
        testing_env!(context.attached_deposit(NFT_MINT_FEE).build());
        let mut contract = TemplarProtocol::new();

        context.attached_deposit(NFT_MINT_FEE);
        testing_env!(context.build());

        contract.create_vault(accounts(2), accounts(3), accounts(4), 150);
        let invite_code = contract.create_invite(accounts(2));
        assert!(contract.invites.get(&invite_code).is_some());
    }

    #[test]
    #[should_panic(expected = "NFT collection not found")]
    fn test_create_invite_invalid_collection() {
        let context = get_context(accounts(1));
        testing_env!(context.build());
        let mut contract = TemplarProtocol::new();

        contract.create_invite(accounts(2));
    }

    #[test]
    fn test_deposit_stablecoin() {
        let mut context = get_context(accounts(1));
        testing_env!(context.attached_deposit(NFT_MINT_FEE).build());
        let mut contract = TemplarProtocol::new();

        // Create a vault first
        let (vault_id, _) = contract.create_vault(accounts(2), accounts(3), accounts(4), 150);

        // Test deposit_stablecoin
        let deposit_amount = U128(1000);
        contract.deposit_stablecoin(vault_id.clone(), deposit_amount);
        // Simulate the actual transfer
        contract.ft_on_transfer(accounts(1), deposit_amount, format!("deposit:{vault_id}"));

        // Check if the vault balance is updated
        let updated_vault = contract.vaults.get(&vault_id).unwrap();
        assert_eq!(updated_vault.stablecoin_balance, 1000);
    }

    #[test]
    fn test_borrow() {
        let mut context = get_context(accounts(1));
        testing_env!(context.attached_deposit(NFT_MINT_FEE).build());
        let mut contract = TemplarProtocol::new();

        // Create a vault first
        let (vault_id, _) = contract.create_vault(accounts(2), accounts(3), accounts(4), 150);

        // Add some liquidity to the vault
        let mut vault = contract.vaults.get(&vault_id).unwrap();
        vault.stablecoin_balance = 10000;
        vault.collateral_balance = 0;
        contract.vaults.insert(&vault_id, &vault);

        // Test borrow
        let collateral_amount = U128(1000);
        let borrow_amount = U128(500);
        let _result = contract.borrow(vault_id.clone(), collateral_amount, borrow_amount);

        // Check if the vault balances are updated
        let final_vault = contract.vaults.get(&vault_id).unwrap();
        assert_eq!(final_vault.collateral_balance, 1000);
        assert_eq!(final_vault.stablecoin_balance, 9500);
        assert_eq!(final_vault.loans.len(), 1);
        assert_eq!(final_vault.loans[0], accounts(1));
    }

    #[test]
    fn test_mint_nft() {
        let mut context = get_context(accounts(1));
        testing_env!(context.attached_deposit(NFT_MINT_FEE).build());
        let mut contract = TemplarProtocol::new();

        // Create an invite
        contract
            .nft_collections
            .insert(&accounts(2).to_string(), &accounts(2));
        let invite_code = contract.create_invite(accounts(2));

        // Test mint_nft
        let token_metadata = TokenMetadata {
            title: Some("Test NFT".to_string()),
            description: Some("A test NFT".to_string()),
            media: None,
            media_hash: None,
            copies: Some(1),
            issued_at: None,
            expires_at: None,
            starts_at: None,
            updated_at: None,
            extra: None,
            reference: None,
            reference_hash: None,
        };

        let token = contract.mint_nft(invite_code.clone(), token_metadata);

        // Check if the NFT was minted correctly
        assert_eq!(token.token_id, "1");
        assert_eq!(token.owner_id, accounts(1));

        // Check if the invite was consumed
        assert!(contract.invites.get(&invite_code).is_none());

        // Check if the user now owns an NFT
        assert!(contract.owns_nft(&accounts(1), &accounts(2).to_string()));
    }
}
