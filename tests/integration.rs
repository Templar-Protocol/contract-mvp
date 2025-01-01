use std::sync::atomic::{AtomicU8, Ordering};

use contract_mvp::ViewVault;
use near_sdk::{
    json_types::{U128, U64},
    AccountId, AccountIdRef,
};
use near_workspaces::{
    network::Sandbox, operations::Function, prelude::TopLevelAccountCreator, Account, Contract,
    DevNetwork, Worker,
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

async fn deploy_mock_ft(
    worker: &Worker<Sandbox>,
    account_id: AccountId,
    name: &str,
    symbol: &str,
    total_supply: u128,
) -> Contract {
    let contract = worker
        .dev_deploy(&near_workspaces::compile_project("./mock/ft").await.unwrap())
        .await
        .unwrap();

    contract
        .call("new")
        .args_json(json!({
            "name": name,
            "symbol": symbol,
            "owner_id": account_id,
            "supply": U128(total_supply),
        }))
        .transact()
        .await
        .unwrap()
        .unwrap();

    contract
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

enum ContractWasm {
    Vault,
    MockFt,
}

static V: AtomicU8 = AtomicU8::new(0);

#[test]
fn order1() {
    let i = V.fetch_add(1, Ordering::Relaxed);
    println!("previous: {i}");
}

#[test]
fn order2() {
    let i = V.fetch_add(10, Ordering::Relaxed);
    println!("previous: {i}");
}

async fn contract_wasm(which: ContractWasm) -> &'static [u8] {
    // match which {
    //     ContractWasm::Vault => {
    //         static WASM: Mutex<Option<&[u8]>> = Mutex::new(None);
    //         let lock = WASM.lock().unwrap();
    //         if lock.
    //         *WASM.lock().unwrap() = Some(b"8");
    //     }
    //     ContractWasm::MockFt => {}
    // }

    todo!()
}

async fn deploy_ft(
    account: &Account,
    name: &str,
    symbol: &str,
    owner_id: &AccountIdRef,
    supply: u128,
) {
    let wasm = near_workspaces::compile_project("./mock/ft/")
        .await
        .unwrap();
    account
        .batch(account.id())
        .deploy(&wasm)
        .call(Function::new("new").args_json(json!({
            "name": name,
            "symbol": symbol,
            "owner_id": owner_id,
            "supply": U128(supply),
        })))
        .transact()
        .await
        .unwrap()
        .unwrap();
}

// ===== TESTS =====

#[tokio::test]
async fn test_create_vault() {
    let (worker, contract) = setup().await;
    accounts!(worker, user, collateral_asset, stablecoin);

    let result = user
        .call(contract.id(), "create_vault")
        .args_json(json!({
            "loan_asset_id": stablecoin.id(),
            "collateral_asset_id": collateral_asset.id(),
            "min_collateral_ratio": [150, 100],
        }))
        .transact()
        .await
        .unwrap();

    assert!(result.is_success());

    let vault_id = result.json::<U64>().unwrap().0;
    println!("Created vault with ID: {vault_id}");

    // Verify the vault was created
    let vault = contract
        .view("get_vault")
        .args_json(json!({ "vault_id": U64(vault_id) }))
        .await
        .unwrap()
        .json::<ViewVault>()
        .unwrap();

    assert_eq!(
        &vault.collateral_asset_id.into_nep141().unwrap(),
        collateral_asset.id(),
    );
    assert_eq!(&vault.loan_asset_id.into_nep141().unwrap(), stablecoin.id());
    assert_eq!(vault.min_collateral_ratio, (150, 100));
}
