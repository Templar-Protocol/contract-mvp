//! Test fixtures shared with the integration suite: a Lazer payload encoder
//! and a stand-in for Pyth's verifier.

extern crate alloc;

use alloc::vec::Vec;

use soroban_sdk::{contract, contracterror, contractimpl, symbol_short, Bytes, Env, Symbol};

pub use crate::MICROS_PER_SEC;

pub const PAYLOAD_MAGIC: u32 = 0x93C7_D375;
pub const CHANNEL_REAL_TIME: u8 = 1;
pub const CHANNEL_200MS: u8 = 3;
const PROP_PRICE: u8 = 0;
const PROP_EXPONENT: u8 = 4;
const PROP_FEED_UPDATE_TIMESTAMP: u8 = 12;

pub struct FeedSpec {
    pub feed_id: u32,
    pub price: Option<i64>,
    pub exponent: Option<i16>,
    pub feed_update_timestamp: Option<u64>,
}

/// A feed carrying price, exponent -8, and its own update time.
#[must_use]
pub fn feed(feed_id: u32, price: i64, feed_update_timestamp: u64) -> FeedSpec {
    FeedSpec {
        feed_id,
        price: Some(price),
        exponent: Some(-8),
        feed_update_timestamp: Some(feed_update_timestamp),
    }
}

/// Encode a Lazer payload (the bytes Pyth's verifier returns) from feed specs.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn encode_payload(env: &Env, timestamp_us: u64, channel: u8, feeds: &[FeedSpec]) -> Bytes {
    let mut out = Vec::new();
    out.extend_from_slice(&PAYLOAD_MAGIC.to_le_bytes());
    out.extend_from_slice(&timestamp_us.to_le_bytes());
    out.push(channel);
    out.push(feeds.len() as u8);
    for spec in feeds {
        out.extend_from_slice(&spec.feed_id.to_le_bytes());
        let count = u8::from(spec.price.is_some())
            + u8::from(spec.exponent.is_some())
            + u8::from(spec.feed_update_timestamp.is_some());
        out.push(count);
        if let Some(price) = spec.price {
            out.push(PROP_PRICE);
            out.extend_from_slice(&(price as u64).to_le_bytes());
        }
        if let Some(exponent) = spec.exponent {
            out.push(PROP_EXPONENT);
            out.extend_from_slice(&(exponent as u16).to_le_bytes());
        }
        if let Some(timestamp) = spec.feed_update_timestamp {
            out.push(PROP_FEED_UPDATE_TIMESTAMP);
            out.push(1);
            out.extend_from_slice(&timestamp.to_le_bytes());
        }
    }
    Bytes::from_slice(env, &out)
}

/// Payload on `fixed_rate@200ms` whose feeds all carry the payload's own timestamp.
#[must_use]
pub fn payload_at(env: &Env, timestamp_us: u64, feeds: &[(u32, i64)]) -> Bytes {
    let specs: Vec<FeedSpec> = feeds
        .iter()
        .map(|(id, price)| feed(*id, *price, timestamp_us))
        .collect();
    encode_payload(env, timestamp_us, CHANNEL_200MS, &specs)
}

const REJECT: Symbol = symbol_short!("REJECT");

#[contracterror]
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MockVerifierError {
    SignerNotTrusted = 6,
}

/// Identity verifier with Pyth's `verify_update` ABI: the submitted bytes are
/// returned as the verified payload unless rejection is switched on.
#[contract]
pub struct MockVerifier;

#[contractimpl]
impl MockVerifier {
    pub fn set_reject(env: Env, reject: bool) {
        env.storage().instance().set(&REJECT, &reject);
    }

    pub fn verify_update(env: Env, data: Bytes) -> Result<Bytes, MockVerifierError> {
        if env
            .storage()
            .instance()
            .get::<_, bool>(&REJECT)
            .unwrap_or(false)
        {
            return Err(MockVerifierError::SignerNotTrusted);
        }
        Ok(data)
    }
}
