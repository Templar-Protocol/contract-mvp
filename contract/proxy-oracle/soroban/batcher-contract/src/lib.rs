#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value.
#![allow(clippy::needless_pass_by_value)]

use soroban_sdk::{contract, contractimpl, Address, Env, Symbol, Vec};
use templar_proxy_oracle_soroban_common::{
    Asset, ProxyOracleMaintenanceClient, RefreshStatus, DEFAULT_TTL_EXTEND_TO,
    DEFAULT_TTL_THRESHOLD,
};

/// Stateless fan-out so a keeper can service every asset in one Soroban
/// operation. Holds no authority: every call it forwards is permissionless on
/// the target.
#[contract]
pub struct ProxyOracleBatcher;

#[contractimpl]
impl ProxyOracleBatcher {
    /// `refresh(asset)` on `oracle` for each asset, in order. A trap in any
    /// refresh reverts the whole batch; status-level failures are returned.
    pub fn refresh_many(env: Env, oracle: Address, assets: Vec<Asset>) -> Vec<RefreshStatus> {
        extend_self(&env);
        let client = ProxyOracleMaintenanceClient::new(&env, &oracle);
        let mut statuses = Vec::new(&env);
        for asset in assets.iter() {
            statuses.push_back(client.refresh(&asset));
        }
        statuses
    }

    /// Renew `oracle`'s instance and code, then `extend_ttl(asset)` on it for
    /// each asset; `false` marks an asset the runtime rejected (unregistered).
    pub fn extend_ttl_many(env: Env, oracle: Address, assets: Vec<Asset>) -> Vec<bool> {
        extend_self(&env);
        extend_instance_and_code(&env, oracle.clone());
        let client = ProxyOracleMaintenanceClient::new(&env, &oracle);
        let mut results = Vec::new(&env);
        for asset in assets.iter() {
            results.push_back(matches!(client.try_extend_ttl(&asset), Ok(Ok(()))));
        }
        results
    }

    /// Renew each contract's instance and code, then call its argument-less
    /// `extend_ttl()` for its persistent entries; `false` marks a failed call.
    pub fn extend_ttl_contracts(env: Env, contracts: Vec<Address>) -> Vec<bool> {
        extend_self(&env);
        let extend_ttl = Symbol::new(&env, "extend_ttl");
        let no_args = Vec::new(&env);
        let mut results = Vec::new(&env);
        for contract in contracts.iter() {
            extend_instance_and_code(&env, contract.clone());
            let outcome = env.try_invoke_contract::<(), soroban_sdk::Error>(
                &contract,
                &extend_ttl,
                no_args.clone(),
            );
            results.push_back(matches!(outcome, Ok(Ok(()))));
        }
        results
    }
}

fn extend_self(env: &Env) {
    extend_instance_and_code(env, env.current_contract_address());
}

fn extend_instance_and_code(env: &Env, contract: Address) {
    env.deployer()
        .extend_ttl(contract, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
}
