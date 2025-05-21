#![warn(missing_docs)]

//! Crate for fetching historical and live ZEC prices.
//!
//! Currently only supports USD.
//! Prices are fetched using the CoinCap API. A CoinCap API key is needed via registration.

use std::{
    collections::HashSet,
    io::{Read, Write},
    time::SystemTime,
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use serde::Deserialize;

use zcash_client_backend::tor::{self, http::cryptex::Exchanges};
use zcash_encoding::{Optional, Vector};

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

/// Price of ZEC in USD at a given point in time.
#[derive(Debug, Clone, Copy)]
pub struct Price {
    /// Time in seconds.
    pub time: u32,
    /// ZEC price in USD.
    pub price_usd: f32,
}

/// Price list for wallets to maintain an updated list of daily ZEC prices.
#[derive(Debug)]
pub struct PriceList {
    /// Current price.
    current_price: Option<Price>,
    /// Historical price data by day.
    daily_prices: Vec<Price>,
    /// CoinCap API key.
    api_key: Option<String>,
    /// Time of last historical price update in seconds.
    time_historical_prices_last_updated: Option<u32>,
}

impl Default for PriceList {
    fn default() -> Self {
        Self::new()
    }
}

impl PriceList {
    /// Constructs a new price list from the time of wallet creation.
    pub fn new() -> Self {
        PriceList {
            current_price: None,
            daily_prices: Vec::new(),
            api_key: None,
            time_historical_prices_last_updated: None,
        }
    }

    /// Returns current price.
    pub fn current_price(&self) -> Option<Price> {
        self.current_price
    }

    /// Returns historical price data by day.
    pub fn daily_prices(&self) -> &[Price] {
        &self.daily_prices
    }

    /// Returns the CoinCap api key for fetching prices.
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Returns time historical prices were last updated.
    pub fn time_historical_prices_last_updated(&self) -> Option<u32> {
        self.time_historical_prices_last_updated
    }

    /// Sets CoinCap API key.
    pub fn set_api_key(&mut self, api_key: String) {
        self.api_key = Some(api_key);
    }

    /// Price list requires a start time before it can be updated.
    ///
    /// Recommended start time is the time the wallet's birthday block height was mined.
    pub fn set_start_time(&mut self, time_of_birthday: u32) {
        self.time_historical_prices_last_updated = Some(time_of_birthday);
    }

    /// Update and return current price of ZEC over tor.
    ///
    /// Currently only USD is supported.
    pub async fn update_current_price(
        &mut self,
        tor_client: &tor::Client,
    ) -> Result<Price, PriceError> {
        let current_price = get_current_price(tor_client).await?;
        self.current_price = Some(current_price);

        Ok(current_price)
    }

    /// Updates historical daily price list.
    ///
    /// Warning: historical price fetch not currently over tor but does not leak any wallet data.
    /// Requires registration of a CoinCap API key.
    /// Currently only USD is supported.
    pub async fn update_historical_price_list(&mut self) -> Result<(), PriceError> {
        let current_time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("should never fail when comparing with an instant so far in the past")
            .as_secs() as u32;

        if let Some(api_key) = self.api_key() {
            if let Some(time_last_updated) = self.time_historical_prices_last_updated {
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
        } else {
            return Err(PriceError::NoApiKey);
        }

        self.time_historical_prices_last_updated = Some(current_time);

        Ok(())
    }

    /// Prunes historical price list to only retain prices for the days containing `transaction_times`.
    ///
    /// Will not remove prices above or equal to the `prune_below` threshold.
    pub fn prune(&mut self, transaction_times: Vec<u32>, prune_below: u32) {
        let mut relevant_days = HashSet::new();

        for transaction_time in transaction_times.into_iter() {
            for daily_price in self.daily_prices() {
                if daily_price.time > transaction_time {
                    assert!(daily_price.time - transaction_time < 86_400);
                    relevant_days.insert(daily_price.time);
                    break;
                }
            }
        }

        self.daily_prices
            .retain(|price| relevant_days.contains(&price.time) || price.time >= prune_below)
    }

    fn serialized_version() -> u8 {
        0
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        let _version = reader.read_u8()?;

        let api_key = Optional::read(&mut reader, read_string)?;
        let time_last_updated = Optional::read(&mut reader, |r| r.read_u32::<LittleEndian>())?;
        let current_price = Optional::read(&mut reader, |r| {
            Ok(Price {
                time: r.read_u32::<LittleEndian>()?,
                price_usd: r.read_f32::<LittleEndian>()?,
            })
        })?;
        let daily_prices = Vector::read(&mut reader, |r| {
            Ok(Price {
                time: r.read_u32::<LittleEndian>()?,
                price_usd: r.read_f32::<LittleEndian>()?,
            })
        })?;

        Ok(Self {
            current_price,
            daily_prices,
            api_key,
            time_historical_prices_last_updated: time_last_updated,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, mut writer: W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;

        Optional::write(&mut writer, self.api_key(), |w, api_key| {
            write_string(w, api_key)
        })?;
        Optional::write(
            &mut writer,
            self.time_historical_prices_last_updated(),
            |w, time| w.write_u32::<LittleEndian>(time),
        )?;
        Optional::write(&mut writer, self.current_price(), |w, price| {
            w.write_u32::<LittleEndian>(price.time)?;
            w.write_f32::<LittleEndian>(price.price_usd)
        })?;
        Vector::write(&mut writer, self.daily_prices(), |w, price| {
            w.write_u32::<LittleEndian>(price.time)?;
            w.write_f32::<LittleEndian>(price.price_usd)
        })
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
    #[error("price list start time has not been set.")]
    PriceListNotInitialized,
    /// API key has not been set. Call `PriceList::set_api_key`.
    #[error("API key has not been set.")]
    NoApiKey,
    /// Tor price fetch error.
    #[error("Tor price fetch error. {0}")]
    TorError(#[from] tor::Error),
    /// Decimal conversion error.
    #[error("Decimal conversion error. {0}")]
    DecimalError(#[from] rust_decimal::Error),
}

/// Get current price of ZEC in USD over tor.
async fn get_current_price(tor_client: &tor::Client) -> Result<Price, PriceError> {
    let exchanges = Exchanges::unauthenticated_known_with_gemini_trusted();
    let current_price = tor_client.get_latest_zec_to_usd_rate(&exchanges).await?;
    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("should never fail when comparing with an instant so far in the past")
        .as_secs() as u32;

    Ok(Price {
        time: current_time,
        price_usd: current_price.try_into()?,
    })
}

/// Get daily prices in USD from `start` to `end` time in milliseconds.
async fn get_daily_prices(start: u128, end: u128, api_key: &str) -> Result<Vec<Price>, PriceError> {
    let url = format!(
        "https://rest.coincap.io/v3/assets/zcash/history?interval=d1&start={}&end={}",
        start, end
    );

    // TODO: implement over tor.
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

fn read_string<R: Read>(mut reader: R) -> std::io::Result<String> {
    let str_len = reader.read_u64::<LittleEndian>()?;
    let mut str_bytes = vec![0; str_len as usize];
    reader.read_exact(&mut str_bytes)?;

    String::from_utf8(str_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

fn write_string<W: Write>(mut writer: W, str: &str) -> std::io::Result<()> {
    writer.write_u64::<LittleEndian>(str.len() as u64)?;
    writer.write_all(str.as_bytes())
}
