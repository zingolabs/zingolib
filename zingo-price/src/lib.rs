#![warn(missing_docs)]

//! Crate for fetching historical and live ZEC prices

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct HistoricalPriceData {
    /// Date.
    #[serde(rename = "date")]
    _date: String,
    /// ZEC price in USD.
    #[serde(rename = "priceUsd")]
    price_usd: String,
    /// Time in milliseconds.
    time: u128,
}

#[derive(Debug, Deserialize)]
struct CurrentPriceData {
    /// ZEC price in USD.
    #[serde(rename = "priceUsd")]
    price_usd: String,
}

/// Price of ZEC in USD at a given point in time.
#[derive(Debug, Clone, Copy)]
pub struct Price {
    /// Time in seconds.
    time: u32,
    /// ZEC price in USD.
    price_usd: f32,
}

/// Price list for wallets to maintain an updated list of daily ZEC prices.
#[derive(Debug)]
pub struct PriceList {
    /// Time of last price update in seconds.
    time_last_updated: Option<u32>,
    /// Current price.
    current_price: Option<Price>,
    /// Historical price data by day.
    daily_prices: Vec<Price>,
}

impl PriceList {
    /// Constructs a new price list from the time of wallet creation.
    pub fn new() -> Self {
        PriceList {
            time_last_updated: None,
            current_price: None,
            daily_prices: Vec::new(),
        }
    }

    /// Returns time price list was last updated.
    pub fn time_last_updated(&self) -> Option<u32> {
        self.time_last_updated
    }

    /// Returns current price.
    pub fn current_price(&self) -> Option<Price> {
        self.current_price
    }

    /// Returns historical price data by day.
    pub fn daily_prices(&self) -> &[Price] {
        &self.daily_prices
    }

    /// Price list requires a start time before it can be updated.
    ///
    /// Recommended start time is the time the wallet's birthday block height was mined.
    pub fn set_start_time(&mut self, time_of_birthday: u32) {
        self.time_last_updated = Some(time_of_birthday);
    }

    /// Updates price list.
    pub async fn update(&mut self) -> Result<(), PriceError> {
        // FIXME: move to secret or user specified
        let api_key = "4bce48cf8766d5c55ecdd83622cbba676fdc23745b7176916fd517b40e5ee6d0";

        self.current_price = Some(get_current_price(&api_key).await?);
        let current_time = self
            .current_price
            .clone()
            .expect("should be non-empty")
            .time;

        // TODO: test no overlap / duplicates
        if let Some(time_last_updated) = self.time_last_updated {
            self.daily_prices.append(
                &mut get_daily_prices(
                    time_last_updated as u128 * 1000,
                    current_time as u128 * 1000,
                    api_key,
                )
                .await?,
            );
        } else {
            return Err(PriceError::PriceListNotInitialized);
        }

        self.time_last_updated = Some(
            self.current_price
                .clone()
                .expect("should be non-empty")
                .time,
        );

        Ok(())
    }
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
    /// Price list start time not set. Call `PriceList::set_start_time`.
    #[error("price list start time not set. call `PriceList::set_start_time`.")]
    PriceListNotInitialized,
}

/// Get current price of ZEC in USD.
async fn get_current_price(api_key: &str) -> Result<Price, PriceError> {
    let url = "https://rest.coincap.io/v3/assets/zcash".to_string();

    let client = reqwest::Client::new();
    let response: serde_json::Value = serde_json::from_str(
        client
            .get(url)
            .bearer_auth(api_key)
            .send()
            .await?
            .text()
            .await?
            .as_str(),
    )?;

    let current_price = if let Some(data) = response.get("data") {
        let response_data: CurrentPriceData = serde_json::from_value(data.clone())?;
        let response_time = response
            .get("timestamp")
            .expect("data should include timestamp")
            .as_u64()
            .expect("should be positive integer");

        Price {
            time: (response_time / 1000) as u32,
            price_usd: response_data.price_usd.parse()?,
        }
    } else {
        return Err(PriceError::ResponseError(
            response
                .get("error")
                .expect("if no `data` is present must have `error` key")
                .to_string(),
        ));
    };

    Ok(current_price)
}

/// Get daily prices in USD from `start` to `end` time in milliseconds.
///
/// Prices taken at 00.00 UTC.
async fn get_daily_prices(start: u128, end: u128, api_key: &str) -> Result<Vec<Price>, PriceError> {
    let url = format!(
        "https://rest.coincap.io/v3/assets/zcash/history?interval=d1&start={}&end={}",
        start, end
    );

    let client = reqwest::Client::new();
    let response: serde_json::Value = serde_json::from_str(
        client
            .get(url)
            .bearer_auth(api_key)
            .send()
            .await?
            .text()
            .await?
            .as_str(),
    )?;

    let response_data: Vec<HistoricalPriceData> = if let Some(data) = response.get("data") {
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
                time: (data.time / 1000) as u32,
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
        let api_key = "4bce48cf8766d5c55ecdd83622cbba676fdc23745b7176916fd517b40e5ee6d0";

        let prices = get_daily_prices(
            start,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
            api_key,
        )
        .await
        .unwrap();

        dbg!(prices);
    }
}
