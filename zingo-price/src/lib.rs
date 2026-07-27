#![warn(missing_docs)]

//! Crate for ZEC price types, storage, and fetching.
//!
//! Currently only supports USD. The fetch defaults to clearnet; a caller that
//! wants to hide the client IP from the price source passes a local SOCKS5
//! proxy address (the Nym mixnet transport, ADR 0011) and the request is
//! routed through it instead.

use std::{
    collections::HashSet,
    io::{Read, Write},
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use serde::Deserialize;
use zcash_encoding::{Optional, Vector};

/// Errors with price requests and parsing.
// TODO: remove unused when historical data is implemented
#[derive(Debug, thiserror::Error)]
pub enum PriceError {
    /// Request failed.
    #[error("request failed. {0}")]
    RequestFailed(#[from] reqwest::Error),
    /// Deserialization failed.
    #[error("deserialization failed. {0}")]
    DeserializationFailed(#[from] serde_json::Error),
    /// Parse error.
    #[error("parse error. {0}")]
    ParseError(#[from] std::num::ParseFloatError),
    /// Price list start time not set. Call `PriceList::set_start_time`.
    #[error("price list start time has not been set.")]
    PriceListNotInitialized,
    /// Decimal conversion error.
    #[error("decimal conversion error. {0}")]
    DecimalError(#[from] rust_decimal::Error),
    /// Invalid price.
    #[error("invalid price.")]
    InvalidPrice,
}

#[derive(Debug, Deserialize)]
struct CurrentPriceResponse {
    price: String,
    timestamp: u32,
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
    // TODO: currently unused
    daily_prices: Vec<Price>,
    /// Time of last historical price update in seconds.
    // TODO: currently unused
    time_historical_prices_last_updated: Option<u32>,
}

impl Default for PriceList {
    fn default() -> Self {
        Self::new()
    }
}

impl PriceList {
    /// Constructs a new price list from the time of wallet creation.
    #[must_use]
    pub fn new() -> Self {
        PriceList {
            current_price: None,
            daily_prices: Vec::new(),
            time_historical_prices_last_updated: None,
        }
    }

    /// Returns current price.
    #[must_use]
    pub fn current_price(&self) -> Option<Price> {
        self.current_price
    }

    /// Returns historical price data by day.
    #[must_use]
    pub fn daily_prices(&self) -> &[Price] {
        &self.daily_prices
    }

    /// Returns time historical prices were last updated.
    #[must_use]
    pub fn time_historical_prices_last_updated(&self) -> Option<u32> {
        self.time_historical_prices_last_updated
    }

    /// Price list requires a start time before it can be updated.
    ///
    /// Recommended start time is the time the wallet's birthday block height was mined.
    pub fn set_start_time(&mut self, time_of_birthday: u32) {
        self.time_historical_prices_last_updated = Some(time_of_birthday);
    }

    /// Update and return current price of ZEC.
    ///
    /// Currently only USD is supported. When `socks5_proxy` is `Some`, the
    /// request is routed through that local SOCKS5 proxy (the Nym mixnet
    /// transport); `None` fetches over clearnet. This is a pure mechanism: the
    /// caller decides which route to use.
    pub async fn update_current_price(
        &mut self,
        socks5_proxy: Option<&str>,
    ) -> Result<Price, PriceError> {
        get_current_price(socks5_proxy, GEMINI_ZECUSD_URL).await
    }

    /// Prunes historical price list to only retain prices for the days containing `transaction_times`.
    ///
    /// Will not remove prices above or equal to the `prune_below` threshold.
    // TODO: under development
    pub fn prune(&mut self, transaction_times: Vec<u32>, prune_below: u32) {
        let mut relevant_days = HashSet::new();

        for transaction_time in transaction_times {
            for daily_price in self.daily_prices() {
                if daily_price.time > transaction_time {
                    assert!(daily_price.time - transaction_time < 60 * 60 * 24);
                    relevant_days.insert(daily_price.time);
                    break;
                }
            }
        }

        self.daily_prices
            .retain(|price| relevant_days.contains(&price.time) || price.time >= prune_below);
    }

    fn serialized_version() -> u8 {
        0
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        let _version = reader.read_u8()?;

        let time_last_updated = Optional::read(
            &mut reader,
            byteorder::ReadBytesExt::read_u32::<LittleEndian>,
        )?;
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
            time_historical_prices_last_updated: time_last_updated,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, mut writer: W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;

        Optional::write(
            &mut writer,
            self.time_historical_prices_last_updated(),
            byteorder::WriteBytesExt::write_u32::<LittleEndian>,
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

/// Installs rustls's `aws-lc-rs` provider as the process-level default if
/// none is installed yet (first-install-wins, so an embedder's choice is
/// kept). Required because reqwest is built with `rustls-tls-no-provider`.
/// Mirrors `zingo_netutils::ensure_default_crypto_provider`, which cannot be
/// used here: zingo-netutils sits above this crate in the dependency graph.
fn ensure_default_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}

/// The public price source: Gemini's recent-trades endpoint for the ZEC/USD
/// pair, eleven trades so the median (index 5 of the sorted list) is a robust
/// current price.
const GEMINI_ZECUSD_URL: &str = "https://api.gemini.com/v1/trades/zecusd?limit_trades=11";

/// Get current price of ZEC in USD from `url`, optionally through a local
/// SOCKS5 proxy.
///
/// When `socks5_proxy` is `Some(addr)`, the request is proxied through
/// `socks5h://addr`, so the destination hostname is resolved at the proxy and
/// never leaked to the local clearnet resolver (the Nym mixnet transport).
/// `None` fetches over clearnet. `url` is the trades endpoint; production
/// callers pass [`GEMINI_ZECUSD_URL`], tests point it at a local server.
async fn get_current_price(socks5_proxy: Option<&str>, url: &str) -> Result<Price, PriceError> {
    ensure_default_crypto_provider();
    let mut builder = reqwest::Client::builder();
    if let Some(addr) = socks5_proxy {
        builder = builder.proxy(reqwest::Proxy::all(format!("socks5h://{addr}"))?);
    }
    let httpget = builder.build()?.get(url).send().await?;
    let mut trades = httpget
        .json::<Vec<CurrentPriceResponse>>()
        .await?
        .iter()
        .map(|response| {
            let price_usd: f32 = response.price.parse()?;
            if !price_usd.is_finite() {
                return Err(PriceError::InvalidPrice);
            }

            Ok(Price {
                price_usd,
                time: response.timestamp,
            })
        })
        .collect::<Result<Vec<Price>, PriceError>>()?;

    trades.sort_by(|a, b| {
        a.price_usd
            .partial_cmp(&b.price_usd)
            .expect("trades are checked to be finite and comparable")
    });

    Ok(trades[5])
}

#[cfg(test)]
mod tests {
    use super::{GEMINI_ZECUSD_URL, get_current_price};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serves exactly one Gemini-shaped trades response over plain HTTP and
    /// returns the URL to hit it. One connection, then the socket closes.
    async fn spawn_trades_server(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // Drain the request headers; the content is irrelevant to the stub.
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(response.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });
        format!("http://{addr}/v1/trades/zecusd")
    }

    /// The clearnet fetch (`socks5_proxy = None`) performs the real HTTP round
    /// trip, deserializes the Gemini trades payload, and returns the median of
    /// the eleven trades (index 5 of the sorted list). Eleven deliberately
    /// out-of-order prices 100..=110 make the median 105 and prove the sort.
    #[tokio::test]
    async fn clearnet_fetch_returns_median_price() {
        let body = r#"[
            {"price":"110","timestamp":1},
            {"price":"100","timestamp":2},
            {"price":"105","timestamp":3},
            {"price":"101","timestamp":4},
            {"price":"109","timestamp":5},
            {"price":"102","timestamp":6},
            {"price":"108","timestamp":7},
            {"price":"103","timestamp":8},
            {"price":"107","timestamp":9},
            {"price":"104","timestamp":10},
            {"price":"106","timestamp":11}
        ]"#;
        let url = spawn_trades_server(body).await;

        let price = get_current_price(None, &url)
            .await
            .expect("the clearnet fetch parses a valid trades response");

        assert_eq!(
            price.price_usd, 105.0,
            "the returned price is the median (index 5) of the eleven sorted trades"
        );
    }

    /// Smoke test against the real Gemini endpoint over clearnet. Ignored by
    /// default (needs network and a live third party); run with
    /// `cargo test -p zingo-price -- --ignored` to confirm the live fetch.
    #[tokio::test]
    #[ignore = "hits the live Gemini API over clearnet"]
    async fn live_gemini_clearnet_fetch_smoke() {
        let price = get_current_price(None, GEMINI_ZECUSD_URL)
            .await
            .expect("the live Gemini fetch succeeds");
        assert!(
            price.price_usd > 0.0 && price.price_usd.is_finite(),
            "a live ZEC/USD price is positive and finite, got {}",
            price.price_usd
        );
    }
}
