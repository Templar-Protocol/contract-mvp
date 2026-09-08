//! Integration tests for the Blend adapter using blend-contract-sdk testutils.
//!
//! These tests deploy a real Blend pool to verify supply, withdraw, total_assets,
//! and rescue through actual pool interactions.

use blend_contract_sdk::{
    pool,
    testutils::{default_reserve_config, BlendFixture},
};
use soroban_sdk::{
    contract, contractimpl,
    testutils::{Address as _, BytesN as _},
    token::StellarAssetClient,
    Address, BytesN, Env, String,
};
use templar_soroban_blend_adapter::{BlendAdapterContract, BlendAdapterContractClient};

#[contract]
struct DummyContract;

#[contractimpl]
impl DummyContract {}

fn register_dummy_contract(env: &Env) -> Address {
    env.register(DummyContract, ())
}

/// Deploy the full Blend protocol, create a pool with one reserve, and activate it.
fn setup_blend_pool(
    env: &Env,
) -> (
    Address,                // pool
    pool::Client<'_>,       // pool_client
    Address,                // asset (the reserve token)
    StellarAssetClient<'_>, // asset SAC admin client
    Address,                // deployer
) {
    let deployer = Address::generate(env);
    let blnd = env
        .register_stellar_asset_contract_v2(deployer.clone())
        .address();
    let usdc = env
        .register_stellar_asset_contract_v2(deployer.clone())
        .address();
    let blend = BlendFixture::deploy(env, &deployer, &blnd, &usdc);

    let asset_sac = env.register_stellar_asset_contract_v2(deployer.clone());
    let asset = asset_sac.address();
    let asset_admin = StellarAssetClient::new(env, &asset);

    let pool_addr = blend.pool_factory.mock_all_auths().deploy(
        &deployer,
        &String::from_str(env, "test"),
        &BytesN::<32>::random(env),
        &Address::generate(env),
        &1_000_000,
        &4,
        &10_000_000,
    );
    let pool_client = pool::Client::new(env, &pool_addr);

    let reserve_config = default_reserve_config();
    pool_client
        .mock_all_auths()
        .queue_set_reserve(&asset, &reserve_config);
    pool_client.mock_all_auths().set_reserve(&asset);

    blend
        .backstop
        .mock_all_auths()
        .deposit(&deployer, &pool_addr, &500_000_000_000);
    pool_client.mock_all_auths().set_status(&3);
    pool_client.mock_all_auths().update_status();

    (pool_addr, pool_client, asset, asset_admin, deployer)
}

fn setup_adapter<'a>(
    env: &Env,
    pool: &Address,
    vault: &Address,
) -> (Address, Address, BlendAdapterContractClient<'a>) {
    let admin = register_dummy_contract(env);
    let adapter = env.register(BlendAdapterContract, (&admin, vault, pool));
    let client = BlendAdapterContractClient::new(env, &adapter);
    (adapter, admin, client)
}

#[test]
fn supply_success_deposits_to_pool() {
    let env = Env::default();
    env.mock_all_auths();
    let (pool, pool_client, asset, asset_admin, _deployer) = setup_blend_pool(&env);

    let vault = register_dummy_contract(&env);
    let (adapter, _admin, client) = setup_adapter(&env, &pool, &vault);

    let supply_amount: i128 = 10_000_000_000;
    asset_admin.mint(&adapter, &supply_amount);

    client.supply(&vault, &asset, &supply_amount);

    let positions = pool_client.get_positions(&adapter);
    assert!(
        !positions.supply.is_empty(),
        "adapter should have supply positions after supply"
    );
    let b_tokens = positions.supply.get(0).unwrap_or(0);
    assert!(b_tokens > 0, "b_tokens should be positive after supply");
}

#[test]
fn withdraw_success_returns_assets() {
    let env = Env::default();
    env.mock_all_auths();
    let (pool, pool_client, asset, asset_admin, _deployer) = setup_blend_pool(&env);

    let vault = register_dummy_contract(&env);
    let (adapter, _admin, client) = setup_adapter(&env, &pool, &vault);

    let supply_amount: i128 = 10_000_000_000;
    asset_admin.mint(&adapter, &supply_amount);
    client.supply(&vault, &asset, &supply_amount);

    let withdraw_amount: i128 = 5_000_000_000;
    client.withdraw(&vault, &asset, &withdraw_amount);

    let token_client = soroban_sdk::token::Client::new(&env, &asset);
    let vault_balance = token_client.balance(&vault);
    assert_eq!(
        vault_balance, withdraw_amount,
        "vault should receive withdrawn assets"
    );

    let positions = pool_client.get_positions(&adapter);
    let b_tokens_after = positions.supply.get(0).unwrap_or(0);
    assert!(
        b_tokens_after > 0,
        "adapter should still have some supply after partial withdraw"
    );
}

#[test]
fn total_assets_returns_correct_value_after_supply() {
    let env = Env::default();
    env.mock_all_auths();
    let (pool, _pool_client, asset, asset_admin, _deployer) = setup_blend_pool(&env);

    let vault = register_dummy_contract(&env);
    let (adapter, _admin, client) = setup_adapter(&env, &pool, &vault);

    let supply_amount: i128 = 10_000_000_000;
    asset_admin.mint(&adapter, &supply_amount);
    client.supply(&vault, &asset, &supply_amount);

    let total = client.total_assets(&asset);
    assert!(total > 0, "total_assets should be positive after supply");
    // Fresh pool b_rate is 1:1, so total ≈ supply_amount
    let diff = (total - supply_amount).abs();
    assert!(
        diff <= 1,
        "total_assets ({total}) should be approximately supply_amount ({supply_amount})"
    );
}

#[test]
fn total_assets_without_position_returns_zero() {
    let env = Env::default();
    env.mock_all_auths();
    let (pool, _pool_client, asset, _asset_admin, _deployer) = setup_blend_pool(&env);

    let vault = register_dummy_contract(&env);
    let (adapter, _admin, _client) = setup_adapter(&env, &pool, &vault);

    env.as_contract(&adapter, || {
        let result = BlendAdapterContract::total_assets(env.clone(), asset.clone());
        assert_eq!(
            result,
            Ok(0),
            "an adapter without a pool position should report zero assets"
        );
    });
}

#[test]
fn total_assets_decreases_after_withdraw() {
    let env = Env::default();
    env.mock_all_auths();
    let (pool, _pool_client, asset, asset_admin, _deployer) = setup_blend_pool(&env);

    let vault = register_dummy_contract(&env);
    let (_adapter, _admin, client) = setup_adapter(&env, &pool, &vault);

    let supply_amount: i128 = 10_000_000_000;
    asset_admin.mint(&_adapter, &supply_amount);
    client.supply(&vault, &asset, &supply_amount);

    let total_before = client.total_assets(&asset);

    client.withdraw(&vault, &asset, &5_000_000_000);

    let total_after = client.total_assets(&asset);

    assert!(
        total_after < total_before,
        "total_assets should decrease after withdrawal: before={total_before}, after={total_after}"
    );
}

#[test]
fn rescue_transfers_assets_to_receiver() {
    let env = Env::default();
    env.mock_all_auths();

    let deployer = Address::generate(&env);
    let asset_sac = env.register_stellar_asset_contract_v2(deployer.clone());
    let asset = asset_sac.address();
    let asset_admin = StellarAssetClient::new(&env, &asset);

    let vault = register_dummy_contract(&env);
    let pool = register_dummy_contract(&env);
    let (adapter, _admin, client) = setup_adapter(&env, &pool, &vault);

    let rescue_amount: i128 = 5_000_000_000;
    let receiver = register_dummy_contract(&env);
    asset_admin.mint(&adapter, &rescue_amount);

    client.rescue(&vault, &asset, &rescue_amount, &receiver);

    let token_client = soroban_sdk::token::Client::new(&env, &asset);
    assert_eq!(
        token_client.balance(&receiver),
        rescue_amount,
        "receiver should get rescued assets"
    );
    assert_eq!(
        token_client.balance(&adapter),
        0,
        "adapter should have no assets after rescue"
    );
}

// ---------------------------------------------------------------------------
// Full flow test
// ---------------------------------------------------------------------------

#[test]
fn full_supply_withdraw_cycle() {
    let env = Env::default();
    env.mock_all_auths();
    let (pool, _pool_client, asset, asset_admin, _deployer) = setup_blend_pool(&env);

    let vault = register_dummy_contract(&env);
    let (_adapter, _admin, client) = setup_adapter(&env, &pool, &vault);

    let amount: i128 = 20_000_000_000;
    asset_admin.mint(&_adapter, &amount);

    // 1. Supply
    client.supply(&vault, &asset, &amount);

    // 2. Verify total_assets
    let total = client.total_assets(&asset);
    let diff = (total - amount).abs();
    assert!(
        diff <= 1,
        "total_assets should match supply: {total} vs {amount}"
    );

    // 3. Withdraw all
    client.withdraw(&vault, &asset, &amount);

    // 4. Verify vault received assets
    let token_client = soroban_sdk::token::Client::new(&env, &asset);
    assert_eq!(token_client.balance(&vault), amount);

    // Blend removes the supply-map entry after a complete exit. Absence means
    // zero exposure, so the adapter must remain readable after withdrawing all.
    assert_eq!(client.total_assets(&asset), 0);
}
