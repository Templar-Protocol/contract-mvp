#![no_std]
// Soroban contract entry points require `env: Env` and `Address` by value.
#![allow(clippy::needless_pass_by_value)]

//! Pyth Lazer as a SEP-40 source for the Soroban proxy oracle.
//!
//! Pyth's on-chain verifier is stateless: `verify_update` proves a payload was
//! signed by a trusted signer and hands back the bytes, with no replay
//! protection, ordering, or freshness check. This contract owns all of that —
//! a channel filter, a payload freshness window, a per-asset strictly advancing
//! publish time — stores one price per mapped asset, and re-exposes it through
//! SEP-40 so the proxy oracle's `refresh` can pull it like any other source.

use pyth_lazer_stellar_sdk::{Channel, PythLazerClient};
use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, symbol_short, Address,
    Bytes, BytesN, Env, Map, Symbol, Vec,
};
use stellar_access::ownable::{set_owner, Ownable};
use stellar_macros::only_owner;
use templar_proxy_oracle_soroban_common::{
    extend_instance_ttl, extend_persistent_ttl, normalized_to_sep40, owner_upgrade, Asset,
    ContractError, NormalizedPrice, PriceData, PriceFeedTrait, DEFAULT_TTL_EXTEND_TO,
    DEFAULT_TTL_THRESHOLD, MAX_SEP40_DECIMALS,
};

#[cfg(any(test, feature = "testutils"))]
pub mod testutils;

pub const MAX_FEED_MAPPINGS: u32 = 64;
pub const MICROS_PER_SEC: u64 = 1_000_000;

const CONFIG: Symbol = symbol_short!("CONFIG");
const FEEDS: Symbol = symbol_short!("FEEDS");

soroban_sdk::contractmeta!(key = "sep", val = "40");

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum LazerSourceError {
    InvalidInput = 1,
    Unauthorized = 2,
    InvalidPayload = 3,
    ChannelMismatch = 4,
    PayloadTooOld = 5,
    PayloadTooFarAhead = 6,
    TooManyMappings = 7,
    DuplicateMapping = 8,
    UnknownFeed = 9,
}

impl From<ContractError> for LazerSourceError {
    fn from(error: ContractError) -> Self {
        match error {
            ContractError::Unauthorized => Self::Unauthorized,
            _ => Self::InvalidInput,
        }
    }
}

#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LazerChannel {
    RealTime,
    FixedRate50ms,
    FixedRate200ms,
    FixedRate1000ms,
}

impl From<&Channel> for LazerChannel {
    fn from(channel: &Channel) -> Self {
        match channel {
            Channel::RealTime => Self::RealTime,
            Channel::FixedRate50ms => Self::FixedRate50ms,
            Channel::FixedRate200ms => Self::FixedRate200ms,
            Channel::FixedRate1000ms => Self::FixedRate1000ms,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedMapping {
    pub feed_id: u32,
    pub asset: Asset,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreshnessConfig {
    /// Reject a payload, or skip a feed, whose timestamp is more than this many seconds old.
    pub max_age_secs: u64,
    /// Reject a payload, or skip a feed, whose timestamp is more than this many seconds ahead of the ledger.
    pub max_ahead_secs: u64,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub verifier: Address,
    pub base: Asset,
    pub decimals: u32,
    pub channel: LazerChannel,
    pub freshness: FreshnessConfig,
}

/// Raw stored feed: mantissa × 10^expo, microsecond publish time.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPrice {
    pub mantissa: i64,
    pub expo: i32,
    pub publish_time_us: u64,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Price(Asset),
}

#[contractevent]
#[derive(Clone)]
pub struct PriceUpdated {
    #[topic]
    pub asset: Asset,
    pub feed_id: u32,
    pub mantissa: i64,
    pub expo: i32,
    pub publish_time_us: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct FeedMappingAdded {
    #[topic]
    pub feed_id: u32,
    pub asset: Asset,
}

#[contractevent]
#[derive(Clone)]
pub struct FeedMappingRemoved {
    #[topic]
    pub feed_id: u32,
    pub asset: Asset,
}

#[contractevent]
#[derive(Clone)]
pub struct FreshnessUpdated {
    pub max_age_secs: u64,
    pub max_ahead_secs: u64,
}

#[contractevent]
#[derive(Clone)]
pub struct DecimalsUpdated {
    pub decimals: u32,
}

#[contractevent]
#[derive(Clone)]
pub struct SourceUpgraded {
    pub new_wasm_hash: BytesN<32>,
}

#[contract]
pub struct PythLazerSource;

#[contractimpl]
impl PythLazerSource {
    pub fn __constructor(
        env: Env,
        owner: Address,
        config: Config,
        feed_mappings: Vec<FeedMapping>,
    ) -> Result<(), LazerSourceError> {
        if config.decimals > MAX_SEP40_DECIMALS {
            return Err(LazerSourceError::InvalidInput);
        }
        validate_freshness(&config.freshness)?;
        let mut feeds: Map<u32, Asset> = Map::new(&env);
        for mapping in feed_mappings.iter() {
            insert_mapping(&mut feeds, &mapping)?;
        }
        extend_instance_ttl(&env);
        env.storage().instance().set(&CONFIG, &config);
        env.storage().instance().set(&FEEDS, &feeds);
        set_owner(&env, &owner);
        Ok(())
    }

    /// Verify a signed Lazer payload through the configured verifier and store
    /// every mapped feed whose own update time is inside the freshness window
    /// and strictly advances. Permissionless: authenticity is cryptographic.
    /// Returns the number of feeds stored.
    pub fn update_price_feeds(env: Env, payload: Bytes) -> Result<u32, LazerSourceError> {
        extend_instance_ttl(&env);
        let config = load_config(&env);
        let update = PythLazerClient::new(&env, &config.verifier)
            .verify_update(&payload)
            .map_err(|_| LazerSourceError::InvalidPayload)?;
        if LazerChannel::from(&update.channel) != config.channel {
            return Err(LazerSourceError::ChannelMismatch);
        }
        let now = env.ledger().timestamp();
        let freshness = &config.freshness;
        let oldest_allowed_secs = now.saturating_sub(freshness.max_age_secs);
        let latest_allowed_secs = now.saturating_add(freshness.max_ahead_secs);
        let payload_secs = update.timestamp / MICROS_PER_SEC;
        if payload_secs < oldest_allowed_secs {
            return Err(LazerSourceError::PayloadTooOld);
        }
        if payload_secs > latest_allowed_secs {
            return Err(LazerSourceError::PayloadTooFarAhead);
        }

        let feeds = load_feeds(&env);
        let mut stored = 0;
        for feed in &update.feeds {
            let Some(asset) = feeds.get(feed.feed_id) else {
                continue;
            };
            let (Some(mantissa), Some(exponent), Some(publish_time_us)) =
                (feed.price, feed.exponent, feed.feed_update_timestamp)
            else {
                continue;
            };
            let publish_secs = publish_time_us / MICROS_PER_SEC;
            if mantissa <= 0
                || publish_secs < oldest_allowed_secs
                || publish_secs > latest_allowed_secs
            {
                continue;
            }
            let key = DataKey::Price(asset.clone());
            let advances = env
                .storage()
                .persistent()
                .get::<_, StoredPrice>(&key)
                .is_none_or(|existing| publish_time_us > existing.publish_time_us);
            if !advances {
                continue;
            }
            let price = StoredPrice {
                mantissa,
                expo: i32::from(exponent),
                publish_time_us,
            };
            env.storage().persistent().set(&key, &price);
            env.storage().persistent().extend_ttl(
                &key,
                DEFAULT_TTL_THRESHOLD,
                DEFAULT_TTL_EXTEND_TO,
            );
            PriceUpdated {
                asset,
                feed_id: feed.feed_id,
                mantissa,
                expo: price.expo,
                publish_time_us,
            }
            .publish(&env);
            stored += 1;
        }
        Ok(stored)
    }

    #[only_owner]
    pub fn add_feed(env: Env, mapping: FeedMapping) -> Result<(), LazerSourceError> {
        extend_instance_ttl(&env);
        let mut feeds = load_feeds(&env);
        insert_mapping(&mut feeds, &mapping)?;
        env.storage().instance().set(&FEEDS, &feeds);
        FeedMappingAdded {
            feed_id: mapping.feed_id,
            asset: mapping.asset,
        }
        .publish(&env);
        Ok(())
    }

    /// Unmap a feed and drop its stored price.
    #[only_owner]
    pub fn remove_feed(env: Env, feed_id: u32) -> Result<(), LazerSourceError> {
        extend_instance_ttl(&env);
        let mut feeds = load_feeds(&env);
        let asset = feeds.get(feed_id).ok_or(LazerSourceError::UnknownFeed)?;
        feeds.remove(feed_id);
        env.storage().instance().set(&FEEDS, &feeds);
        env.storage()
            .persistent()
            .remove(&DataKey::Price(asset.clone()));
        FeedMappingRemoved { feed_id, asset }.publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn set_freshness(env: Env, freshness: FreshnessConfig) -> Result<(), LazerSourceError> {
        validate_freshness(&freshness)?;
        extend_instance_ttl(&env);
        let mut config = load_config(&env);
        config.freshness = freshness.clone();
        env.storage().instance().set(&CONFIG, &config);
        FreshnessUpdated {
            max_age_secs: freshness.max_age_secs,
            max_ahead_secs: freshness.max_ahead_secs,
        }
        .publish(&env);
        Ok(())
    }

    #[only_owner]
    pub fn set_decimals(env: Env, decimals: u32) -> Result<(), LazerSourceError> {
        if decimals > MAX_SEP40_DECIMALS {
            return Err(LazerSourceError::InvalidInput);
        }
        extend_instance_ttl(&env);
        let mut config = load_config(&env);
        config.decimals = decimals;
        env.storage().instance().set(&CONFIG, &config);
        DecimalsUpdated { decimals }.publish(&env);
        Ok(())
    }

    /// Signature matches the OpenZeppelin `Upgradeable` trait shape.
    pub fn upgrade(
        env: Env,
        new_wasm_hash: BytesN<32>,
        operator: Address,
    ) -> Result<(), LazerSourceError> {
        owner_upgrade(&env, &new_wasm_hash, &operator)?;
        extend_instance_ttl(&env);
        SourceUpgraded { new_wasm_hash }.publish(&env);
        Ok(())
    }

    /// Permissionless: renews instance state and every stored price.
    pub fn extend_ttl(env: Env) {
        extend_instance_ttl(&env);
        let feeds = load_feeds(&env);
        for asset in feeds.values().iter() {
            extend_persistent_ttl(&env, &DataKey::Price(asset));
        }
    }

    pub fn config(env: Env) -> Option<Config> {
        extend_instance_ttl(&env);
        env.storage().instance().get(&CONFIG)
    }

    pub fn feed_mappings(env: Env) -> Vec<FeedMapping> {
        extend_instance_ttl(&env);
        let feeds = load_feeds(&env);
        let mut mappings = Vec::new(&env);
        for (feed_id, asset) in feeds.iter() {
            mappings.push_back(FeedMapping { feed_id, asset });
        }
        mappings
    }

    pub fn stored_price(env: Env, asset: Asset) -> Option<StoredPrice> {
        env.storage().persistent().get(&DataKey::Price(asset))
    }
}

#[contractimpl(contracttrait)]
impl Ownable for PythLazerSource {}

/// Only the latest price per asset is retained, at second precision so the
/// proxy oracle's freshness filter sees the exact publish time: `resolution`
/// is 1, `price` answers only for that exact second, `prices` has one record.
#[contractimpl]
impl PriceFeedTrait for PythLazerSource {
    fn base(env: Env) -> Asset {
        extend_instance_ttl(&env);
        load_config(&env).base
    }

    fn assets(env: Env) -> Vec<Asset> {
        extend_instance_ttl(&env);
        load_feeds(&env).values()
    }

    fn decimals(env: Env) -> u32 {
        extend_instance_ttl(&env);
        load_config(&env).decimals
    }

    fn resolution(_env: Env) -> u32 {
        1
    }

    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData> {
        Self::lastprice(env, asset).filter(|price| price.timestamp == timestamp)
    }

    fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>> {
        if records == 0 {
            return None;
        }
        let price = Self::lastprice(env.clone(), asset)?;
        Some(Vec::from_array(&env, [price]))
    }

    fn lastprice(env: Env, asset: Asset) -> Option<PriceData> {
        extend_instance_ttl(&env);
        let stored: StoredPrice = env.storage().persistent().get(&DataKey::Price(asset))?;
        let normalized = NormalizedPrice {
            mantissa: stored.mantissa,
            expo: stored.expo,
            timestamp: stored.publish_time_us / MICROS_PER_SEC,
        };
        normalized_to_sep40(&normalized, load_config(&env).decimals).ok()
    }
}

fn validate_freshness(freshness: &FreshnessConfig) -> Result<(), LazerSourceError> {
    if freshness.max_age_secs == 0 {
        return Err(LazerSourceError::InvalidInput);
    }
    Ok(())
}

fn insert_mapping(
    feeds: &mut Map<u32, Asset>,
    mapping: &FeedMapping,
) -> Result<(), LazerSourceError> {
    if feeds.len() >= MAX_FEED_MAPPINGS {
        return Err(LazerSourceError::TooManyMappings);
    }
    if feeds.contains_key(mapping.feed_id) || feeds.values().contains(&mapping.asset) {
        return Err(LazerSourceError::DuplicateMapping);
    }
    feeds.set(mapping.feed_id, mapping.asset.clone());
    Ok(())
}

#[allow(clippy::expect_used)]
fn load_config(env: &Env) -> Config {
    env.storage().instance().get(&CONFIG).expect("CONFIG")
}

#[allow(clippy::expect_used)]
fn load_feeds(env: &Env) -> Map<u32, Asset> {
    env.storage().instance().get(&FEEDS).expect("FEEDS")
}

#[cfg(test)]
mod tests;
