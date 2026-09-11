//! LayerZero Scan read-only evidence adapter. Scan is corroborating evidence;
//! chain RPC remains authoritative for source and destination finality.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScanMessageV1 {
    pub guid: String,
    pub status: String,
    pub source_transaction: String,
    pub destination_transaction: Option<String>,
    pub raw: serde_json::Value,
}

#[async_trait::async_trait]
pub trait ScanClient: Send + Sync {
    async fn messages_by_transaction(&self, transaction_hash: &str) -> Result<Vec<ScanMessageV1>>;
}

pub struct HttpScanClient {
    base_url: reqwest::Url,
    client: reqwest::Client,
}

impl HttpScanClient {
    pub fn new(base_url: &str) -> Result<Self> {
        let mut base_url = reqwest::Url::parse(base_url)
            .map_err(|error| Error::InvalidInput(format!("invalid Scan URL: {error}")))?;
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        Ok(Self {
            base_url,
            client: reqwest::Client::new(),
        })
    }

    pub fn decode_response(value: serde_json::Value) -> Result<Vec<ScanMessageV1>> {
        let messages = value
            .as_array()
            .or_else(|| value.get("data").and_then(serde_json::Value::as_array))
            .or_else(|| value.get("messages").and_then(serde_json::Value::as_array))
            .ok_or_else(|| Error::Chain("Scan response omitted its messages array".into()))?;
        messages.iter().cloned().map(Self::decode_message).collect()
    }

    fn decode_message(raw: serde_json::Value) -> Result<ScanMessageV1> {
        fn string_at(value: &serde_json::Value, paths: &[&[&str]]) -> Option<String> {
            paths.iter().find_map(|path| {
                path.iter()
                    .try_fold(value, |current, key| current.get(*key))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
        }

        let guid = string_at(&raw, &[&["guid"], &["message", "guid"]])
            .ok_or_else(|| Error::Chain("Scan message omitted guid".into()))?;
        let status = string_at(&raw, &[&["status"], &["status", "name"]])
            .ok_or_else(|| Error::Chain("Scan message omitted status".into()))?;
        let source_transaction = string_at(
            &raw,
            &[
                &["source", "tx", "txHash"],
                &["source", "transaction", "txHash"],
                &["source", "txHash"],
            ],
        )
        .ok_or_else(|| Error::Chain("Scan message omitted source transaction".into()))?;
        let destination_transaction = string_at(
            &raw,
            &[
                &["destination", "tx", "txHash"],
                &["destination", "transaction", "txHash"],
                &["destination", "txHash"],
            ],
        );
        Ok(ScanMessageV1 {
            guid,
            status,
            source_transaction,
            destination_transaction,
            raw,
        })
    }
}

#[async_trait::async_trait]
impl ScanClient for HttpScanClient {
    async fn messages_by_transaction(&self, transaction_hash: &str) -> Result<Vec<ScanMessageV1>> {
        if transaction_hash.trim().is_empty() || transaction_hash.contains('/') {
            return Err(Error::InvalidInput(
                "Scan transaction hash must be a non-empty path segment".into(),
            ));
        }
        let url = self
            .base_url
            .join(&format!("messages/tx/{transaction_hash}"))
            .map_err(|error| Error::InvalidInput(format!("invalid Scan request URL: {error}")))?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| Error::Chain(format!("Scan request failed: {error}")))?;
        if !response.status().is_success() {
            return Err(Error::Chain(format!(
                "Scan request returned HTTP {}",
                response.status()
            )));
        }
        let value = response
            .json::<serde_json::Value>()
            .await
            .map_err(|error| Error::Chain(format!("Scan response JSON failed: {error}")))?;
        Self::decode_response(value)
    }
}
