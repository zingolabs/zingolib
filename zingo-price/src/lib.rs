#![warn(missing_docs)]

//! Crate for fetching historical and live ZEC prices

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ResponseData {
    #[serde(rename = "date")]
    _date: String,
    #[serde(rename = "priceUsd")]
    price_usd: String,
    time: u64,
}

/// Price of ZEC in USD at a given point in time.
#[derive(Debug)]
pub struct Price {
    time: u64,
    price_usd: f32,
}

/// Errors with price requests and parsing.
#[derive(Debug, thiserror::Error)]
pub enum PriceError {
    /// Request failed.
    #[error("request failed. {0}")]
    RequestFailed(#[from] reqwest::Error),
    /// Deserialization failed.
    #[error("deserialization failed. {0}")]
    DeserializationFailed(#[from] serde_json::Error),
    /// Response error. Commonly due to bad or missing CoinCap API keys.
    #[error("response error. {0}")]
    ResponseError(String),
    /// Parse error.
    #[error("parse error. {0}")]
    ParseError(#[from] std::num::ParseFloatError),
}

/// Get daily prices in USD from `start` to `end` time in milliseconds.
///
/// Prices taken at 00.00 UTC.
async fn get_daily_prices(start: u128, end: u128) -> Result<Vec<Price>, PriceError> {
    let url = format!(
        "https://rest.coincap.io/v3/assets/zcash/history?interval=d1&start={}&end={}",
        start, end
    );

    let client = reqwest::Client::new();
    let response: serde_json::Value = serde_json::from_str(
        client
            .get(url)
            // TODO: make secret or user specifies their own API keys
            .bearer_auth("4bce48cf8766d5c55ecdd83622cbba676fdc23745b7176916fd517b40e5ee6d0")
            .send()
            .await?
            .text()
            .await?
            .as_str(),
    )?;

    let response_data: Vec<ResponseData> = if let Some(data) = response.get("data") {
        serde_json::from_value(data.clone())?
    } else {
        return Err(PriceError::ResponseError(
            response
                .get("error")
                .expect("if no `data` is present must have `error` key")
                .to_string(),
        ));
    };

    Ok(response_data
        .into_iter()
        .map(|data| {
            Ok(Price {
                time: data.time / 1000,
                price_usd: data.price_usd.parse()?,
            })
        })
        .collect::<Result<Vec<Price>, std::num::ParseFloatError>>()?)
}

#[cfg(test)]
mod test {
    use crate::get_daily_prices;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn price_test() {
        let start: u128 = 1744870230000;

        let prices = get_daily_prices(
            start,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .await
        .unwrap();

        dbg!(prices);
    }
}
