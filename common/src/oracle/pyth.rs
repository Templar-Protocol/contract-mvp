//! Derived from <https://github.com/pyth-network/pyth-crosschain/blob/main/target_chains/near>.
//! Modified for use with the Templar Protocol contracts.
//!
//! The original code was released under the following license:
//!
//! Copyright 2025 Pyth Data Association.
//!
//! Licensed under the Apache License, Version 2.0 (the "License");
//! you may not use this file except in compliance with the License.
//! You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//!
//! Unless required by applicable law or agreed to in writing, software
//! distributed under the License is distributed on an "AS IS" BASIS,
//! WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//! See the License for the specific language governing permissions and
//! limitations under the License.
use std::{collections::HashMap, fmt::Display};

use near_sdk::{
    ext_contract,
    json_types::{I64, U64},
    near,
};

pub type OracleResponse = HashMap<PriceIdentifier, Option<Price>>;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [borsh, json])]
pub struct PriceIdentifier(
    #[cfg_attr(
        not(target_arch = "wasm32"),
        schemars(schema_with = "price_identifier_json_schema")
    )]
    #[serde(
        serialize_with = "hex::serde::serialize",
        deserialize_with = "hex::serde::deserialize"
    )]
    pub [u8; 32],
);

#[cfg(not(target_arch = "wasm32"))]
fn price_identifier_json_schema(
    gen: &mut schemars::gen::SchemaGenerator,
) -> schemars::schema::Schema {
    let mut schema = gen.subschema_for::<String>().into_object();
    let validation = schema.string();
    validation.min_length = Some(64);
    validation.max_length = Some(64);
    validation.pattern = Some("^[0-9A-Fa-f]{64}$".to_string());
    schema.into()
}

impl std::fmt::Debug for PriceIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl Display for PriceIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// A price with a degree of uncertainty, represented as a price +- a confidence interval.
///
/// The confidence interval roughly corresponds to the standard error of a normal distribution.
/// Both the price and confidence are stored in a fixed-point numeric representation,
/// `x * (10^expo)`, where `expo` is the exponent.
//
/// Please refer to the documentation at
/// <https://docs.pyth.network/documentation/pythnet-price-feeds/best-practices>
/// for how to use this price safely.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
pub struct Price {
    pub price: I64,
    /// Confidence interval around the price
    pub conf: U64,
    /// The exponent
    pub expo: i32,
    /// Unix timestamp of when this price was computed
    pub publish_time: PythTimestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json, borsh])]
#[serde(transparent)]
pub struct PythTimestamp(i64);

impl PythTimestamp {
    /// Creates a `PythTimestamp` from a value in seconds.
    pub fn from_secs(secs: i64) -> Self {
        Self(secs)
    }

    /// Converts milliseconds to a [`PythTimestamp`], stored in whole seconds,
    /// truncating any fractional seconds.
    pub fn from_ms(ms: i64) -> Self {
        Self(ms / 1000)
    }

    /// Returns the timestamp value in seconds.
    pub fn as_secs(&self) -> i64 {
        self.0
    }

    /// Converts a [`PythTimestamp`] (stored in whole seconds) to milliseconds
    /// by performing a checked multiplication by 1000.
    pub fn as_ms(&self) -> Option<i64> {
        self.0.checked_mul(1000)
    }

    pub fn try_into_time(self) -> Option<templar_primitives::Nanoseconds> {
        let ms = self.as_ms()?;
        Some(templar_primitives::Nanoseconds::from_ms(
            u64::try_from(ms).ok()?,
        ))
    }

    pub fn try_from_time(value: templar_primitives::Nanoseconds) -> Option<Self> {
        let ms = value.as_ms();
        Some(PythTimestamp::from_ms(i64::try_from(ms).ok()?))
    }
}

#[ext_contract(ext_pyth)]
pub trait Pyth {
    // See implementations for details, PriceIdentifier can be passed either as a 64 character
    // hex price ID which can be found on the Pyth homepage.
    fn price_feed_exists(&self, price_identifier: PriceIdentifier) -> bool;
    // fn get_price(&self, price_identifier: PriceIdentifier) -> Option<Price>;
    // fn get_price_unsafe(&self, price_identifier: PriceIdentifier) -> Option<Price>;
    // fn get_price_no_older_than(&self, price_id: PriceIdentifier, age: u64) -> Option<Price>;
    // fn get_ema_price(&self, price_id: PriceIdentifier) -> Option<Price>;
    // fn get_ema_price_unsafe(&self, price_id: PriceIdentifier) -> Option<Price>;
    // fn get_ema_price_no_older_than(&self, price_id: PriceIdentifier, age: u64) -> Option<Price>;
    // fn list_prices(
    //     &self,
    //     price_ids: Vec<PriceIdentifier>,
    // ) -> HashMap<PriceIdentifier, Option<Price>>;
    // fn list_prices_unsafe(
    //     &self,
    //     price_ids: Vec<PriceIdentifier>,
    // ) -> HashMap<PriceIdentifier, Option<Price>>;
    // fn list_prices_no_older_than(
    //     &self,
    //     price_ids: Vec<PriceIdentifier>,
    // ) -> HashMap<PriceIdentifier, Option<Price>>;
    // fn list_ema_prices(
    //     &self,
    //     price_ids: Vec<PriceIdentifier>,
    // ) -> HashMap<PriceIdentifier, Option<Price>>;
    fn list_ema_prices_unsafe(
        &self,
        price_ids: Vec<PriceIdentifier>,
    ) -> HashMap<PriceIdentifier, Option<Price>>;
    fn list_ema_prices_no_older_than(
        &self,
        price_ids: Vec<PriceIdentifier>,
        age: u64,
    ) -> HashMap<PriceIdentifier, Option<Price>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use templar_primitives::Nanoseconds;

    #[test]
    fn price_identifier_schema_matches_serde_wire_format() {
        let schema = near_sdk::serde_json::to_value(schemars::schema_for!(PriceIdentifier))
            .expect("price identifier schema serializes");
        let validator =
            jsonschema::draft7::new(&schema).expect("price identifier schema is Draft 7");
        let identifier = PriceIdentifier([0xaa; 32]);
        let serialized =
            near_sdk::serde_json::to_value(identifier).expect("price identifier serializes");

        assert_eq!(
            serialized,
            near_sdk::serde_json::json!(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            )
        );
        validator
            .validate(&serialized)
            .expect("serialized price identifier is valid under its schema");
        assert_eq!(
            near_sdk::serde_json::from_value::<PriceIdentifier>(serialized)
                .expect("price identifier deserializes"),
            identifier
        );

        let uppercase = near_sdk::serde_json::json!(
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );
        validator
            .validate(&uppercase)
            .expect("uppercase hexadecimal is schema-valid");
        assert_eq!(
            near_sdk::serde_json::from_value::<PriceIdentifier>(uppercase)
                .expect("uppercase hexadecimal deserializes"),
            identifier
        );

        for malformed in [
            near_sdk::serde_json::json!(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            near_sdk::serde_json::json!(
                "gaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            near_sdk::serde_json::json!([0, 1]),
        ] {
            validator
                .validate(&malformed)
                .expect_err("malformed price identifier is not schema-valid");
            near_sdk::serde_json::from_value::<PriceIdentifier>(malformed)
                .expect_err("malformed price identifier does not deserialize");
        }
    }

    #[test]
    fn can_parse_real_price() {
        let real_price = r#"{ "conf": "2696300000", "expo": -8, "price": "7154901300000", "publish_time": 1773381271 }"#;

        let parsed = near_sdk::serde_json::from_str::<Price>(real_price).unwrap();
        assert_eq!(parsed.price.0, 7_154_901_300_000);
        assert_eq!(parsed.conf.0, 2_696_300_000);
        assert_eq!(parsed.expo, -8);
        assert_eq!(parsed.publish_time.as_secs(), 1_773_381_271);
    }

    #[test]
    fn try_into_time_handles_negative_millisecond_inputs_per_current_truncation() {
        // `from_ms` stores whole seconds, so negative sub-second values truncate toward zero.
        let truncated_to_zero = PythTimestamp::from_ms(-1);
        assert_eq!(truncated_to_zero.try_into_time(), Some(Nanoseconds::zero()));

        // Negative whole-second values remain negative and cannot convert to unsigned time.
        let negative_second = PythTimestamp::from_ms(-1_000);
        assert_eq!(negative_second.try_into_time(), None);
    }

    #[test]
    fn try_from_time_accepts_max_representable_nanoseconds_range() {
        let value = Nanoseconds::from_ns(u64::MAX);
        let expected = PythTimestamp::from_ms(i64::try_from(value.as_ms()).unwrap());

        assert_eq!(PythTimestamp::try_from_time(value), Some(expected));
    }

    #[test]
    fn try_from_time_and_try_into_time_round_trip_truncates_to_whole_seconds() {
        let value = Nanoseconds::from_ns(1_234_567_890);

        let round_tripped = PythTimestamp::try_from_time(value)
            .and_then(PythTimestamp::try_into_time)
            .unwrap();

        assert_eq!(round_tripped, Nanoseconds::from_secs(1));
    }
}
