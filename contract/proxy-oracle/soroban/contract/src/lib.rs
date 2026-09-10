#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value;
// every `#[contractimpl]` method in this crate is an ABI entry point.
#![allow(clippy::needless_pass_by_value)]

extern crate alloc;

use alloc::vec::Vec as AllocVec;

use soroban_sdk::{contract, contractimpl, contracttype, Address, Bytes, BytesN, Env, Vec};
use stellar_access::ownable::{set_owner, Ownable};
use stellar_macros::only_owner;
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::{
    proxy::circuit_breaker::{
        CircuitBreakerEvent as KernelCircuitBreakerEvent, CircuitBreakerSet,
        CircuitBreakerSetConfig,
    },
    Price,
};
use templar_proxy_oracle_soroban_common::validate_proxy_config;
use templar_proxy_oracle_soroban_common::{
    extend_instance_ttl, extend_persistent_ttl, owner_upgrade,
};
pub use templar_proxy_oracle_soroban_common::{
    Asset, CircuitBreakerConfig, ContractError,
    CumulativeChangeConfig as SorobanCumulativeChangeConfig,
    MonotonicRunConfig as SorobanMonotonicRunConfig, NormalizedPrice, PriceData, PriceFeedClient,
    PriceFeedTrait, ProxyConfig, ProxyOracleClient, ProxyOracleMaintenanceTrait, ProxyOracleTrait,
    RearmConfig, RefreshStatus, SetEnforcedConfig, SorobanDecimal, SourceConfig,
    StepwiseChangeConfig as SorobanStepwiseChangeConfig,
    WindowedChangeDeltaConfig as SorobanWindowedChangeDeltaConfig, MAX_MANUAL_TRIP_METADATA_LEN,
};

pub type SorobanRearmConfig = RearmConfig;
pub type SorobanSetEnforcedConfig = SetEnforcedConfig;

mod codes;
mod conversion;
mod events;
mod refresh;
mod storage;

pub use events::{
    CacheBlocked, CircuitBreakerAdded, CircuitBreakerConfigSet, CircuitBreakerEnforcementSet,
    CircuitBreakerRearmed, CircuitBreakerRemoved, CircuitBreakerTripped, ContractUpgraded,
    ManualTripSet, ProxyRemoved, ProxySet, RefreshEvaluated, RefreshFailure, RefreshSuccess,
    TtlExtended,
};

use codes::breaker_error;
use conversion::{circuit_breaker_from_config, validate_source_decimals};
use refresh::{cached_accepted_no_older_than, refresh_one};
use storage::{
    add_asset, clear_history, invalidate_cache, load_assets, load_breakers, remove_asset,
    require_proxy_exists, store_breakers, DataKey,
};

pub(crate) const MAX_HISTORY_RECORDS: u32 = 32;
const MAX_BREAKERS_PER_PROXY: usize = 16;
pub(crate) const MAX_REGISTERED_ASSETS: u32 = 64;

// `RefreshFailure` / `CacheBlocked` event codes published as the `code` field.
pub(crate) const AGGREGATION_FAILED_CODE: u32 = 1;
pub(crate) const STORAGE_FAILED_CODE: u32 = 3;
pub(crate) const SOURCE_UNAVAILABLE_CODE: u32 = 5;
pub(crate) const UNKNOWN_ASSET_CODE: u32 = 6;

#[contract]
pub struct SorobanProxyOracle;

#[derive(Clone)]
#[contracttype]
pub enum CachedStatus {
    Accepted(NormalizedPrice),
    Blocked(u32),
    ResolveFailed(u32),
}

#[derive(Clone)]
#[contracttype]
pub struct CachedProxyPrice {
    pub updated_at: u64,
    pub status: CachedStatus,
}

#[derive(Clone)]
#[contracttype]
pub struct CircuitBreakerSetView {
    pub breaker_count: u32,
    pub next_id: u32,
    pub sample_interval_secs: u64,
    pub history_len: u32,
    pub is_manually_tripped: bool,
    pub is_blocking: bool,
}

fn with_breakers<T>(
    env: &Env,
    asset: &Asset,
    op: impl FnOnce(
        &mut CircuitBreakerSet,
    ) -> Result<(T, AllocVec<KernelCircuitBreakerEvent>), ContractError>,
) -> Result<(T, bool), ContractError> {
    extend_instance_ttl(env);
    require_proxy_exists(env, asset)?;
    let mut breakers = load_breakers(env, asset)?;
    let before = breakers.clone();
    let (result, events) = op(&mut breakers)?;
    if breakers == before {
        return Ok((result, false));
    }
    store_breakers(env, asset, &breakers)?;
    events::publish_breaker_events(env, asset, events);
    invalidate_cache(env, asset);
    Ok((result, true))
}

#[contractimpl]
impl SorobanProxyOracle {
    pub fn __constructor(env: Env, governance: Address, base: Asset) {
        extend_instance_ttl(&env);
        env.storage().instance().set(&DataKey::Base, &base);
        env.storage()
            .persistent()
            .set(&DataKey::Assets, &Vec::<Asset>::new(&env));
        set_owner(&env, &governance);
    }

    /// Owner-only runtime upgrade. Signature matches the OpenZeppelin
    /// `stellar-contract-utils::upgradeable::Upgradeable` trait's
    /// `fn upgrade(env, new_wasm_hash, operator)`. The contract isn't
    /// pulled in directly (see "Known Limits" in the soroban README for
    /// the toolchain × sdk version blocker), but matching the shape now
    /// means migrating to the trait later is a no-op at the ABI level.
    /// Does not accept a `migrate` payload — avoids widening the owner's
    /// authority surface beyond a typed code swap.
    pub fn upgrade(
        env: Env,
        new_wasm_hash: BytesN<32>,
        operator: Address,
    ) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        owner_upgrade(&env, &new_wasm_hash, &operator)?;
        ContractUpgraded { new_wasm_hash }.publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn set_proxy(env: Env, asset: Asset, config: ProxyConfig) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        validate_proxy_config(&config)?;
        validate_source_decimals(&env, &config)?;
        add_asset(&env, &asset)?;
        let storage = env.storage().persistent();
        let previous = storage.get::<_, ProxyConfig>(&DataKey::Proxy(asset.clone()));
        let config_changed = previous.as_ref() != Some(&config);
        let aggregation_changed = previous.as_ref().is_some_and(|current| {
            current.sources != config.sources || current.min_sources != config.min_sources
        });
        if aggregation_changed || !storage.has(&DataKey::Breakers(asset.clone())) {
            store_breakers(&env, &asset, &CircuitBreakerSet::empty())?;
        }
        storage.set(&DataKey::Proxy(asset.clone()), &config);
        if config_changed {
            invalidate_cache(&env, &asset);
            clear_history(&env, &asset);
        }
        ProxySet {
            asset,
            source_count: config.sources.len(),
            min_sources: config.min_sources,
        }
        .publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn remove_proxy(env: Env, asset: Asset) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        let storage = env.storage().persistent();
        storage.remove(&DataKey::Proxy(asset.clone()));
        storage.remove(&DataKey::Breakers(asset.clone()));
        storage.remove(&DataKey::History(asset.clone()));
        remove_asset(&env, &asset)?;
        invalidate_cache(&env, &asset);
        ProxyRemoved { asset }.publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn configure_breakers(
        env: Env,
        asset: Asset,
        sample_interval_secs: u64,
        history_len: u32,
    ) -> Result<(), ContractError> {
        if history_len == 0 || history_len > MAX_HISTORY_RECORDS {
            return Err(ContractError::InvalidInput);
        }
        with_breakers(&env, &asset, |breakers| {
            let outcome = breakers
                .set_config(CircuitBreakerSetConfig {
                    sample_interval_ns: Nanoseconds::from_secs(sample_interval_secs),
                    history_len,
                })
                .map_err(breaker_error)?;
            Ok(((), outcome.events))
        })
        .map(|(result, _)| result)
    }

    #[only_owner]
    pub fn add_breaker(
        env: Env,
        asset: Asset,
        breaker: CircuitBreakerConfig,
    ) -> Result<u32, ContractError> {
        let baseline = match &breaker {
            CircuitBreakerConfig::CumulativeChange(_) => {
                let proxy_config = env
                    .storage()
                    .persistent()
                    .get::<_, ProxyConfig>(&DataKey::Proxy(asset.clone()))
                    .ok_or(ContractError::InvalidInput)?;
                let max_age_secs = proxy_config
                    .max_age_secs
                    .ok_or(ContractError::InvalidInput)?;
                env.storage()
                    .persistent()
                    .get::<_, CachedProxyPrice>(&DataKey::Cache(asset.clone()))
                    .and_then(|cached| {
                        cached_accepted_no_older_than(
                            &cached,
                            max_age_secs,
                            env.ledger().timestamp(),
                        )
                    })
                    .filter(|price| price.mantissa > 0)
                    .map(|price| Price {
                        price: price.mantissa,
                        conf: 0,
                        expo: price.expo,
                        publish_time_ns: Nanoseconds::from_secs(price.timestamp),
                    })
            }
            CircuitBreakerConfig::StepwiseChange(_)
            | CircuitBreakerConfig::MonotonicRun(_)
            | CircuitBreakerConfig::WindowedChangeDelta(_) => None,
        };
        let breaker = circuit_breaker_from_config(breaker, baseline)?;
        with_breakers(&env, &asset, |breakers| {
            if breakers.breaker_count() >= MAX_BREAKERS_PER_PROXY {
                return Err(ContractError::TooManyBreakers);
            }
            let breaker_id = breakers.next_id();
            let outcome = breakers.add(breaker_id, breaker).map_err(breaker_error)?;
            Ok((breaker_id, outcome.events))
        })
        .map(|(result, _)| result)
    }

    #[only_owner]
    pub fn remove_breaker(env: Env, asset: Asset, breaker_id: u32) -> Result<(), ContractError> {
        with_breakers(&env, &asset, |breakers| {
            let outcome = breakers.remove(breaker_id).map_err(breaker_error)?;
            Ok(((), outcome.events))
        })
        .map(|(result, _)| result)
    }

    #[only_owner]
    pub fn rearm(
        env: Env,
        asset: Asset,
        breaker_id: u32,
        config: RearmConfig,
    ) -> Result<(), ContractError> {
        let armed_after_secs = env
            .ledger()
            .timestamp()
            .checked_add(config.arming_delay_secs)
            .ok_or(ContractError::InvalidInput)?;
        let armed_after_ns =
            Nanoseconds::checked_from_secs(armed_after_secs).ok_or(ContractError::InvalidInput)?;
        with_breakers(&env, &asset, |breakers| {
            let outcome = breakers
                .rearm(breaker_id, armed_after_ns)
                .map_err(breaker_error)?;
            Ok(((), outcome.events))
        })
        .map(|(result, _)| result)
    }

    #[only_owner]
    pub fn set_enforced(
        env: Env,
        asset: Asset,
        breaker_id: u32,
        config: SetEnforcedConfig,
    ) -> Result<(), ContractError> {
        with_breakers(&env, &asset, |breakers| {
            let outcome = breakers
                .set_enforced(breaker_id, config.is_enforced)
                .map_err(breaker_error)?;
            Ok(((), outcome.events))
        })
        .map(|(result, _)| result)
    }

    #[only_owner]
    pub fn set_manual_trip(
        env: Env,
        asset: Asset,
        is_manually_tripped: bool,
        metadata: Option<Bytes>,
    ) -> Result<(), ContractError> {
        if metadata
            .as_ref()
            .is_some_and(|m| m.len() as usize > MAX_MANUAL_TRIP_METADATA_LEN)
        {
            return Err(ContractError::InvalidInput);
        }
        let kernel_metadata = metadata.as_ref().map(Bytes::to_alloc_vec);
        let ((), did_mutate) = with_breakers(&env, &asset, |breakers| {
            use templar_proxy_oracle_kernel::primitive::AccountId as KernelAccountId;
            let outcome = breakers.set_manual_trip(
                is_manually_tripped,
                KernelAccountId::from_bytes([0_u8; 64]),
                kernel_metadata,
            );
            Ok(((), outcome.events))
        })?;
        if !did_mutate {
            return Ok(());
        }
        ManualTripSet {
            asset,
            is_manually_tripped,
            metadata,
        }
        .publish(&env);
        Ok(())
    }

    pub fn get_proxy(env: Env, asset: Asset) -> Option<ProxyConfig> {
        env.storage().persistent().get(&DataKey::Proxy(asset))
    }

    pub fn get_cached(env: Env, asset: Asset) -> Option<CachedProxyPrice> {
        env.storage().persistent().get(&DataKey::Cache(asset))
    }

    pub fn get_breaker_set_view(env: Env, asset: Asset) -> Option<CircuitBreakerSetView> {
        if !env
            .storage()
            .persistent()
            .has(&DataKey::Proxy(asset.clone()))
        {
            return None;
        }
        let breakers = load_breakers(&env, &asset).ok()?;
        Some(CircuitBreakerSetView {
            breaker_count: u32::try_from(breakers.breaker_count()).ok()?,
            next_id: breakers.next_id(),
            sample_interval_secs: breakers.sample_interval_ns().as_secs(),
            history_len: breakers.accepted_history().capacity(),
            is_manually_tripped: breakers.is_manually_tripped(),
            is_blocking: breakers.is_blocking(),
        })
    }
}

#[contractimpl]
impl ProxyOracleMaintenanceTrait for SorobanProxyOracle {
    fn refresh(env: Env, asset: Asset) -> RefreshStatus {
        extend_instance_ttl(&env);
        refresh_one(&env, asset)
    }

    fn extend_ttl(env: Env, asset: Asset) -> Result<(), ContractError> {
        extend_instance_ttl(&env);
        require_proxy_exists(&env, &asset)?;
        extend_persistent_ttl(&env, &DataKey::Assets);
        extend_persistent_ttl(&env, &DataKey::Proxy(asset.clone()));
        extend_persistent_ttl(&env, &DataKey::Breakers(asset.clone()));
        extend_persistent_ttl(&env, &DataKey::Cache(asset.clone()));
        extend_persistent_ttl(&env, &DataKey::History(asset.clone()));
        TtlExtended { asset }.publish(&env);
        Ok(())
    }
}

/// Owner/governance surface is delegated to `stellar_access::ownable`, which
/// exposes `get_owner`, two-step `transfer_ownership`/`accept_ownership`, and
/// `renounce_ownership` via the standard `Ownable` trait. We re-export those
/// methods on the contract's client by exposing the trait's default
/// implementations through `#[contractimpl(contracttrait)]`.
#[contractimpl(contracttrait)]
impl Ownable for SorobanProxyOracle {}

/// Read API for `Sep40Adapter` contracts. The proxy oracle does not
/// implement SEP-40; adapters scale `NormalizedPrice` to their own
/// per-adapter decimals + resolution + base.
#[contractimpl]
impl ProxyOracleTrait for SorobanProxyOracle {
    fn aggregated_latest(env: Env, asset: Asset) -> Option<NormalizedPrice> {
        let cached = env
            .storage()
            .persistent()
            .get::<_, CachedProxyPrice>(&DataKey::Cache(asset.clone()))?;
        let proxy_config = env
            .storage()
            .persistent()
            .get::<_, ProxyConfig>(&DataKey::Proxy(asset))?;
        let max_age = proxy_config.max_age_secs?;
        cached_accepted_no_older_than(&cached, max_age, env.ledger().timestamp())
    }

    fn aggregated_history(env: Env, asset: Asset, records: u32) -> Option<Vec<NormalizedPrice>> {
        if records == 0 {
            return None;
        }
        let breakers = load_breakers(&env, &asset).ok()?;
        if breakers.is_blocking() {
            return None;
        }
        let history = env
            .storage()
            .persistent()
            .get::<_, Vec<NormalizedPrice>>(&DataKey::History(asset))?;
        if history.is_empty() {
            return None;
        }
        let start = history.len().saturating_sub(records);
        Some(history.slice(start..))
    }

    fn source_base(env: Env) -> Option<Asset> {
        env.storage().instance().get(&DataKey::Base)
    }
}

/// Admin / introspection helpers — deliberately named to avoid collision with
/// SEP-40's `base()` / `assets()`, since these mean different things here.
#[contractimpl]
impl SorobanProxyOracle {
    /// Assets with a registered proxy config. Used by off-chain indexers
    /// and adapter deployer tooling.
    pub fn registered_assets(env: Env) -> Result<Vec<Asset>, ContractError> {
        load_assets(&env)
    }
}

#[cfg(test)]
mod tests;
