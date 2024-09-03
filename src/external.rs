use near_sdk::{
    borsh::{self, BorshDeserialize, BorshSerialize},
    ext_contract,
    serde::{Deserialize, Serialize},
    Balance, Timestamp,
};

type AssetId = String;
pub type DurationSec = u32;

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct AssetPrice {
    pub asset_id: AssetId,
    pub price: Price,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Debug)]
#[serde(crate = "near_sdk::serde")]
pub struct AssetOptionalPrice {
    pub asset_id: AssetId,
    pub price: Option<Price>,
}

#[derive(BorshSerialize, BorshDeserialize, Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(crate = "near_sdk::serde")]
pub struct Price {
    #[serde(with = "u128_dec_format")]
    pub multiplier: Balance,
    pub decimals: u8,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Debug)]
#[serde(crate = "near_sdk::serde")]
pub struct PriceData {
    #[serde(with = "u64_dec_format")]
    pub timestamp: Timestamp,
    pub recency_duration_sec: DurationSec,

    pub prices: Vec<AssetOptionalPrice>,
}

impl Default for PriceData {
    fn default() -> Self {
        Self {
            timestamp: 1,            // default value for timestamp
            recency_duration_sec: 1, // default value for recency_duration_sec
            prices: vec![
                AssetOptionalPrice {
                    asset_id: "wrap.testnet".to_string(),
                    price: Some(Price {
                        multiplier: 15000, // default value for multiplier
                        decimals: 6,       // default value for decimals
                    }),
                },
                AssetOptionalPrice {
                    asset_id: "usdt.fakes.testnet".to_string(),
                    price: Some(Price {
                        multiplier: 10000,
                        decimals: 10,
                    }),
                },
            ], // default value for prices
        }
    }
}

// Validator interface, for cross-contract calls
#[ext_contract(ext_price_oracle)]
trait PriceOracle {
    fn get_price_data(&self, asset_ids: Option<Vec<AssetId>>) -> PriceData;
}

pub mod u128_dec_format {
    use near_sdk::serde::de;
    use near_sdk::serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(num: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&num.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

pub mod u64_dec_format {
    use near_sdk::serde::de;
    use near_sdk::serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(num: &u64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&num.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}
