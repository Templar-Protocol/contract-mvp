use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use templar_common::oracle::{pyth::PriceIdentifier, redstone};
use templar_gateway_macros::MethodSpec;

/// Submit a Pyth oracle update for one or more feeds.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "oracle.updatePyth")]
pub struct UpdatePyth {
    pub oracle_id: near_account_id::AccountId,
    /// Fetched from Hermes as one VAA and written in a single `update_price_feeds`
    /// call. Empty is a no-op.
    pub price_ids: Vec<PriceIdentifier>,
}

/// Submit a RedStone oracle update for one or more feeds.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "oracle.updateRedStone")]
pub struct UpdateRedStone {
    pub oracle_id: near_account_id::AccountId,
    /// Fetched as one payload and written in a single `redstone.writePrices` call,
    /// matching the adapter's own multi-feed method. Empty is a no-op.
    pub feed_ids: Vec<redstone::FeedId>,
}

/// Submit a Pyth Lazer oracle update for one or more feeds.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "oracle.updateLazer")]
pub struct UpdateLazer {
    pub oracle_id: near_account_id::AccountId,
    /// Fetched as one payload covering every feed and written in a single
    /// `update_price_feeds` call. Empty is a no-op.
    pub feed_ids: Vec<u32>,
}

/// Submit all updates needed for prices.
#[derive(MethodSpec, Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[method(write = "oracle.updatePrices")]
pub struct UpdatePrices {
    pub oracle_id: near_account_id::AccountId,
    pub price_ids: Vec<PriceIdentifier>,
}
