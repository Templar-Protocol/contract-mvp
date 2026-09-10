//! Conversions between the Soroban surface types and the kernel's primitives.
//!
//! Sources speak SEP-40 `PriceData { i128, fixed-decimals }`; the kernel
//! speaks `Price { i64, expo }`; the main contract's cache + history speak
//! `NormalizedPrice { i64, expo, timestamp }`. Adapters scale `NormalizedPrice`
//! back to SEP-40 with their own per-adapter decimals.

use soroban_sdk::Env;
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::{
    proxy::{
        aggregator::{method::median::MedianLow, Aggregator},
        circuit_breaker::{
            CircuitBreaker, CumulativeChange, MonotonicRun, StepwiseChange, WindowedChangeDelta,
        },
        FreshnessFilter, Proxy, WeightedSource,
    },
    Price,
};
use templar_proxy_oracle_soroban_common::{
    CircuitBreakerConfig, ContractError, CumulativeChangeConfig as SorobanCumulativeChangeConfig,
    MonotonicRunConfig as SorobanMonotonicRunConfig, NormalizedPrice, PriceData, PriceFeedClient,
    ProxyConfig, StepwiseChangeConfig as SorobanStepwiseChangeConfig,
    WindowedChangeDeltaConfig as SorobanWindowedChangeDeltaConfig, MAX_SEP40_DECIMALS,
};

/// Convert a source feed's `PriceData` (decimal-prefixed i128) into the
/// kernel's `Price { i64, expo }` representation, downscaling if `value`
/// doesn't fit in i64.
pub fn source_price_to_kernel(
    source_price: PriceData,
    source_decimals: u32,
) -> Result<Price, ContractError> {
    if source_decimals > MAX_SEP40_DECIMALS {
        return Err(ContractError::InvalidInput);
    }
    let mut value = source_price.price;
    let mut expo = i32::try_from(source_decimals)
        .map_err(|_| ContractError::ConversionOverflow)?
        .checked_neg()
        .ok_or(ContractError::ConversionOverflow)?;
    while value > i128::from(i64::MAX) || value < i128::from(i64::MIN) {
        value /= 10;
        expo = expo
            .checked_add(1)
            .ok_or(ContractError::ConversionOverflow)?;
    }
    Ok(Price {
        price: i64::try_from(value).map_err(|_| ContractError::ConversionOverflow)?,
        conf: 0,
        expo,
        publish_time_ns: Nanoseconds::from_secs(source_price.timestamp),
    })
}

pub fn kernel_price_to_normalized(price: Price) -> NormalizedPrice {
    NormalizedPrice {
        mantissa: price.price,
        expo: price.expo,
        timestamp: price.publish_time_ns.as_secs(),
    }
}

pub fn circuit_breaker_from_config(
    config: CircuitBreakerConfig,
    baseline: Option<Price>,
) -> Result<CircuitBreaker, ContractError> {
    match config {
        CircuitBreakerConfig::StepwiseChange(SorobanStepwiseChangeConfig {
            max_relative_change,
        }) => {
            let max_relative_change = max_relative_change.to_decimal();
            if max_relative_change.is_zero() {
                return Err(ContractError::InvalidInput);
            }
            Ok(CircuitBreaker::StepwiseChange(StepwiseChange {
                max_relative_change,
            }))
        }
        CircuitBreakerConfig::MonotonicRun(SorobanMonotonicRunConfig {
            max_streak,
            min_relative_step_change,
        }) => {
            if max_streak == 0 {
                return Err(ContractError::InvalidInput);
            }
            let min_relative_step_change = min_relative_step_change.to_decimal();
            if min_relative_step_change.is_zero() {
                return Err(ContractError::InvalidInput);
            }
            Ok(CircuitBreaker::MonotonicRun(MonotonicRun {
                max_streak,
                min_relative_step_change,
            }))
        }
        CircuitBreakerConfig::WindowedChangeDelta(SorobanWindowedChangeDeltaConfig {
            window_len,
            lookback_windows,
            max_relative_mean_change,
        }) => {
            if window_len < 2 {
                return Err(ContractError::InvalidInput);
            }
            if lookback_windows == 0 {
                return Err(ContractError::InvalidInput);
            }
            let max_relative_mean_change = max_relative_mean_change.to_decimal();
            if max_relative_mean_change.is_zero() {
                return Err(ContractError::InvalidInput);
            }
            Ok(CircuitBreaker::WindowedChangeDelta(WindowedChangeDelta {
                window_len,
                lookback_windows,
                max_relative_mean_change,
            }))
        }
        CircuitBreakerConfig::CumulativeChange(SorobanCumulativeChangeConfig {
            max_relative_change,
        }) => {
            let max_relative_change = max_relative_change.to_decimal();
            if max_relative_change.is_zero() {
                return Err(ContractError::InvalidInput);
            }
            Ok(CircuitBreaker::CumulativeChange(CumulativeChange {
                baseline: baseline.ok_or(ContractError::InvalidInput)?,
                max_relative_change,
            }))
        }
    }
}

pub fn kernel_proxy_from_config(config: &ProxyConfig) -> Proxy<u32> {
    let mut median =
        MedianLow::new((0..config.sources.len()).map(|index| WeightedSource::new(index, 1)));
    median.min_sources = config.min_sources;
    Proxy::new(
        Aggregator::MedianLow(median),
        FreshnessFilter::new(
            config.max_age_secs.map(Nanoseconds::from_secs),
            config.max_clock_drift_secs.map(Nanoseconds::from_secs),
        ),
    )
}

pub fn validate_source_decimals(env: &Env, config: &ProxyConfig) -> Result<(), ContractError> {
    for source in config.sources.iter() {
        let decimals = PriceFeedClient::new(env, &source.oracle)
            .try_decimals()
            .map_err(|_| ContractError::InvalidInput)?
            .map_err(|_| ContractError::InvalidInput)?;
        if decimals > MAX_SEP40_DECIMALS {
            return Err(ContractError::InvalidInput);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_decimals_are_bounded_at_sep40_maximum() {
        let price = PriceData {
            price: 1,
            timestamp: 100,
        };
        assert!(source_price_to_kernel(price.clone(), 18).is_ok());
        assert_eq!(
            source_price_to_kernel(price, 19),
            Err(ContractError::InvalidInput)
        );
    }
}
