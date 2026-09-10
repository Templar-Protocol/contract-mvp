#![no_std]

use soroban_sdk::{
    contractclient, contracterror, contracttype, Address, BytesN, Env, IntoVal, Symbol, Val, Vec,
};
use stellar_access::ownable::get_owner;
use templar_primitives::Decimal;

pub const DEFAULT_TTL_THRESHOLD: u32 = 518_400;
pub const DEFAULT_TTL_EXTEND_TO: u32 = 3_110_400;
pub const MAX_MANUAL_TRIP_METADATA_LEN: usize = 1024;
/// Largest SEP-40 `decimals` a price can be re-scaled to without overflowing i128 arithmetic.
pub const MAX_SEP40_DECIMALS: u32 = 18;

pub fn extend_instance_ttl(env: &Env) {
    env.storage()
        .instance()
        .extend_ttl(DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
}

pub fn extend_persistent_ttl<K: IntoVal<Env, Val>>(env: &Env, key: &K) {
    let storage = env.storage().persistent();
    if storage.has(key) {
        storage.extend_ttl(key, DEFAULT_TTL_THRESHOLD, DEFAULT_TTL_EXTEND_TO);
    }
}

/// Owner-gated wasm swap in the OpenZeppelin `Upgradeable` shape
/// (`upgrade(env, new_wasm_hash, operator)`); the caller publishes its own event.
pub fn owner_upgrade(
    env: &Env,
    new_wasm_hash: &BytesN<32>,
    operator: &Address,
) -> Result<(), ContractError> {
    operator.require_auth();
    if get_owner(env).as_ref() != Some(operator) {
        return Err(ContractError::Unauthorized);
    }
    if is_zero_wasm_hash(new_wasm_hash) {
        return Err(ContractError::InvalidInput);
    }
    env.deployer()
        .update_current_contract_wasm(new_wasm_hash.clone());
    Ok(())
}

/// SEP-40 timestamp bucketing: the start of the `resolution`-wide window holding `timestamp`.
#[must_use]
pub fn bucket_timestamp(timestamp: u64, resolution: u32) -> u64 {
    timestamp - (timestamp % u64::from(resolution))
}

/// Returns true when `wasm_hash` is all zero bytes.
///
/// Implemented without `Env` so it can be used anywhere `BytesN<32>` is
/// available, including in pure validation helpers.
#[must_use]
pub fn is_zero_wasm_hash(wasm_hash: &BytesN<32>) -> bool {
    wasm_hash.to_array() == [0_u8; 32]
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum Asset {
    Stellar(Address),
    Other(Symbol),
}

/// Decimals-free price representation used internally and across the
/// proxy-oracle ↔ SEP-40 adapter boundary. Adapters convert this to
/// SEP-40 `PriceData` using their own configured decimals.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct NormalizedPrice {
    pub mantissa: i64,
    pub expo: i32,
    pub timestamp: u64,
}

/// Outcome of one runtime `refresh(asset)` call.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum RefreshStatus {
    Accepted(NormalizedPrice),
    Blocked(u32),
    ResolveFailed(u32),
    UnknownAsset,
    SourceUnavailable,
}

/// SEP-40 `PriceFeed` trait. Implemented by external price sources the
/// main proxy oracle consumes, and by the `Sep40Adapter` contracts that
/// re-expose the proxy oracle's normalized prices in SEP-40 form.
#[contractclient(name = "PriceFeedClient")]
pub trait PriceFeedTrait {
    fn base(env: Env) -> Asset;
    fn assets(env: Env) -> Vec<Asset>;
    fn decimals(env: Env) -> u32;
    fn resolution(env: Env) -> u32;
    fn price(env: Env, asset: Asset, timestamp: u64) -> Option<PriceData>;
    fn prices(env: Env, asset: Asset, records: u32) -> Option<Vec<PriceData>>;
    fn lastprice(env: Env, asset: Asset) -> Option<PriceData>;
}

/// Read API exposed by the proxy oracle runtime and consumed by `Sep40Adapter`
/// contracts. Returns kernel-form prices (`NormalizedPrice { mantissa, expo,
/// timestamp }`) post-aggregation; adapters re-scale to their own SEP-40
/// fixed decimals.
///
/// The proxy oracle owns the freshness check (`max_age_secs` from
/// `ProxyConfig`); `aggregated_latest` already applies it before returning.
#[contractclient(name = "ProxyOracleClient")]
pub trait ProxyOracleTrait {
    /// Latest aggregated price for `asset`, post-freshness-check. Returns
    /// `None` if no proxy is registered, the cache is empty / not accepted,
    /// or the cached entry is older than the configured `max_age_secs`.
    fn aggregated_latest(env: Env, asset: Asset) -> Option<NormalizedPrice>;
    /// Last `records` aggregated prices for `asset`, oldest first. Does not
    /// apply a freshness filter; callers that care about staleness should
    /// inspect the returned timestamps.
    fn aggregated_history(env: Env, asset: Asset, records: u32) -> Option<Vec<NormalizedPrice>>;
    fn source_base(env: Env) -> Option<Asset>;
}

/// Permissionless maintenance surface of the proxy oracle runtime, safe to fan
/// out from a batcher.
#[contractclient(name = "ProxyOracleMaintenanceClient")]
pub trait ProxyOracleMaintenanceTrait {
    fn refresh(env: Env, asset: Asset) -> RefreshStatus;
    fn extend_ttl(env: Env, asset: Asset) -> Result<(), ContractError>;
}

/// Convert a normalized exponent-form price to SEP-40 `PriceData` with the
/// given target decimals. Used by `Sep40Adapter` contracts.
pub fn normalized_to_sep40(
    price: &NormalizedPrice,
    decimals: u32,
) -> Result<PriceData, ContractError> {
    let decimals = i32::try_from(decimals).map_err(|_| ContractError::ConversionOverflow)?;
    let scale = decimals
        .checked_add(price.expo)
        .ok_or(ContractError::ConversionOverflow)?;
    let mut value = i128::from(price.mantissa);
    if scale >= 0 {
        value = value
            .checked_mul(
                10_i128
                    .checked_pow(scale.unsigned_abs())
                    .ok_or(ContractError::ConversionOverflow)?,
            )
            .ok_or(ContractError::ConversionOverflow)?;
    } else {
        let divisor = 10_i128
            .checked_pow(scale.unsigned_abs())
            .ok_or(ContractError::ConversionOverflow)?;
        let scaled = value / divisor;
        if value != 0 && scaled == 0 {
            return Err(ContractError::ConversionOverflow);
        }
        value = scaled;
    }
    Ok(PriceData {
        price: value,
        timestamp: price.timestamp,
    })
}

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ContractError {
    AlreadyInitialized = 1,
    MissingConfig = 2,
    Unauthorized = 3,
    InvalidInput = 4,
    StorageError = 5,
    SourceUnavailable = 6,
    ResolveFailed = 7,
    ConversionOverflow = 8,
    TooManySources = 9,
    TooManyBreakers = 10,
    BreakerError = 11,
    TooManyAssets = 12,
    TooFewSources = 13,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct SourceConfig {
    pub oracle: Address,
    pub asset: Asset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct ProxyConfig {
    pub sources: Vec<SourceConfig>,
    pub min_sources: u32,
    pub max_age_secs: Option<u64>,
    pub max_clock_drift_secs: Option<u64>,
}

pub const MAX_SOURCES_PER_PROXY: u32 = 16;
pub const MAX_SOURCE_AGE_SECS: u64 = 7 * 24 * 60 * 60;
pub const MAX_CLOCK_DRIFT_SECS: u64 = 60 * 60;

pub fn validate_proxy_config(config: &ProxyConfig) -> Result<(), ContractError> {
    if config.sources.len() < 3 {
        return Err(ContractError::TooFewSources);
    }
    if config.sources.len() > MAX_SOURCES_PER_PROXY {
        return Err(ContractError::TooManySources);
    }
    if config.min_sources < 3 || config.min_sources > config.sources.len() {
        return Err(ContractError::InvalidInput);
    }
    let (Some(max_age_secs), Some(max_clock_drift_secs)) =
        (config.max_age_secs, config.max_clock_drift_secs)
    else {
        return Err(ContractError::InvalidInput);
    };
    if max_age_secs > MAX_SOURCE_AGE_SECS || max_clock_drift_secs > MAX_CLOCK_DRIFT_SECS {
        return Err(ContractError::InvalidInput);
    }
    for i in 0..config.sources.len() {
        let source = config.sources.get(i).ok_or(ContractError::InvalidInput)?;
        for j in (i + 1)..config.sources.len() {
            if source.oracle
                == config
                    .sources
                    .get(j)
                    .ok_or(ContractError::InvalidInput)?
                    .oracle
            {
                return Err(ContractError::InvalidInput);
            }
        }
    }
    Ok(())
}

/// Soroban wire format for a 512-bit `Decimal`. Wrapping `BytesN<64>` (the
/// 8 × `u64` words of `Decimal::as_repr` laid out little-endian) gives a
/// compile-time length guarantee at the contract boundary — unlike a raw
/// `Vec<u64>` field, which could arrive empty, short, or oversized and
/// would have to be re-validated by every consumer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct SorobanDecimal(pub BytesN<64>);

impl SorobanDecimal {
    // `From<Decimal> for SorobanDecimal` would be the natural counterpart to
    // the `From<&SorobanDecimal> for Decimal` impl below, but construction
    // needs the `Env` (to allocate the `BytesN<64>` in host memory) and the
    // `From` trait doesn't admit a context parameter — so this stays as a
    // method.
    #[must_use]
    pub fn from_decimal(env: &Env, decimal: Decimal) -> Self {
        // SAFETY: `[u64; 8]` and `[u8; 64]` are POD types of equal size with
        // no invalid bit patterns. `u64::to_le` forces little-endian byte
        // layout regardless of target (no-op on LE, byte-swap on BE), so
        // the wire format is platform-independent.
        let words = decimal.as_repr().map(u64::to_le);
        let bytes: [u8; 64] = unsafe { core::mem::transmute(words) };
        Self(BytesN::from_array(env, &bytes))
    }

    /// Convenience for sites where `.into()` would force the caller to
    /// annotate the destination type (e.g. `let d = sd.to_decimal();` reads
    /// cleaner than `let d: Decimal = sd.into();`). Equivalent to
    /// `Decimal::from(&self)`.
    #[must_use]
    pub fn to_decimal(&self) -> Decimal {
        Decimal::from(self)
    }
}

impl From<&SorobanDecimal> for Decimal {
    fn from(value: &SorobanDecimal) -> Self {
        // SAFETY: see `SorobanDecimal::from_decimal`.
        let raw: [u64; 8] = unsafe { core::mem::transmute(value.0.to_array()) };
        Self::from_repr(raw.map(u64::from_le))
    }
}

impl From<SorobanDecimal> for Decimal {
    fn from(value: SorobanDecimal) -> Self {
        Self::from(&value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct StepwiseChangeConfig {
    pub max_relative_change: SorobanDecimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct MonotonicRunConfig {
    pub max_streak: u32,
    pub min_relative_step_change: SorobanDecimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct WindowedChangeDeltaConfig {
    pub window_len: u32,
    pub lookback_windows: u32,
    pub max_relative_mean_change: SorobanDecimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct CumulativeChangeConfig {
    pub max_relative_change: SorobanDecimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum CircuitBreakerConfig {
    StepwiseChange(StepwiseChangeConfig),
    MonotonicRun(MonotonicRunConfig),
    WindowedChangeDelta(WindowedChangeDeltaConfig),
    CumulativeChange(CumulativeChangeConfig),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct SetEnforcedConfig {
    pub is_enforced: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub struct RearmConfig {
    pub arming_delay_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use soroban_sdk::Env;

    /// Cover the unsafe `transmute` path in `SorobanDecimal` end-to-end:
    /// every value round-trips through `to_decimal()` and both `From` impls.
    #[rstest]
    #[case::zero(Decimal::ZERO)]
    #[case::one_half(Decimal::ONE_HALF)]
    #[case::one(Decimal::ONE)]
    #[case::two(Decimal::TWO)]
    #[case::ln2(Decimal::LN2)]
    #[case::e(Decimal::E)]
    #[case::max(Decimal::MAX)]
    fn soroban_decimal_round_trips(#[case] original: Decimal) {
        let env = Env::default();
        let wrapped = SorobanDecimal::from_decimal(&env, original);
        assert_eq!(wrapped.to_decimal(), original, "to_decimal");
        assert_eq!(Decimal::from(&wrapped), original, "From<&SorobanDecimal>");
        assert_eq!(Decimal::from(wrapped), original, "From<SorobanDecimal>");
    }

    #[test]
    fn normalized_to_sep40_preserves_only_representable_values() {
        let price = |mantissa, expo| NormalizedPrice {
            mantissa,
            expo,
            timestamp: 100,
        };

        assert_eq!(
            normalized_to_sep40(&price(1, -1), 0),
            Err(ContractError::ConversionOverflow)
        );
        assert_eq!(
            normalized_to_sep40(&price(-1, -1), 0),
            Err(ContractError::ConversionOverflow)
        );
        assert_eq!(normalized_to_sep40(&price(10, -1), 0).unwrap().price, 1);
        assert_eq!(normalized_to_sep40(&price(0, -1), 0).unwrap().price, 0);
        assert_eq!(
            normalized_to_sep40(&price(i64::MAX, 120), 18),
            Err(ContractError::ConversionOverflow)
        );
        assert_eq!(
            normalized_to_sep40(&price(500_000, -4), 8).unwrap().price,
            5_000_000_000
        );
    }
}
