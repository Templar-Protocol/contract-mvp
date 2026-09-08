#![no_std]

use soroban_sdk::{
    address_payload::AddressPayload,
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    BytesN, Env, IntoVal, Symbol, Vec,
};
use stellar_contract_utils::upgradeable::{self, Upgradeable};

use blend_contract_sdk::pool::{Client as PoolClient, Request};

const REQUEST_SUPPLY: u32 = 0;
const REQUEST_WITHDRAW: u32 = 1;
const SCALAR_12: i128 = 1_000_000_000_000;
/// Re-extend instance TTL when remaining TTL drops below ~30 days.
const INSTANCE_TTL_THRESHOLD: u32 = 518_400;
/// Extend instance TTL to the Soroban maximum (~6 months).
const INSTANCE_TTL_EXTEND_TO: u32 = 3_110_400;
/// Maximum allowed staleness for reserve data in seconds.
const RESERVE_STALE_WINDOW_SECS: u64 = 300;

#[contracttype]
#[derive(Clone, Debug)]
enum DataKey {
    Admin,
    PendingAdmin,
    Vault,
    Pool,
    Paused,
}

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AdapterError {
    Unauthorized = 1,
    InvalidInput = 2,
    MissingConfig = 3,
    /// Arithmetic overflow when computing total assets.
    ArithmeticOverflow = 4,
    /// Reserved for backward compatibility with the deployed error ABI.
    MissingPosition = 5,
    /// Arithmetic underflow when computing actual withdrawal.
    ArithmeticUnderflow = 6,
    /// Withdrawal returned zero assets.
    ZeroWithdrawal = 7,
    /// Reserve data is stale.
    StaleReserve = 8,
    /// Emergency pause is active.
    Paused = 9,
}

#[contract]
pub struct BlendAdapterContract;

#[contractimpl]
impl BlendAdapterContract {
    /// Runs atomically during contract deployment — no separate `initialize`
    /// transaction that could be front-run.
    pub fn __constructor(
        env: Env,
        admin: Address,
        vault: Address,
        pool: Address,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_contract_address(&vault, AdapterError::InvalidInput)?;
        require_contract_address(&pool, AdapterError::InvalidInput)?;

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Vault, &vault);
        env.storage().instance().set(&DataKey::Pool, &pool);
        Ok(())
    }

    /// Pause or unpause adapter state-changing operations (admin or vault).
    #[allow(deprecated)]
    pub fn set_paused(env: Env, caller: Address, paused: bool) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin_or_vault(&env, &caller)?;
        env.storage().instance().set(&DataKey::Paused, &paused);
        env.events()
            .publish((symbol_short!("paused"), caller), paused);
        Ok(())
    }

    pub fn paused(env: Env) -> bool {
        extend_instance_ttl(&env);
        is_paused(&env)
    }

    /// Supply assets from the adapter into the Blend pool (vault-only).
    #[allow(deprecated)]
    pub fn supply(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_not_paused(&env)?;
        require_vault(&env, &caller)?;
        if amount <= 0 {
            return Err(AdapterError::InvalidInput);
        }

        let pool = get_pool(&env)?;
        let client = PoolClient::new(&env, &pool);
        let adapter = env.current_contract_address();
        let request = Request {
            request_type: REQUEST_SUPPLY,
            address: asset.clone(),
            amount,
        };
        let mut requests = Vec::new(&env);
        requests.push_back(request);

        // Authorize the token transfer the pool will make from the adapter.
        env.authorize_as_current_contract(Vec::from_array(
            &env,
            [InvokerContractAuthEntry::Contract(SubContractInvocation {
                context: ContractContext {
                    contract: asset.clone(),
                    fn_name: Symbol::new(&env, "transfer"),
                    args: (adapter.clone(), pool.clone(), amount).into_val(&env),
                },
                sub_invocations: Vec::new(&env),
            })],
        ));

        client.submit(&adapter, &adapter, &adapter, &requests);
        env.events()
            .publish((symbol_short!("supply"), asset), amount);
        Ok(())
    }

    /// Withdraw assets from the Blend pool and transfer them to the vault.
    #[allow(deprecated)]
    pub fn withdraw(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<(), AdapterError> {
        Self::progress_withdrawal(env, caller, asset, amount).map(|_| ())
    }

    /// Progress a withdrawal from Blend and return actual assets sent to the vault.
    #[allow(deprecated)]
    pub fn progress_withdrawal(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
    ) -> Result<i128, AdapterError> {
        extend_instance_ttl(&env);
        require_not_paused(&env)?;
        require_vault(&env, &caller)?;
        if amount <= 0 {
            return Err(AdapterError::InvalidInput);
        }
        let vault = get_vault(&env)?;

        let pool = get_pool(&env)?;
        let client = PoolClient::new(&env, &pool);
        let adapter = env.current_contract_address();
        let request = Request {
            request_type: REQUEST_WITHDRAW,
            address: asset.clone(),
            amount,
        };
        let mut requests = Vec::new(&env);
        requests.push_back(request);
        let token = soroban_sdk::token::Client::new(&env, &asset);
        let balance_before = token.balance(&adapter);
        client.submit(&adapter, &adapter, &adapter, &requests);

        let balance_after = token.balance(&adapter);
        let actual_withdrawn = balance_after
            .checked_sub(balance_before)
            .ok_or(AdapterError::ArithmeticUnderflow)?;
        if actual_withdrawn <= 0 {
            return Err(AdapterError::ZeroWithdrawal);
        }
        token.transfer(&adapter, &vault, &actual_withdrawn);
        env.events()
            .publish((symbol_short!("withdraw"), asset), actual_withdrawn);
        Ok(actual_withdrawn)
    }

    /// Rescue assets held by the adapter and transfer them to `receiver`.
    #[allow(deprecated)]
    pub fn rescue(
        env: Env,
        caller: Address,
        asset: Address,
        amount: i128,
        receiver: Address,
    ) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_not_paused(&env)?;
        require_vault(&env, &caller)?;
        if amount <= 0 {
            return Err(AdapterError::InvalidInput);
        }
        require_contract_address(&receiver, AdapterError::InvalidInput)?;
        if receiver == env.current_contract_address() {
            return Err(AdapterError::InvalidInput);
        }

        let adapter = env.current_contract_address();
        let token = soroban_sdk::token::Client::new(&env, &asset);
        token.transfer(&adapter, &receiver, &amount);
        env.events()
            .publish((symbol_short!("rescue"), asset, receiver), amount);
        Ok(())
    }

    /// Query total assets for `asset` from the Blend pool.
    pub fn total_assets(env: Env, asset: Address) -> Result<i128, AdapterError> {
        extend_instance_ttl(&env);
        let pool = get_pool(&env)?;
        let client = PoolClient::new(&env, &pool);
        let reserve = client.get_reserve(&asset);
        let now = env.ledger().timestamp();
        let last_update = reserve.data.last_time as u64;
        if now.saturating_sub(last_update) > RESERVE_STALE_WINDOW_SECS {
            return Err(AdapterError::StaleReserve);
        }
        let positions = client.get_positions(&env.current_contract_address());
        let index = reserve.config.index;
        // Blend omits zero-valued positions from its sparse supply map.
        let b_tokens = positions.supply.get(index).unwrap_or(0);
        b_tokens
            .checked_mul(reserve.data.b_rate)
            .and_then(|value| value.checked_div(SCALAR_12))
            .ok_or(AdapterError::ArithmeticOverflow)
    }

    pub fn admin(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_admin(&env)
    }

    pub fn vault(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_vault(&env)
    }

    pub fn pool(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_pool(&env)
    }

    /// Propose a new admin. The pending admin must accept in a separate call.
    #[allow(deprecated)]
    pub fn set_admin(env: Env, caller: Address, admin: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        env.storage().instance().set(&DataKey::PendingAdmin, &admin);
        env.events()
            .publish((symbol_short!("admin_set"), caller), admin);
        Ok(())
    }

    /// Accept admin role previously proposed with `set_admin`.
    #[allow(deprecated)]
    pub fn accept_admin(env: Env, caller: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        caller.require_auth();
        let pending_admin = get_pending_admin(&env)?;
        if caller != pending_admin {
            return Err(AdapterError::Unauthorized);
        }
        let old_admin = get_admin(&env)?;
        env.storage().instance().set(&DataKey::Admin, &caller);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        env.events()
            .publish((symbol_short!("admin_acc"), old_admin), caller);
        Ok(())
    }

    pub fn pending_admin(env: Env) -> Result<Address, AdapterError> {
        extend_instance_ttl(&env);
        get_pending_admin(&env)
    }

    /// Extend instance storage TTL (admin-only).
    pub fn extend_ttl(env: Env, caller: Address) -> Result<(), AdapterError> {
        extend_instance_ttl(&env);
        require_admin(&env, &caller)?;
        Ok(())
    }
}

#[contractimpl]
impl Upgradeable for BlendAdapterContract {
    #[allow(deprecated)]
    fn upgrade(e: &Env, new_wasm_hash: BytesN<32>, operator: Address) {
        extend_instance_ttl(e);
        require_admin(e, &operator).unwrap_or_else(|err| panic_with_error!(e, err));
        upgradeable::upgrade(e, &new_wasm_hash);
        e.events()
            .publish((symbol_short!("upgrade"), operator), new_wasm_hash);
    }
}

fn get_address(env: &Env, key: DataKey) -> Result<Address, AdapterError> {
    env.storage()
        .instance()
        .get(&key)
        .ok_or(AdapterError::MissingConfig)
}

fn get_admin(env: &Env) -> Result<Address, AdapterError> {
    get_address(env, DataKey::Admin)
}

fn get_pending_admin(env: &Env) -> Result<Address, AdapterError> {
    get_address(env, DataKey::PendingAdmin)
}

fn get_vault(env: &Env) -> Result<Address, AdapterError> {
    get_address(env, DataKey::Vault)
}

fn get_pool(env: &Env) -> Result<Address, AdapterError> {
    get_address(env, DataKey::Pool)
}

fn require_admin(env: &Env, caller: &Address) -> Result<(), AdapterError> {
    caller.require_auth();
    let admin = get_admin(env)?;
    if caller != &admin {
        return Err(AdapterError::Unauthorized);
    }
    Ok(())
}

fn require_vault(env: &Env, caller: &Address) -> Result<(), AdapterError> {
    caller.require_auth();
    let vault = get_vault(env)?;
    if caller != &vault {
        return Err(AdapterError::Unauthorized);
    }
    Ok(())
}

fn require_admin_or_vault(env: &Env, caller: &Address) -> Result<(), AdapterError> {
    caller.require_auth();
    let admin = get_admin(env)?;
    let vault = get_vault(env)?;
    if caller != &admin && caller != &vault {
        return Err(AdapterError::Unauthorized);
    }
    Ok(())
}

fn is_paused(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false)
}

fn require_not_paused(env: &Env) -> Result<(), AdapterError> {
    if is_paused(env) {
        return Err(AdapterError::Paused);
    }
    Ok(())
}

fn is_contract_address(addr: &Address) -> bool {
    matches!(
        AddressPayload::from_address(addr),
        Some(AddressPayload::ContractIdHash(_))
    )
}

fn require_contract_address(addr: &Address, err: AdapterError) -> Result<(), AdapterError> {
    if is_contract_address(addr) {
        Ok(())
    } else {
        Err(err)
    }
}

fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(INSTANCE_TTL_THRESHOLD, INSTANCE_TTL_EXTEND_TO);
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use soroban_sdk::testutils::{Address as _, Events as _};

    const BLEND_ADAPTER_SOURCE: &str = include_str!("lib.rs");

    #[test]
    fn admin_accounting_shortcuts_are_not_public_entrypoints() {
        assert!(!BLEND_ADAPTER_SOURCE.contains(concat!("\n    pub fn ", "supply_balance(")));
        assert!(!BLEND_ADAPTER_SOURCE.contains(concat!("\n    pub fn ", "withdraw_to_vault(")));
    }

    #[contract]
    struct DummyContract;

    #[contractimpl]
    impl DummyContract {}

    fn register_dummy_contract(env: &Env) -> Address {
        env.register(DummyContract, ())
    }

    fn account_address(env: &Env) -> Address {
        Address::from_str(
            env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        )
    }

    fn adapter_event_count(env: &Env, contract_id: &Address) -> usize {
        env.events()
            .all()
            .filter_by_contract(contract_id)
            .events()
            .len()
    }

    fn empty_wasm_hash(env: &Env) -> BytesN<32> {
        BytesN::from_array(
            env,
            &[
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
                0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
                0x78, 0x52, 0xb8, 0x55,
            ],
        )
    }

    #[test]
    fn constructor_sets_config() {
        let env = Env::default();
        let admin = register_dummy_contract(&env);
        let vault = register_dummy_contract(&env);
        let pool = register_dummy_contract(&env);

        let contract_id = env.register(BlendAdapterContract, (&admin, &vault, &pool));
        env.as_contract(&contract_id, || {
            assert_eq!(BlendAdapterContract::admin(env.clone()).unwrap(), admin);
            assert_eq!(BlendAdapterContract::vault(env.clone()).unwrap(), vault);
            assert_eq!(BlendAdapterContract::pool(env.clone()).unwrap(), pool);
        });
    }

    #[test]
    fn constructor_accepts_account_admin() {
        let env = Env::default();
        let admin = account_address(&env);
        let vault = register_dummy_contract(&env);
        let pool = register_dummy_contract(&env);

        let contract_id = env.register(BlendAdapterContract, (&admin, &vault, &pool));
        env.as_contract(&contract_id, || {
            assert_eq!(BlendAdapterContract::admin(env.clone()).unwrap(), admin);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #2)")]
    fn constructor_rejects_non_contract_vault() {
        let env = Env::default();
        let admin = register_dummy_contract(&env);
        let vault = account_address(&env);
        let pool = register_dummy_contract(&env);

        env.register(BlendAdapterContract, (&admin, &vault, &pool));
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #2)")]
    fn constructor_rejects_non_contract_pool() {
        let env = Env::default();
        let admin = register_dummy_contract(&env);
        let vault = register_dummy_contract(&env);
        let pool = account_address(&env);

        env.register(BlendAdapterContract, (&admin, &vault, &pool));
    }

    /// Helper: deploy a contract via constructor and return (contract_id, admin, vault, pool).
    fn setup_adapter(env: &Env) -> (Address, Address, Address, Address) {
        let admin = register_dummy_contract(env);
        let vault = register_dummy_contract(env);
        let pool = register_dummy_contract(env);
        let contract_id = env.register(BlendAdapterContract, (&admin, &vault, &pool));
        (contract_id, admin, vault, pool)
    }

    #[test]
    fn supply_zero_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), 0);
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn supply_negative_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::supply(env.clone(), vault.clone(), asset.clone(), -100);
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn supply_unauthorized_caller_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let impostor = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::supply(env.clone(), impostor.clone(), asset.clone(), 100);
            assert_eq!(result, Err(AdapterError::Unauthorized));
        });
    }

    #[test]
    fn admin_can_pause_adapter_operations() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            assert!(!BlendAdapterContract::paused(env.clone()));
            BlendAdapterContract::set_paused(env.clone(), admin, true).unwrap();
            assert!(BlendAdapterContract::paused(env.clone()));

            let result = BlendAdapterContract::supply(env.clone(), vault, asset, 100);
            assert_eq!(result, Err(AdapterError::Paused));
        });
    }

    #[test]
    fn vault_can_pause_adapter_operations() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::set_paused(env.clone(), vault.clone(), true).unwrap();

            let result = BlendAdapterContract::withdraw(env.clone(), vault, asset, 100);
            assert_eq!(result, Err(AdapterError::Paused));
        });
    }

    #[test]
    fn non_admin_or_vault_cannot_pause_adapter() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _pool) = setup_adapter(&env);
        let impostor = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::set_paused(env.clone(), impostor, true);
            assert_eq!(result, Err(AdapterError::Unauthorized));
        });
    }

    #[test]
    fn withdraw_zero_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::withdraw(env.clone(), vault.clone(), asset.clone(), 0);
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn withdraw_negative_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::withdraw(env.clone(), vault.clone(), asset.clone(), -50);
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn withdraw_unauthorized_caller_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let impostor = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result =
                BlendAdapterContract::withdraw(env.clone(), impostor.clone(), asset.clone(), 100);
            assert_eq!(result, Err(AdapterError::Unauthorized));
        });
    }

    #[test]
    fn rescue_zero_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let receiver = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::rescue(
                env.clone(),
                vault.clone(),
                asset.clone(),
                0,
                receiver.clone(),
            );
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn rescue_unauthorized_caller_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, _vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let impostor = Address::generate(&env);
        let receiver = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::rescue(
                env.clone(),
                impostor.clone(),
                asset.clone(),
                100,
                receiver.clone(),
            );
            assert_eq!(result, Err(AdapterError::Unauthorized));
        });
    }

    #[test]
    fn rescue_rejects_adapter_receiver() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::rescue(
                env.clone(),
                vault.clone(),
                asset.clone(),
                100,
                contract_id.clone(),
            );
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn rescue_rejects_non_contract_receiver() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, _admin, vault, _pool) = setup_adapter(&env);
        let asset = Address::generate(&env);
        let account = Address::from_str(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        );
        env.as_contract(&contract_id, || {
            let result = BlendAdapterContract::rescue(
                env.clone(),
                vault.clone(),
                asset.clone(),
                100,
                account,
            );
            assert_eq!(result, Err(AdapterError::InvalidInput));
        });
    }

    #[test]
    fn blend_adapter_does_not_expose_admin_retarget_entrypoints() {
        let source = include_str!("lib.rs");
        assert!(!source.contains(concat!("pub fn ", "set_pool")));
        assert!(!source.contains(concat!("pub fn ", "set_vault")));
        assert!(!source.contains(concat!("pool", "_upd")));
        assert!(!source.contains(concat!("vlt", "_upd")));
    }

    #[test]
    fn set_admin_requires_pending_admin_acceptance() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _pool) = setup_adapter(&env);
        let new_admin = register_dummy_contract(&env);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::set_admin(env.clone(), admin.clone(), new_admin.clone()).unwrap();
            assert_eq!(BlendAdapterContract::admin(env.clone()).unwrap(), admin);
            assert_eq!(
                BlendAdapterContract::pending_admin(env.clone()).unwrap(),
                new_admin
            );
            BlendAdapterContract::accept_admin(env.clone(), new_admin.clone()).unwrap();
            assert_eq!(BlendAdapterContract::admin(env.clone()).unwrap(), new_admin);
            assert_eq!(
                BlendAdapterContract::pending_admin(env.clone()),
                Err(AdapterError::MissingConfig)
            );
        });
    }

    #[test]
    fn set_admin_emits_propose_and_accept_events() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _pool) = setup_adapter(&env);
        let new_admin = register_dummy_contract(&env);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::set_admin(env.clone(), admin, new_admin.clone()).unwrap();
            BlendAdapterContract::accept_admin(env.clone(), new_admin).unwrap();
        });
        assert_eq!(adapter_event_count(&env, &contract_id), 2);
    }

    #[test]
    fn set_admin_accepts_account_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _pool) = setup_adapter(&env);
        let account = account_address(&env);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::set_admin(env.clone(), admin, account.clone()).unwrap();
        });
        env.as_contract(&contract_id, || {
            assert_eq!(
                BlendAdapterContract::pending_admin(env.clone()).unwrap(),
                account
            );
        });
        env.as_contract(&contract_id, || {
            BlendAdapterContract::accept_admin(env.clone(), account.clone()).unwrap();
        });
        env.as_contract(&contract_id, || {
            BlendAdapterContract::set_paused(env.clone(), account.clone(), true).unwrap();
            assert_eq!(BlendAdapterContract::admin(env.clone()).unwrap(), account);
            assert!(BlendAdapterContract::paused(env.clone()));
        });
    }

    #[test]
    fn upgrade_requires_admin_and_emits_event() {
        let env = Env::default();
        env.mock_all_auths();
        let (contract_id, admin, _vault, _pool) = setup_adapter(&env);
        let new_hash = empty_wasm_hash(&env);
        env.as_contract(&contract_id, || {
            BlendAdapterContract::upgrade(&env, new_hash.clone(), admin);
        });
        assert_eq!(adapter_event_count(&env, &contract_id), 1);
    }

    // Note: "query before initialize" test not applicable — __constructor
    // runs atomically during `env.register()`, so there is no uninitialized state.
}
