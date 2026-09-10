#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value;
// every `#[contractimpl]` method in this crate is an ABI entry point.
#![allow(clippy::needless_pass_by_value)]

use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, symbol_short, Address, BytesN, Env,
    Symbol, Vec,
};
use stellar_access::ownable::{renounce_ownership as relinquish_ownership, set_owner, Ownable};
use stellar_macros::only_owner;
use templar_proxy_oracle_soroban_common::{
    bucket_timestamp, extend_instance_ttl, normalized_to_sep40, owner_upgrade, Asset,
    ContractError, PriceData, PriceFeedTrait, ProxyOracleClient, MAX_SEP40_DECIMALS,
};

const MAX_HISTORY_RECORDS: u32 = 32;

/// Keep the deployed `Config` encoding stable; decommissioning uses a separate key.
const CONFIG: Symbol = symbol_short!("CONFIG");
const DECOMMISSIONED: Symbol = symbol_short!("DECOM");

soroban_sdk::contractmeta!(key = "sep", val = "40");

#[contractevent]
#[derive(Clone)]
pub struct DecimalsUpdated {
    pub decimals: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct AdapterDecommissioned {
    pub decommissioned: bool,
}

#[contractevent]
#[derive(Clone)]
pub struct AdapterUpgraded {
    pub new_wasm_hash: BytesN<32>,
}

#[contract]
pub struct Sep40Adapter;

#[contracttype]
#[derive(Clone)]
pub struct Config {
    pub parent_oracle: Address,
    pub asset: Asset,
    pub decimals: u32,
    pub resolution: u32,
    pub base: Asset,
}

#[contractimpl]
impl Sep40Adapter {
    pub fn __constructor(
        env: Env,
        owner: Address,
        parent_oracle: Address,
        asset: Asset,
        decimals: u32,
        resolution: u32,
        base: Asset,
    ) -> Result<(), ContractError> {
        if decimals > MAX_SEP40_DECIMALS || resolution == 0 {
            return Err(ContractError::InvalidInput);
        }
        let parent = ProxyOracleClient::new(&env, &parent_oracle);
        if !matches!(parent.try_source_base(), Ok(Ok(Some(source_base))) if source_base == base) {
            return Err(ContractError::InvalidInput);
        }
        extend_instance_ttl(&env);
        env.storage().instance().set(
            &CONFIG,
            &Config {
                parent_oracle,
                asset,
                decimals,
                resolution,
                base,
            },
        );
        set_owner(&env, &owner);
        Ok(())
    }

    #[only_owner]
    pub fn set_decimals(env: Env, decimals: u32) -> Result<(), ContractError> {
        if decimals > MAX_SEP40_DECIMALS {
            return Err(ContractError::InvalidInput);
        }
        extend_instance_ttl(&env);
        let mut config = load_config(&env);
        if is_decommissioned(&env) {
            return Err(ContractError::InvalidInput);
        }
        config.decimals = decimals;
        env.storage().instance().set(&CONFIG, &config);
        DecimalsUpdated { decimals }.publish(&env);
        Ok(())
    }

    /// Signature matches the OpenZeppelin `Upgradeable` trait shape
    /// (`upgrade(env, new_wasm_hash, operator)`) so this adapter is
    /// forward-compatible with `stellar-contract-utils` adoption later.
    pub fn upgrade(
        env: Env,
        new_wasm_hash: BytesN<32>,
        operator: Address,
    ) -> Result<(), ContractError> {
        owner_upgrade(&env, &new_wasm_hash, &operator)?;
        extend_instance_ttl(&env);
        AdapterUpgraded { new_wasm_hash }.publish(&env);
        Ok(())
    }

    pub fn extend_ttl(env: Env) {
        extend_instance_ttl(&env);
    }

    #[only_owner]
    pub fn decommission(env: Env) {
        if is_decommissioned(&env) {
            return;
        }
        env.storage().instance().set(&DECOMMISSIONED, &true);
        AdapterDecommissioned {
            decommissioned: true,
        }
        .publish(&env);
    }

    pub fn config(env: Env) -> Option<Config> {
        extend_instance_ttl(&env);
        env.storage().instance().get(&CONFIG)
    }
}

#[contractimpl(contracttrait)]
impl Ownable for Sep40Adapter {
    fn renounce_ownership(env: &Env) {
        if !is_decommissioned(env) {
            env.panic_with_error(ContractError::InvalidInput);
        }
        relinquish_ownership(env);
    }
}

// SEP-40 getters cannot return Option per the interface contract; panic on
// missing key is documented fail-closed behavior.
#[allow(clippy::expect_used)]
#[contractimpl]
impl PriceFeedTrait for Sep40Adapter {
    fn base(env: Env) -> Asset {
        load_config(&env).base
    }

    fn assets(env: Env) -> Vec<Asset> {
        let config = load_config(&env);
        if is_decommissioned(&env) {
            return Vec::new(&env);
        }
        let mut assets = Vec::new(&env);
        assets.push_back(config.asset);
        assets
    }

    fn decimals(env: Env) -> u32 {
        load_config(&env).decimals
    }

    fn resolution(env: Env) -> u32 {
        load_config(&env).resolution
    }

    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData> {
        let config = active_config(&env, &asset)?;
        if timestamp % u64::from(config.resolution) != 0 {
            return None;
        }
        let client = ProxyOracleClient::new(&env, &config.parent_oracle);
        let history = client.aggregated_history(&asset, &MAX_HISTORY_RECORDS)?;
        for entry in history.iter().rev() {
            if bucket_timestamp(entry.timestamp, config.resolution) == timestamp {
                return normalized_to_adapter(&entry, config.decimals, config.resolution).ok();
            }
        }
        None
    }

    fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>> {
        if records == 0 {
            return None;
        }
        let config = active_config(&env, &asset)?;
        let client = ProxyOracleClient::new(&env, &config.parent_oracle);
        let history = client.aggregated_history(&asset, &records)?;
        let mut prices: Vec<PriceData> = Vec::new(&env);
        for entry in history.iter() {
            let Ok(price) = normalized_to_adapter(&entry, config.decimals, config.resolution)
            else {
                continue;
            };
            let previous_index = prices.len().saturating_sub(1);
            if prices
                .get(previous_index)
                .is_some_and(|previous| previous.timestamp == price.timestamp)
            {
                prices.set(previous_index, price);
            } else {
                prices.push_back(price);
            }
        }
        (!prices.is_empty()).then_some(prices)
    }

    fn lastprice(env: Env, asset: Asset) -> Option<PriceData> {
        let config = active_config(&env, &asset)?;
        let client = ProxyOracleClient::new(&env, &config.parent_oracle);
        let normalized = client.aggregated_latest(&asset)?;
        normalized_to_adapter(&normalized, config.decimals, config.resolution).ok()
    }
}

#[allow(clippy::expect_used)]
fn load_config(env: &Env) -> Config {
    extend_instance_ttl(env);
    env.storage().instance().get(&CONFIG).expect("CONFIG")
}

fn is_decommissioned(env: &Env) -> bool {
    env.storage()
        .instance()
        .get::<_, bool>(&DECOMMISSIONED)
        .unwrap_or(false)
}

fn active_config(env: &Env, asset: &Asset) -> Option<Config> {
    let config = load_config(env);
    if is_decommissioned(env) || &config.asset != asset || !parent_base_matches(env, &config) {
        return None;
    }
    Some(config)
}

fn parent_base_matches(env: &Env, config: &Config) -> bool {
    let parent = ProxyOracleClient::new(env, &config.parent_oracle);
    matches!(parent.try_source_base(), Ok(Ok(Some(base))) if base == config.base)
}

fn normalized_to_adapter(
    price: &templar_proxy_oracle_soroban_common::NormalizedPrice,
    decimals: u32,
    resolution: u32,
) -> Result<PriceData, ContractError> {
    let mut projected = normalized_to_sep40(price, decimals)?;
    if price.mantissa != 0 && projected.price == 0 {
        return Err(ContractError::ConversionOverflow);
    }
    projected.timestamp = bucket_timestamp(projected.timestamp, resolution);
    Ok(projected)
}

#[cfg(test)]
mod tests;
