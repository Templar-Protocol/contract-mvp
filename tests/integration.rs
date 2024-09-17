use contract_mvp::{NFT_MINT_FEE, vault::Vault};
use near_sdk::{AccountId, NearToken};
use near_workspaces::{
    network::Sandbox, prelude::TopLevelAccountCreator, Account, Contract, DevNetwork, Worker,
};
use serde_json::{json, Value};

// ===== UTIL FUNCTIONS =====

async fn create_prefixed_account<T: DevNetwork + TopLevelAccountCreator + 'static>(
    prefix: &str,
    worker: &near_workspaces::Worker<T>,
) -> Account {
    let (genid, sk) = worker.dev_generate().await;
    let new_id: AccountId = format!("{prefix}{}", &genid.as_str()[prefix.len()..])
        .parse()
        .unwrap();
    worker.create_tla(new_id, sk).await.unwrap().unwrap()
}

macro_rules! accounts {
    ($w: ident, $($n:ident),*) => {
        let ($($n,)*) = tokio::join!( $(create_prefixed_account(stringify!($n), &$w)),* );
    };
}

async fn setup() -> (Worker<Sandbox>, Contract) {
    let sandbox = near_workspaces::sandbox().await.unwrap();
    let contract_wasm = near_workspaces::compile_project("./").await.unwrap();

    let contract = sandbox.dev_deploy(&contract_wasm).await.unwrap();
    contract
        .call("new")
        .args_json(json!({}))
        .transact()
        .await
        .unwrap()
        .unwrap();

    (sandbox, contract)
}

// ===== TESTS =====

#[tokio::test]
async fn test_create_vault() {
    let (sandbox, contract) = setup().await;
    accounts!(sandbox, user, nft_collection, collateral_asset, stablecoin);

    let (vault_id, token_id) = user
        .call(contract.id(), "create_vault")
        .args_json(json!({
            "nft_collection": nft_collection.id(),
            "collateral_asset": collateral_asset.id(),
            "stablecoin": stablecoin.id(),
            "min_collateral_ratio": 150,
        }))
        .deposit(NFT_MINT_FEE)
        .transact()
        .await
        .unwrap()
        .json::<(String, String)>()
        .unwrap();

    assert_eq!(
        vault_id,
        format!("{}:{}", collateral_asset.id(), stablecoin.id()),
        "Vault ID formatted incorrectly",
    );
    assert_eq!(
        token_id, "1",
        "Token ID generated incorrectly or out-of-sequence",
    );
}

#[tokio::test]
async fn test_deposit_stablecoin() {
    let (sandbox, contract) = setup().await;
    accounts!(sandbox, user, nft_collection, collateral_asset, stablecoin);

    // Create a vault first
    let (vault_id, _) = user
        .call(contract.id(), "create_vault")
        .args_json(json!({
            "nft_collection": nft_collection.id(),
            "collateral_asset": collateral_asset.id(),
            "stablecoin": stablecoin.id(),
            "min_collateral_ratio": 150,
        }))
        .deposit(NFT_MINT_FEE)
        .transact()
        .await
        .unwrap()
        .json::<(String, String)>()
        .unwrap();

    // Test deposit_stablecoin
    let deposit_amount = 1000;
    let result = user
        .call(stablecoin.id(), "ft_transfer_call")
        .args_json(json!({
            "receiver_id": contract.id(),
            "amount": deposit_amount.to_string(),
            "msg": format!("deposit:{}", vault_id),
        }))
        .deposit(NFT_MINT_FEE)
        .transact()
        .await
        .unwrap();

    // Check if the deposit was successful
    assert!(result.is_success(), "Deposit failed");

    // Check if the vault balance is updated
    let updated_vault: Value = contract
        .view("vaults")
        .args_json(json!({ "key": vault_id }))
        .await
        .unwrap()
        .json()
        .unwrap();

    let stablecoin_balance = updated_vault["stablecoin_balance"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    let deposits_count = updated_vault["deposits"].as_array().unwrap().len();

    assert_eq!(
        stablecoin_balance,
        deposit_amount,
        "Stablecoin balance not updated correctly"
    );
    assert_eq!(
        deposits_count,
        1,
        "Deposit not recorded"
    );
}

#[tokio::test]
async fn test_borrow() {
    let (sandbox, contract) = setup().await;
    accounts!(sandbox, user, nft_collection, collateral_asset, stablecoin);

    // Create a vault first
    let (vault_id, _) = user
        .call(contract.id(), "create_vault")
        .args_json(json!({
            "nft_collection": nft_collection.id(),
            "collateral_asset": collateral_asset.id(),
            "stablecoin": stablecoin.id(),
            "min_collateral_ratio": 150,
        }))
        .deposit(NFT_MINT_FEE)
        .transact()
        .await
        .unwrap()
        .json::<(String, String)>()
        .unwrap();

    // Add some liquidity to the vault
    let initial_balance = 10000;
    user
        .call(stablecoin.id(), "ft_transfer_call")
        .args_json(json!({
            "receiver_id": contract.id(),
            "amount": initial_balance.to_string(),
            "msg": format!("deposit:{}", vault_id),
        }))
        .deposit(NFT_MINT_FEE) 
        .transact()
        .await
        .unwrap();

    // Test borrow
    let collateral_amount = 1000;
    let borrow_amount = 500;

    // First, transfer collateral
    user
        .call(collateral_asset.id(), "ft_transfer_call")
        .args_json(json!({
            "receiver_id": contract.id(),
            "amount": collateral_amount.to_string(),
            "msg": format!("collateral:{}", vault_id),
        }))
        .deposit(NFT_MINT_FEE)
        .transact()
        .await
        .unwrap();

    // Then, borrow
    let result = user
        .call(contract.id(), "borrow")
        .args_json(json!({
            "vault_id": vault_id,
            "borrow_amount": borrow_amount.to_string(),
        }))
        .transact()
        .await
        .unwrap();

    // Check if the borrow was successful
    assert!(result.is_success(), "Borrow failed");

    // Check if the vault balances are updated
    let final_vault: Value = contract
        .view("vaults")
        .args_json(json!({ "key": vault_id }))
        .await
        .unwrap()
        .json()
        .unwrap();

    let collateral_balance = final_vault["collateral_balance"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    let stablecoin_balance = final_vault["stablecoin_balance"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    let loans_count = final_vault["loans"].as_array().unwrap().len();

    assert_eq!(
        collateral_balance,
        collateral_amount,
        "Collateral balance not updated correctly"
    );
    assert_eq!(
        stablecoin_balance,
        initial_balance - borrow_amount,
        "Stablecoin balance not updated correctly"
    );
    assert_eq!(
        loans_count,
        1,
        "Loan not recorded"
    );
}
