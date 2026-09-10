//! The Pyth Lazer source contract standing in as one of the runtime's three
//! SEP-40 upstreams, driven end to end: push a payload → `refresh` → adapter
//! `lastprice`; plus the batcher fanning `refresh` / `extend_ttl` across assets.

use soroban_sdk::{testutils::Ledger as _, Address, Bytes, Env, Symbol, Vec};
use templar_proxy_oracle_soroban_batcher_contract::{ProxyOracleBatcher, ProxyOracleBatcherClient};
use templar_proxy_oracle_soroban_common::{Asset, ProxyConfig, RefreshStatus, SourceConfig};
use templar_proxy_oracle_soroban_governance_common::GovernanceAction;
use templar_proxy_oracle_soroban_integration_tests::common::Bootstrap;
use templar_proxy_oracle_soroban_pyth_lazer_source_contract::{
    testutils::{payload_at, MockVerifier, MICROS_PER_SEC},
    Config, FeedMapping, FreshnessConfig, LazerChannel, PythLazerSource, PythLazerSourceClient,
};

const BTC_FEED: u32 = 1;

fn btc_payload(env: &Env, publish_secs: u64, mantissa: i64) -> Bytes {
    payload_at(env, publish_secs * MICROS_PER_SEC, &[(BTC_FEED, mantissa)])
}

struct Wired {
    b: Bootstrap,
    source: PythLazerSourceClient<'static>,
}

/// Runtime configured with two mock upstreams plus the real Lazer source.
fn wired() -> Wired {
    let b = Bootstrap::new();
    let config = Config {
        verifier: b.env.register(MockVerifier, ()),
        base: b.base_usd.clone(),
        decimals: 8,
        channel: LazerChannel::FixedRate200ms,
        freshness: FreshnessConfig {
            max_age_secs: 120,
            max_ahead_secs: 5,
        },
    };
    let mappings = Vec::from_array(
        &b.env,
        [FeedMapping {
            feed_id: BTC_FEED,
            asset: b.asset_btc.clone(),
        }],
    );
    let source_id = b
        .env
        .register(PythLazerSource, (&b.admin, config, mappings));
    let mut sources = b.source_configs(&b.asset_btc);
    sources.pop_back();
    sources.push_back(SourceConfig {
        oracle: source_id.clone(),
        asset: b.asset_btc.clone(),
    });
    b.submit_and_execute(
        &b.admin,
        GovernanceAction::SetProxy(
            b.asset_btc.clone(),
            ProxyConfig {
                sources,
                min_sources: 3,
                max_age_secs: Some(300),
                max_clock_drift_secs: Some(60),
            },
        ),
    );
    let source = PythLazerSourceClient::new(&b.env, &source_id);
    Wired { b, source }
}

#[test]
fn lazer_source_feeds_the_runtime_and_adapter() {
    let w = wired();
    let now = w.b.env.ledger().timestamp();
    w.b.push_upstream_price(&w.b.asset_btc, 5_000_000_000, now);

    assert!(matches!(
        w.b.refresh_one(&w.b.asset_btc),
        RefreshStatus::SourceUnavailable | RefreshStatus::ResolveFailed(_)
    ));

    assert_eq!(
        w.source
            .update_price_feeds(&btc_payload(&w.b.env, now, 5_100_000_000)),
        1
    );
    let RefreshStatus::Accepted(price) = w.b.refresh_one(&w.b.asset_btc) else {
        panic!("expected accepted refresh");
    };
    assert_eq!(price.mantissa, 5_000_000_000);
    assert_eq!(
        w.b.adapter.lastprice(&w.b.asset_btc).map(|p| p.price),
        Some(5_000_000_000)
    );
}

#[test]
fn stale_lazer_price_is_dropped_by_the_runtime_freshness_filter() {
    let w = wired();
    let now = w.b.env.ledger().timestamp();
    w.b.push_upstream_price(&w.b.asset_btc, 5_000_000_000, now);
    w.source
        .update_price_feeds(&btc_payload(&w.b.env, now - 100, 5_100_000_000));
    assert!(matches!(
        w.b.refresh_one(&w.b.asset_btc),
        RefreshStatus::Accepted(_)
    ));

    w.b.env.ledger().set_timestamp(now + 400);
    w.b.push_upstream_price(&w.b.asset_btc, 5_000_000_000, now + 400);
    assert!(matches!(
        w.b.refresh_one(&w.b.asset_btc),
        RefreshStatus::ResolveFailed(_)
    ));
}

#[test]
fn batcher_refreshes_and_extends_every_asset_in_one_call() {
    let w = wired();
    let now = w.b.env.ledger().timestamp();
    w.b.push_upstream_price(&w.b.asset_btc, 5_000_000_000, now);
    w.source
        .update_price_feeds(&btc_payload(&w.b.env, now, 5_100_000_000));

    let batcher_id = w.b.env.register(ProxyOracleBatcher, ());
    let batcher = ProxyOracleBatcherClient::new(&w.b.env, &batcher_id);
    let eth = Asset::Other(Symbol::new(&w.b.env, "ETH"));
    let assets = Vec::from_array(
        &w.b.env,
        [w.b.asset_btc.clone(), eth, w.b.asset_btc.clone()],
    );

    let statuses = batcher.refresh_many(&w.b.runtime_id, &assets);
    assert_eq!(statuses.len(), 3);
    assert!(matches!(statuses.get(0), Some(RefreshStatus::Accepted(_))));
    assert_eq!(statuses.get(1), Some(RefreshStatus::UnknownAsset));
    assert!(matches!(statuses.get(2), Some(RefreshStatus::Accepted(_))));

    assert_eq!(
        batcher.extend_ttl_many(&w.b.runtime_id, &assets),
        Vec::from_array(&w.b.env, [true, false, true])
    );
    let contracts: Vec<Address> = Vec::from_array(
        &w.b.env,
        [
            w.b.governance_id.clone(),
            w.b.adapter_id.clone(),
            w.source.address.clone(),
            w.b.runtime_id.clone(),
        ],
    );
    assert_eq!(
        batcher.extend_ttl_contracts(&contracts),
        Vec::from_array(&w.b.env, [true, true, true, false])
    );
}
