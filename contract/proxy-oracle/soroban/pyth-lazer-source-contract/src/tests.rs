#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::should_panic_without_expect)]

extern crate std;

use super::*;

use rstest::rstest;
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger},
    Address, Bytes, Env, Symbol,
};

use crate::testutils::{
    encode_payload, feed, payload_at, FeedSpec, MockVerifier, MockVerifierClient, CHANNEL_200MS,
    CHANNEL_REAL_TIME,
};

/// Pyth's shared Sui/Stellar test vector: BTC (1), ETH (2), SOL (112) on
/// `fixed_rate@200ms` at 1_771_252_161_800_000 µs.
const VECTOR_TIMESTAMP_US: u64 = 1_771_252_161_800_000;
const VECTOR_BTC_PRICE: i64 = 6_828_284_601_313;
const VECTOR_ETH_PRICE: i64 = 195_892_878_231;

fn vector_payload(env: &Env) -> Bytes {
    Bytes::from_slice(
        env,
        &hex_literal::hex!(
            "75d3c7934067e9c7f14a06000303010000000b00e1637ad5"
            "35060000015a2507d335060000027f8bfdf53506000004f8"
            "ff0600070008000900000a601299cd3e0600000bc07595c7"
            "3e0600000c014067e9c7f14a0600020000000b00971b209c"
            "2d0000000144056b9b2d0000000298fb6b9c2d00000004f8"
            "ff0600070008000900000a284444f92d0000000b480c07f9"
            "2d0000000c014067e9c7f14a0600700000000b0020d85dd2"
            "d78df30001000000000000000002000000000000000004f4"
            "ff060130f80bfeffffffff0701b8ab7057ec4a0600080100"
            "209db4060000000900000a00000000000000000b00000000"
            "000000000c014067e9c7f14a0600"
        ),
    )
}

const BTC_FEED: u32 = 1;
const ETH_FEED: u32 = 2;
const XLM_FEED: u32 = 23;

struct Harness {
    env: Env,
    owner: Address,
    verifier: MockVerifierClient<'static>,
    source: PythLazerSourceClient<'static>,
    base: Asset,
    btc: Asset,
    eth: Asset,
}

fn freshness() -> FreshnessConfig {
    FreshnessConfig {
        max_age_secs: 60,
        max_ahead_secs: 5,
    }
}

fn symbol_asset(env: &Env, symbol: &str) -> Asset {
    Asset::Other(Symbol::new(env, symbol))
}

fn mapping(env: &Env, feed_id: u32, symbol: &str) -> FeedMapping {
    FeedMapping {
        feed_id,
        asset: symbol_asset(env, symbol),
    }
}

fn harness_with(channel: LazerChannel, mappings: &[(u32, &str)]) -> Harness {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger()
        .set_timestamp(VECTOR_TIMESTAMP_US / MICROS_PER_SEC + 10);
    let owner = Address::generate(&env);
    let base = symbol_asset(&env, "USD");
    let verifier_id = env.register(MockVerifier, ());
    let mut feed_mappings = Vec::new(&env);
    for (feed_id, symbol) in mappings {
        feed_mappings.push_back(mapping(&env, *feed_id, symbol));
    }
    let config = Config {
        verifier: verifier_id.clone(),
        base: base.clone(),
        decimals: 8,
        channel,
        freshness: freshness(),
    };
    let source_id = env.register(PythLazerSource, (&owner, config, feed_mappings));
    Harness {
        verifier: MockVerifierClient::new(&env, &verifier_id),
        source: PythLazerSourceClient::new(&env, &source_id),
        btc: symbol_asset(&env, "BTC"),
        eth: symbol_asset(&env, "ETH"),
        env,
        owner,
        base,
    }
}

fn harness() -> Harness {
    harness_with(
        LazerChannel::FixedRate200ms,
        &[(BTC_FEED, "BTC"), (ETH_FEED, "ETH")],
    )
}

fn stored_btc(h: &Harness) -> StoredPrice {
    h.source.stored_price(&h.btc).expect("btc stored")
}

#[derive(Clone, Copy)]
enum MappingSet {
    One,
    DuplicateFeed,
    DuplicateAsset,
    TooMany,
}

fn mappings(env: &Env, set: MappingSet) -> Vec<FeedMapping> {
    match set {
        MappingSet::One => Vec::from_array(env, [mapping(env, BTC_FEED, "BTC")]),
        MappingSet::DuplicateFeed => Vec::from_array(
            env,
            [mapping(env, BTC_FEED, "BTC"), mapping(env, BTC_FEED, "ETH")],
        ),
        MappingSet::DuplicateAsset => Vec::from_array(
            env,
            [mapping(env, BTC_FEED, "BTC"), mapping(env, ETH_FEED, "BTC")],
        ),
        MappingSet::TooMany => {
            let mut all = Vec::new(env);
            for feed_id in 0..=MAX_FEED_MAPPINGS {
                all.push_back(mapping(env, feed_id, &std::format!("A{feed_id}")));
            }
            all
        }
    }
}

fn construct(decimals: u32, max_age_secs: u64, set: MappingSet) {
    let env = Env::default();
    let owner = Address::generate(&env);
    let config = Config {
        verifier: env.register(MockVerifier, ()),
        base: symbol_asset(&env, "USD"),
        decimals,
        channel: LazerChannel::FixedRate200ms,
        freshness: FreshnessConfig {
            max_age_secs,
            max_ahead_secs: 5,
        },
    };
    env.register(PythLazerSource, (&owner, config, mappings(&env, set)));
}

#[rstest]
#[should_panic]
#[case::decimals_above_max(19, 60, MappingSet::One)]
#[should_panic]
#[case::zero_max_age(8, 0, MappingSet::One)]
#[should_panic]
#[case::duplicate_feed(8, 60, MappingSet::DuplicateFeed)]
#[should_panic]
#[case::duplicate_asset(8, 60, MappingSet::DuplicateAsset)]
#[should_panic]
#[case::too_many_mappings(8, 60, MappingSet::TooMany)]
fn constructor_rejects_invalid_config(
    #[case] decimals: u32,
    #[case] max_age_secs: u64,
    #[case] set: MappingSet,
) {
    construct(decimals, max_age_secs, set);
}

#[test]
fn constructor_accepts_boundary_config() {
    construct(MAX_SEP40_DECIMALS, 1, MappingSet::One);
}

#[test]
fn exposes_sep40_metadata_and_config() {
    let h = harness();
    assert_eq!(h.source.base(), h.base);
    assert_eq!(h.source.decimals(), 8);
    assert_eq!(h.source.resolution(), 1);
    let assets = h.source.assets();
    assert_eq!(assets.len(), 2);
    assert!(assets.contains(&h.btc));
    assert!(assets.contains(&h.eth));
    let config = h.source.config().expect("config");
    assert_eq!(config.channel, LazerChannel::FixedRate200ms);
    assert_eq!(config.freshness, freshness());
    assert_eq!(h.source.feed_mappings().len(), 2);
    assert_eq!(h.source.get_owner(), Some(h.owner.clone()));
    assert_eq!(h.source.lastprice(&h.btc), None);
}

#[test]
fn stores_mapped_feeds_from_pyth_vector_and_ignores_unmapped() {
    let h = harness();
    assert_eq!(h.source.update_price_feeds(&vector_payload(&h.env)), 2);
    let updates = h.env.events().all().filter_by_contract(&h.source.address);
    assert_eq!(updates.events().len(), 2);

    assert_eq!(
        h.source.lastprice(&h.btc),
        Some(PriceData {
            price: i128::from(VECTOR_BTC_PRICE),
            timestamp: VECTOR_TIMESTAMP_US / MICROS_PER_SEC,
        })
    );
    assert_eq!(
        h.source.lastprice(&h.eth).map(|p| p.price),
        Some(i128::from(VECTOR_ETH_PRICE))
    );
    assert_eq!(
        stored_btc(&h),
        StoredPrice {
            mantissa: VECTOR_BTC_PRICE,
            expo: -8,
            publish_time_us: VECTOR_TIMESTAMP_US,
        }
    );
    assert_eq!(h.source.stored_price(&symbol_asset(&h.env, "SOL")), None);
}

#[test]
fn rescales_to_configured_decimals() {
    let h = harness();
    h.source.update_price_feeds(&vector_payload(&h.env));
    h.source.set_decimals(&6);
    assert_eq!(
        h.source.lastprice(&h.btc).expect("btc").price,
        i128::from(VECTOR_BTC_PRICE / 100)
    );
    h.source.set_decimals(&10);
    assert_eq!(
        h.source.lastprice(&h.btc).expect("btc").price,
        i128::from(VECTOR_BTC_PRICE) * 100
    );
    assert_eq!(
        h.source.try_set_decimals(&(MAX_SEP40_DECIMALS + 1)),
        Err(Ok(LazerSourceError::InvalidInput))
    );
}

#[test]
fn rejects_channel_mismatch() {
    let h = harness_with(LazerChannel::RealTime, &[(BTC_FEED, "BTC")]);
    assert_eq!(
        h.source.try_update_price_feeds(&vector_payload(&h.env)),
        Err(Ok(LazerSourceError::ChannelMismatch))
    );
    let real_time = encode_payload(
        &h.env,
        VECTOR_TIMESTAMP_US,
        CHANNEL_REAL_TIME,
        &[feed(BTC_FEED, 5, VECTOR_TIMESTAMP_US)],
    );
    assert_eq!(h.source.update_price_feeds(&real_time), 1);
}

#[test]
fn enforces_payload_freshness_window() {
    let h = harness();
    let now = h.env.ledger().timestamp();
    let at = |secs: u64| payload_at(&h.env, secs * MICROS_PER_SEC, &[(BTC_FEED, 5)]);
    assert_eq!(
        h.source.try_update_price_feeds(&at(now - 61)),
        Err(Ok(LazerSourceError::PayloadTooOld))
    );
    assert_eq!(
        h.source.try_update_price_feeds(&at(now + 6)),
        Err(Ok(LazerSourceError::PayloadTooFarAhead))
    );
    assert_eq!(h.source.update_price_feeds(&at(now - 60)), 1);
}

#[test]
fn publish_time_must_strictly_advance_per_asset() {
    let h = harness();
    let vector = vector_payload(&h.env);
    assert_eq!(h.source.update_price_feeds(&vector), 2);
    assert_eq!(h.source.update_price_feeds(&vector), 0);

    let older = payload_at(&h.env, VECTOR_TIMESTAMP_US - 1, &[(BTC_FEED, 7)]);
    assert_eq!(h.source.update_price_feeds(&older), 0);
    assert_eq!(stored_btc(&h).mantissa, VECTOR_BTC_PRICE);

    let newer = payload_at(&h.env, VECTOR_TIMESTAMP_US + 1, &[(BTC_FEED, 7)]);
    assert_eq!(h.source.update_price_feeds(&newer), 1);
    assert_eq!(stored_btc(&h).mantissa, 7);
    assert_eq!(stored_btc(&h).publish_time_us, VECTOR_TIMESTAMP_US + 1);
}

#[test]
fn feed_update_time_is_the_stored_clock_and_is_window_checked() {
    let h = harness();
    let now = h.env.ledger().timestamp();
    let payload_us = now * MICROS_PER_SEC;
    let at = |spec: FeedSpec| encode_payload(&h.env, payload_us, CHANNEL_200MS, &[spec]);

    let earlier = payload_us - 5_000_000;
    assert_eq!(
        h.source.update_price_feeds(&at(feed(BTC_FEED, 9, earlier))),
        1
    );
    assert_eq!(stored_btc(&h).publish_time_us, earlier);

    let ahead = feed(BTC_FEED, 11, (now + 6) * MICROS_PER_SEC);
    assert_eq!(h.source.update_price_feeds(&at(ahead)), 0);
    let stale = feed(ETH_FEED, 11, (now - 61) * MICROS_PER_SEC);
    assert_eq!(h.source.update_price_feeds(&at(stale)), 0);
    let no_feed_time = FeedSpec {
        feed_update_timestamp: None,
        ..feed(ETH_FEED, 11, payload_us)
    };
    assert_eq!(h.source.update_price_feeds(&at(no_feed_time)), 0);

    assert_eq!(h.source.lastprice(&h.eth), None);
    assert_eq!(stored_btc(&h).mantissa, 9);
}

#[test]
fn skips_feeds_without_a_positive_price_or_an_exponent() {
    let h = harness();
    let payload = encode_payload(
        &h.env,
        VECTOR_TIMESTAMP_US,
        CHANNEL_200MS,
        &[
            feed(BTC_FEED, 0, VECTOR_TIMESTAMP_US),
            feed(ETH_FEED, -1, VECTOR_TIMESTAMP_US),
            FeedSpec {
                exponent: None,
                ..feed(BTC_FEED, 5, VECTOR_TIMESTAMP_US)
            },
        ],
    );
    assert_eq!(h.source.update_price_feeds(&payload), 0);
    assert_eq!(h.source.lastprice(&h.btc), None);
    assert_eq!(h.source.lastprice(&h.eth), None);
}

#[test]
fn verifier_rejection_fails_the_update() {
    let h = harness();
    h.verifier.set_reject(&true);
    assert!(h
        .source
        .try_update_price_feeds(&vector_payload(&h.env))
        .is_err());
    assert_eq!(h.source.lastprice(&h.btc), None);
}

#[test]
fn malformed_verified_bytes_are_invalid_payload() {
    let h = harness();
    let garbage = Bytes::from_slice(&h.env, &[1, 2, 3]);
    assert_eq!(
        h.source.try_update_price_feeds(&garbage),
        Err(Ok(LazerSourceError::InvalidPayload))
    );
}

#[test]
fn feed_mapping_admin() {
    let h = harness();
    let xlm = symbol_asset(&h.env, "XLM");
    let xlm_payload = payload_at(&h.env, VECTOR_TIMESTAMP_US, &[(XLM_FEED, 17_000_000)]);
    assert_eq!(h.source.update_price_feeds(&xlm_payload), 0);

    h.source.add_feed(&mapping(&h.env, XLM_FEED, "XLM"));
    assert_eq!(h.source.update_price_feeds(&xlm_payload), 1);
    assert_eq!(h.source.lastprice(&xlm).expect("xlm").price, 17_000_000);
    assert_eq!(h.source.assets().len(), 3);

    assert_eq!(
        h.source.try_add_feed(&mapping(&h.env, 99, "XLM")),
        Err(Ok(LazerSourceError::DuplicateMapping))
    );

    h.source.remove_feed(&XLM_FEED);
    assert_eq!(h.source.lastprice(&xlm), None);
    assert_eq!(h.source.stored_price(&xlm), None);
    assert_eq!(h.source.assets().len(), 2);
    assert_eq!(
        h.source.try_remove_feed(&XLM_FEED),
        Err(Ok(LazerSourceError::UnknownFeed))
    );
}

#[test]
fn set_freshness_validates_and_applies() {
    let h = harness();
    assert_eq!(
        h.source.try_set_freshness(&FreshnessConfig {
            max_age_secs: 0,
            ..freshness()
        }),
        Err(Ok(LazerSourceError::InvalidInput))
    );
    h.source.set_freshness(&FreshnessConfig {
        max_age_secs: 5,
        max_ahead_secs: 0,
    });
    assert_eq!(
        h.source.try_update_price_feeds(&vector_payload(&h.env)),
        Err(Ok(LazerSourceError::PayloadTooOld))
    );
    assert_eq!(
        h.source.config().expect("config").freshness.max_ahead_secs,
        0
    );
}

#[test]
fn price_and_prices_serve_only_the_latest_record() {
    let h = harness_with(LazerChannel::FixedRate200ms, &[(BTC_FEED, "BTC")]);
    h.source
        .update_price_feeds(&payload_at(&h.env, VECTOR_TIMESTAMP_US, &[(BTC_FEED, 5)]));
    let last = h.source.lastprice(&h.btc).expect("btc");
    assert_eq!(h.source.price(&h.btc, &last.timestamp), Some(last.clone()));
    assert_eq!(h.source.price(&h.btc, &(last.timestamp - 1)), None);
    assert_eq!(h.source.prices(&h.btc, &0), None);
    assert_eq!(
        h.source.prices(&h.btc, &5),
        Some(Vec::from_array(&h.env, [last]))
    );
}

#[test]
fn upgrade_is_owner_gated_and_rejects_zero_hash() {
    let h = harness();
    let hash = BytesN::from_array(&h.env, &[7_u8; 32]);
    let stranger = Address::generate(&h.env);
    assert_eq!(
        h.source.try_upgrade(&hash, &stranger),
        Err(Ok(LazerSourceError::Unauthorized))
    );
    assert_eq!(
        h.source
            .try_upgrade(&BytesN::from_array(&h.env, &[0_u8; 32]), &h.owner),
        Err(Ok(LazerSourceError::InvalidInput))
    );
}

#[test]
fn extend_ttl_is_permissionless_and_covers_stored_prices() {
    let h = harness();
    h.source.update_price_feeds(&vector_payload(&h.env));
    h.source.extend_ttl();
    assert!(h.source.stored_price(&h.btc).is_some());
}
