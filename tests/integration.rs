use contract_mvp::NFT_MINT_FEE;
use near_sdk::AccountId;
use near_workspaces::{
    network::Sandbox, prelude::TopLevelAccountCreator, Account, Contract, DevNetwork, Worker,
};
use serde_json::json;

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
