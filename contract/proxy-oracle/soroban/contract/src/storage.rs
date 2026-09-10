//! Storage layout for the runtime contract.
//!
//! `instance` storage holds the source base; the owner address is managed by
//! `stellar_access::ownable` under its own storage key. `persistent` storage
//! is keyed per-asset (proxy config, breaker set, price cache, history).
//! SEP-40 surface concerns (decimals/resolution) live in the per-feed
//! `Sep40Adapter` contracts, not here. `Base` is retained as the
//! source-validation invariant — every source must report prices in the same
//! base.

use soroban_sdk::{contracttype, Bytes, Env, Vec};
use templar_proxy_oracle_kernel::proxy::circuit_breaker::CircuitBreakerSet;
use templar_proxy_oracle_soroban_common::{Asset, ContractError, NormalizedPrice};

use crate::{CachedProxyPrice, MAX_REGISTERED_ASSETS};

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Base,
    Assets,
    Proxy(Asset),
    Breakers(Asset),
    Cache(Asset),
    History(Asset),
}

pub(crate) struct PendingHistoryUpdate {
    key: DataKey,
    history: Vec<NormalizedPrice>,
}

pub(crate) enum HistoryUpdate {
    Append(PendingHistoryUpdate),
    Unchanged(NormalizedPrice),
}

pub fn require_proxy_exists(env: &Env, asset: &Asset) -> Result<(), ContractError> {
    if env
        .storage()
        .persistent()
        .has(&DataKey::Proxy(asset.clone()))
    {
        Ok(())
    } else {
        Err(ContractError::InvalidInput)
    }
}

pub fn invalidate_cache(env: &Env, asset: &Asset) {
    env.storage()
        .persistent()
        .remove(&DataKey::Cache(asset.clone()));
}

pub fn load_assets(env: &Env) -> Result<Vec<Asset>, ContractError> {
    let assets: Vec<Asset> = env
        .storage()
        .persistent()
        .get(&DataKey::Assets)
        .ok_or(ContractError::StorageError)?;
    if assets.len() > MAX_REGISTERED_ASSETS {
        return Err(ContractError::StorageError);
    }
    for i in 0..assets.len() {
        let asset = assets.get(i).ok_or(ContractError::StorageError)?;
        for j in (i + 1)..assets.len() {
            if asset == assets.get(j).ok_or(ContractError::StorageError)? {
                return Err(ContractError::StorageError);
            }
        }
    }
    Ok(assets)
}

pub fn add_asset(env: &Env, asset: &Asset) -> Result<(), ContractError> {
    let mut assets = load_assets(env)?;
    if assets.iter().any(|entry| &entry == asset) {
        return Ok(());
    }
    if assets.len() >= MAX_REGISTERED_ASSETS {
        return Err(ContractError::TooManyAssets);
    }
    assets.push_back(asset.clone());
    env.storage().persistent().set(&DataKey::Assets, &assets);
    Ok(())
}

pub fn remove_asset(env: &Env, asset: &Asset) -> Result<(), ContractError> {
    let mut assets: Vec<Asset> = env
        .storage()
        .persistent()
        .get(&DataKey::Assets)
        .ok_or(ContractError::StorageError)?;
    if let Some(index) = assets
        .iter()
        .position(|entry| &entry == asset)
        .and_then(|i| u32::try_from(i).ok())
    {
        assets.remove(index);
        env.storage().persistent().set(&DataKey::Assets, &assets);
    }
    Ok(())
}

pub fn load_breakers(env: &Env, asset: &Asset) -> Result<CircuitBreakerSet, ContractError> {
    let Some(bytes) = env
        .storage()
        .persistent()
        .get::<_, Bytes>(&DataKey::Breakers(asset.clone()))
    else {
        return Err(ContractError::StorageError);
    };
    let breakers: CircuitBreakerSet =
        postcard::from_bytes(&bytes.to_alloc_vec()).map_err(|_| ContractError::StorageError)?;
    breakers
        .validate()
        .map_err(|_| ContractError::StorageError)?;
    Ok(breakers)
}

pub fn store_breakers(
    env: &Env,
    asset: &Asset,
    breakers: &CircuitBreakerSet,
) -> Result<(), ContractError> {
    breakers
        .validate()
        .map_err(|_| ContractError::StorageError)?;
    write_breakers(env, asset, breakers)
}

fn write_breakers(
    env: &Env,
    asset: &Asset,
    breakers: &CircuitBreakerSet,
) -> Result<(), ContractError> {
    let bytes = postcard::to_allocvec(breakers).map_err(|_| ContractError::StorageError)?;
    let decoded: CircuitBreakerSet =
        postcard::from_bytes(&bytes).map_err(|_| ContractError::StorageError)?;
    decoded
        .validate()
        .map_err(|_| ContractError::StorageError)?;
    env.storage().persistent().set(
        &DataKey::Breakers(asset.clone()),
        &Bytes::from_slice(env, &bytes),
    );
    Ok(())
}

pub fn prepare_history_update(
    env: &Env,
    asset: &Asset,
    price: &NormalizedPrice,
    max_records: u32,
) -> HistoryUpdate {
    let key = DataKey::History(asset.clone());
    let mut history = env
        .storage()
        .persistent()
        .get::<_, Vec<NormalizedPrice>>(&key)
        .unwrap_or_else(|| Vec::new(env));
    if let Some(last) = history.get(history.len().saturating_sub(1)) {
        if price.timestamp <= last.timestamp {
            return HistoryUpdate::Unchanged(last);
        }
    }
    history.push_back(price.clone());
    while history.len() > max_records {
        history.pop_front();
    }
    HistoryUpdate::Append(PendingHistoryUpdate { key, history })
}

pub fn commit_history_update(env: &Env, update: PendingHistoryUpdate) {
    env.storage().persistent().set(&update.key, &update.history);
}

pub fn cache_price(env: &Env, asset: &Asset, cached: &CachedProxyPrice) {
    env.storage()
        .persistent()
        .set(&DataKey::Cache(asset.clone()), cached);
}

pub fn clear_history(env: &Env, asset: &Asset) {
    env.storage()
        .persistent()
        .remove(&DataKey::History(asset.clone()));
}
