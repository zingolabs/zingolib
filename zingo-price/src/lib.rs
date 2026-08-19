#![warn(missing_docs)]

//! Crate for ZEC price types, storage, and fetching.
//!
//! Currently only supports USD. The fetch defaults to clearnet; a caller that
//! wants to hide the client IP from the price source passes a local SOCKS5
//! proxy address (the Nym mixnet transport, ADR 0011) and the request is
//! routed through it instead.
//!
//! The whole fetch surface — and every dependency it needs — sits behind
//! the `socks5-fetch` feature (on by default for this crate alone).
//! Without it the crate is the storage-only data model: [`Price`] and
//! [`PriceList`] with their wallet-file serialization. This is the
//! dependency half of the mixnet-only price rule (ADR 0011, amendment
//! 2026-07-28): a wallet build without the mixnet compiles no fetch.

#[cfg(feature = "socks5-fetch")]
use std::time::Duration;
use std::{
    collections::HashSet,
    io::{Read, Write},
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

#[cfg(feature = "socks5-fetch")]
use serde::Deserialize;
use zcash_encoding::{Optional, Vector};
#[cfg(feature = "socks5-fetch")]
use zingo_net_diag::{NetOpFailure, NetOpStage, chain_texts};

/// Errors with price requests and parsing.
// TODO: remove unused when historical data is implemented
#[cfg(feature = "socks5-fetch")]
#[derive(Debug, thiserror::Error)]
pub enum PriceError {
    /// The HTTP request failed. The typed [`NetOpFailure`] names the stage
    /// and target; the original [`reqwest::Error`] is preserved whole as the
    /// source, so nothing is flattened away. This `Display` prints its own
    /// layer only — the cause chain belongs to `source()`.
    #[error("price request failed at {} to {}", failure.stage, failure.target)]
    RequestFailed {
        /// Which stage failed, against what target, with the cause chain
        /// captured layer by layer.
        failure: NetOpFailure,
        /// The underlying reqwest failure, untouched.
        #[source]
        source: reqwest::Error,
    },
    /// The price source answered with fewer trades than requested, so the
    /// median position does not exist. A typed [`NetOpStage::PayloadDecode`]
    /// condition: the payload decoded but was structurally short.
    #[error("the price source returned {received} trades where {TRADES_REQUESTED} were requested")]
    InsufficientTrades {
        /// How many trades the response carried.
        received: usize,
    },
    /// Deserialization failed.
    #[error("deserialization failed. {0}")]
    DeserializationFailed(#[from] serde_json::Error),
    /// Parse error.
    #[error("parse error. {0}")]
    ParseError(#[from] std::num::ParseFloatError),
    /// Price list start time not set. Call `PriceList::set_start_time`.
    #[error("price list start time has not been set.")]
    PriceListNotInitialized,
    /// Invalid price.
    #[error("invalid price.")]
    InvalidPrice,
    /// The source answered in-band with its own error report instead of a
    /// price (Kraken's `error` array). The report carries the source's own
    /// words.
    #[error("the price source reported: {0}")]
    SourceReportedError(String),
    /// The body decoded as JSON but its structure was not the source's
    /// documented shape. Names the missing piece.
    #[error("the price source answered with an unexpected shape: missing {0}")]
    UnexpectedShape(&'static str),
}

#[cfg(feature = "socks5-fetch")]
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

    /// Records `price` as the current price, so it survives serialization
    /// and [`Self::current_price`] reflects the latest fetch.
    ///
    /// This is the storage half of a price update. A caller that must not
    /// hold a lock across the network wait runs the wallet's speed-priority
    /// wave first, then records the result here under a briefly-held lock
    /// (the net-diag polling-blackout remedy).
    pub fn record_current_price(&mut self, price: Price) {
        self.current_price = Some(price);
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
#[cfg(feature = "socks5-fetch")]
fn ensure_default_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}

/// How many trades the fetch requests from the price source. Eleven, so the
/// median of the sorted list is a robust current price. [`GEMINI_ZECUSD_URL`]
/// embeds this value; a test pins the two together.
#[cfg(feature = "socks5-fetch")]
const TRADES_REQUESTED: usize = 11;

/// The median position in the sorted trades list.
#[cfg(feature = "socks5-fetch")]
const MEDIAN_INDEX: usize = TRADES_REQUESTED / 2;

/// The public price source: Gemini's recent-trades endpoint for the ZEC/USD
/// pair, requesting [`TRADES_REQUESTED`] trades.
#[cfg(feature = "socks5-fetch")]
const GEMINI_ZECUSD_URL: &str = "https://api.gemini.com/v1/trades/zecusd?limit_trades=11";

/// The client-side bound on the whole request. Twenty seconds keeps the
/// native bound under the mobile UI's 25-second watchdog, so the native call
/// ends (and releases whatever holds it) before or shortly after the UI
/// gives up. A hang through a half-dead tunnel becomes a typed
/// [`NetOpStage::TimedOut`] failure instead of an unbounded wait.
#[cfg(feature = "socks5-fetch")]
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// The client-side bound on establishing the connection alone.
#[cfg(feature = "socks5-fetch")]
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The winning answer of the three-source race: the price and the source
/// that answered first.
#[cfg(feature = "socks5-fetch")]
#[derive(Debug, Clone, Copy)]
pub struct RacedPrice {
    /// The first price to arrive.
    pub price: Price,
    /// The source that answered it.
    pub source: PriceSource,
}

/// Every source in the race failed. Each failure keeps its source's name
/// beside the typed error, and the rendered report carries every cause
/// chain, so a total outage is diagnosable per operator.
#[cfg(feature = "socks5-fetch")]
#[derive(Debug, thiserror::Error)]
#[error("every price source failed. {}", self.report())]
pub struct PriceRaceFailure {
    /// Each source's typed failure, in completion order.
    pub failures: Vec<(PriceSource, PriceError)>,
}

/// Separates a source's name from its rendered failure in a race report.
#[cfg(feature = "socks5-fetch")]
const RACE_LABEL_SEPARATOR: &str = ": ";

/// Separates one link of a rendered cause chain from the next in a race
/// report, which keeps every link on the one line.
#[cfg(feature = "socks5-fetch")]
const RACE_CHAIN_SEPARATOR: &str = ": ";

/// Separates one source's rendered failure from the next in a race report.
#[cfg(feature = "socks5-fetch")]
const RACE_SOURCE_SEPARATOR: &str = "; ";

#[cfg(feature = "socks5-fetch")]
impl PriceRaceFailure {
    /// One line per source: its name, its error, and the full cause chain.
    pub fn report(&self) -> String {
        self.failures
            .iter()
            .map(|(source, error)| {
                format!(
                    "{}{RACE_LABEL_SEPARATOR}{}",
                    source.name(),
                    chain_texts(error).join(RACE_CHAIN_SEPARATOR)
                )
            })
            .collect::<Vec<_>>()
            .join(RACE_SOURCE_SEPARATOR)
    }
}

/// The first quote among a driven race's outcomes, or the report naming
/// every source's typed failure when none answered. The wallet's one
/// speed-priority wave runs the race and turns its outcomes into the
/// result through this, so the racing lives in one place and this crate
/// says only what to fetch.
#[cfg(feature = "socks5-fetch")]
pub fn first_quote(
    outcomes: Vec<(PriceSource, Result<Price, PriceError>)>,
) -> Result<RacedPrice, PriceRaceFailure> {
    let mut failures = Vec::new();
    for (source, outcome) in outcomes {
        match outcome {
            Ok(price) => return Ok(RacedPrice { price, source }),
            Err(error) => failures.push((source, error)),
        }
    }
    Err(PriceRaceFailure { failures })
}

/// Every price source a run races, so a caller that drives the race asks
/// the same operators in the same order.
#[cfg(feature = "socks5-fetch")]
pub const RACED_SOURCES: [PriceSource; 9] = [
    PriceSource::Gemini,
    PriceSource::Kraken,
    PriceSource::CoinGecko,
    PriceSource::Coinbase,
    PriceSource::Bitfinex,
    PriceSource::Okx,
    PriceSource::Mexc,
    PriceSource::GateIo,
    PriceSource::Kucoin,
];

/// Kraken's public recent-trades endpoint for the ZEC/USD pair, requesting
/// [`TRADES_REQUESTED`] trades so the Gemini median contract transfers.
#[cfg(feature = "socks5-fetch")]
const KRAKEN_ZECUSD_URL: &str = "https://api.kraken.com/0/public/Trades?pair=ZECUSD&count=11";

/// CoinGecko's simple-price endpoint for ZEC in USD. An aggregator spot
/// value with its update time; there are no trades to take a median of.
#[cfg(feature = "socks5-fetch")]
const COINGECKO_ZECUSD_URL: &str = "https://api.coingecko.com/api/v3/simple/price?ids=zcash&vs_currencies=usd&include_last_updated_at=true";

/// Coinbase's spot-price endpoint for the ZEC/USD pair, a single quote
/// carrying no trades and no timestamp.
#[cfg(feature = "socks5-fetch")]
const COINBASE_ZECUSD_URL: &str = "https://api.coinbase.com/v2/prices/ZEC-USD/spot";

/// Bitfinex's public recent-trades endpoint for the ZEC/USD pair,
/// requesting [`TRADES_REQUESTED`] trades so the median contract transfers.
#[cfg(feature = "socks5-fetch")]
const BITFINEX_ZECUSD_URL: &str = "https://api-pub.bitfinex.com/v2/trades/tZECUSD/hist?limit=11";

/// OKX's public recent-trades endpoint for the ZEC/USDT pair — the
/// dollar-pegged stablecoin quote standing in for USD — requesting
/// [`TRADES_REQUESTED`] trades.
#[cfg(feature = "socks5-fetch")]
const OKX_ZECUSDT_URL: &str = "https://www.okx.com/api/v5/market/trades?instId=ZEC-USDT&limit=11";

/// MEXC's public recent-trades endpoint for the ZEC/USDT pair,
/// requesting [`TRADES_REQUESTED`] trades.
#[cfg(feature = "socks5-fetch")]
const MEXC_ZECUSDT_URL: &str = "https://api.mexc.com/api/v3/trades?symbol=ZECUSDT&limit=11";

/// Gate.io's public recent-trades endpoint for the ZEC/USDT pair,
/// requesting [`TRADES_REQUESTED`] trades.
#[cfg(feature = "socks5-fetch")]
const GATEIO_ZECUSDT_URL: &str =
    "https://api.gateio.ws/api/v4/spot/trades?currency_pair=ZEC_USDT&limit=11";

/// KuCoin's public trade-histories endpoint for the ZEC/USDT pair, which
/// takes no count parameter, so the parser keeps the newest
/// [`TRADES_REQUESTED`] rows.
#[cfg(feature = "socks5-fetch")]
const KUCOIN_ZECUSDT_URL: &str = "https://api.kucoin.com/api/v1/market/histories?symbol=ZEC-USDT";

/// The public price sources, each an independent operator and failure
/// domain.
#[cfg(feature = "socks5-fetch")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceSource {
    /// Gemini's recent-trades endpoint, median of eleven trades.
    Gemini,
    /// Kraken's recent-trades endpoint, median of eleven trades.
    Kraken,
    /// CoinGecko's simple-price endpoint, an aggregator's spot value.
    CoinGecko,
    /// Coinbase's spot-price endpoint, a single quote stamped at the fetch.
    Coinbase,
    /// Bitfinex's recent-trades endpoint, median of eleven trades.
    Bitfinex,
    /// OKX's recent-trades endpoint for the USDT pair, median of eleven
    /// trades.
    Okx,
    /// MEXC's recent-trades endpoint for the USDT pair, median of eleven
    /// trades.
    Mexc,
    /// Gate.io's recent-trades endpoint for the USDT pair, median of
    /// eleven trades.
    GateIo,
    /// KuCoin's trade-histories endpoint for the USDT pair, median of the
    /// newest eleven trades.
    Kucoin,
}

#[cfg(feature = "socks5-fetch")]
impl PriceSource {
    /// The stable lowercase name carried in payloads and reports.
    pub fn name(self) -> &'static str {
        match self {
            PriceSource::Gemini => "gemini",
            PriceSource::Kraken => "kraken",
            PriceSource::CoinGecko => "coingecko",
            PriceSource::Coinbase => "coinbase",
            PriceSource::Bitfinex => "bitfinex",
            PriceSource::Okx => "okx",
            PriceSource::Mexc => "mexc",
            PriceSource::GateIo => "gateio",
            PriceSource::Kucoin => "kucoin",
        }
    }

    /// The https endpoint this source answers at.
    pub fn url(self) -> &'static str {
        match self {
            PriceSource::Gemini => GEMINI_ZECUSD_URL,
            PriceSource::Kraken => KRAKEN_ZECUSD_URL,
            PriceSource::CoinGecko => COINGECKO_ZECUSD_URL,
            PriceSource::Coinbase => COINBASE_ZECUSD_URL,
            PriceSource::Bitfinex => BITFINEX_ZECUSD_URL,
            PriceSource::Okx => OKX_ZECUSDT_URL,
            PriceSource::Mexc => MEXC_ZECUSDT_URL,
            PriceSource::GateIo => GATEIO_ZECUSDT_URL,
            PriceSource::Kucoin => KUCOIN_ZECUSDT_URL,
        }
    }

    /// The source's parser, a pure function over the response text except
    /// Coinbase's, whose timeless spot quote is stamped at the fetch.
    fn parse(self, body: &str) -> Result<Price, PriceError> {
        match self {
            PriceSource::Gemini => parse_gemini_trades(body),
            PriceSource::Kraken => parse_kraken_trades(body),
            PriceSource::CoinGecko => parse_coingecko_simple(body),
            PriceSource::Coinbase => parse_coinbase_spot(body),
            PriceSource::Bitfinex => parse_bitfinex_trades(body),
            PriceSource::Okx => parse_okx_trades(body),
            PriceSource::Mexc => parse_mexc_trades(body),
            PriceSource::GateIo => parse_gateio_trades(body),
            PriceSource::Kucoin => parse_kucoin_histories(body),
        }
    }
}

/// The median of a sorted trades list, guarded so a structurally short
/// response is a typed refusal, never an index panic.
#[cfg(feature = "socks5-fetch")]
fn median_price(mut trades: Vec<Price>) -> Result<Price, PriceError> {
    trades.sort_by(|a, b| {
        a.price_usd
            .partial_cmp(&b.price_usd)
            .expect("trades are checked to be finite and comparable")
    });
    let received = trades.len();
    trades
        .get(MEDIAN_INDEX)
        .copied()
        .ok_or(PriceError::InsufficientTrades { received })
}

#[cfg(feature = "socks5-fetch")]
fn parse_gemini_trades(body: &str) -> Result<Price, PriceError> {
    let responses: Vec<CurrentPriceResponse> = serde_json::from_str(body)?;
    let trades = responses
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
    median_price(trades)
}

#[cfg(feature = "socks5-fetch")]
fn parse_kraken_trades(body: &str) -> Result<Price, PriceError> {
    let envelope: serde_json::Value = serde_json::from_str(body)?;
    if let Some(reported) = envelope["error"].as_array()
        && !reported.is_empty()
    {
        return Err(PriceError::SourceReportedError(
            reported
                .iter()
                .filter_map(|entry| entry.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    let pairs = envelope["result"]
        .as_object()
        .ok_or(PriceError::UnexpectedShape("the result object"))?;
    // The trades live under the pair's name (`XZECZUSD`); `last` is the
    // pagination cursor beside it.
    let (_pair, entries) = pairs
        .iter()
        .find(|(key, _)| *key != "last")
        .ok_or(PriceError::UnexpectedShape("the traded pair"))?;
    let trades = entries
        .as_array()
        .ok_or(PriceError::UnexpectedShape("the trades array"))?
        .iter()
        .map(|entry| {
            let price_usd: f32 = entry
                .get(0)
                .and_then(|price| price.as_str())
                .ok_or(PriceError::UnexpectedShape("a trade's price"))?
                .parse()?;
            if !price_usd.is_finite() {
                return Err(PriceError::InvalidPrice);
            }
            let time = entry
                .get(2)
                .and_then(|time| time.as_f64())
                .ok_or(PriceError::UnexpectedShape("a trade's time"))?
                as u32;
            Ok(Price { price_usd, time })
        })
        .collect::<Result<Vec<Price>, PriceError>>()?;
    median_price(trades)
}

#[cfg(feature = "socks5-fetch")]
#[derive(Debug, Deserialize)]
struct CoinGeckoZecQuote {
    usd: f32,
    last_updated_at: u32,
}

#[cfg(feature = "socks5-fetch")]
#[derive(Debug, Deserialize)]
struct CoinGeckoSimplePrice {
    zcash: CoinGeckoZecQuote,
}

#[cfg(feature = "socks5-fetch")]
fn parse_coingecko_simple(body: &str) -> Result<Price, PriceError> {
    let quote: CoinGeckoSimplePrice = serde_json::from_str(body)?;
    if !quote.zcash.usd.is_finite() {
        return Err(PriceError::InvalidPrice);
    }
    Ok(Price {
        price_usd: quote.zcash.usd,
        time: quote.zcash.last_updated_at,
    })
}

/// Coinbase's spot response: a single amount with no trades and no
/// timestamp, so the price is stamped at the fetch.
#[cfg(feature = "socks5-fetch")]
fn parse_coinbase_spot(body: &str) -> Result<Price, PriceError> {
    let envelope: serde_json::Value = serde_json::from_str(body)?;
    let price_usd: f32 = envelope["data"]["amount"]
        .as_str()
        .ok_or(PriceError::UnexpectedShape("the spot amount"))?
        .parse()?;
    if !price_usd.is_finite() {
        return Err(PriceError::InvalidPrice);
    }
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as u32)
        .unwrap_or_default();
    Ok(Price { price_usd, time })
}

/// Bitfinex's trades response: an array of `[id, millis, amount, price]`
/// rows, reduced to the median.
#[cfg(feature = "socks5-fetch")]
fn parse_bitfinex_trades(body: &str) -> Result<Price, PriceError> {
    let entries: Vec<serde_json::Value> = serde_json::from_str(body)?;
    let trades = entries
        .iter()
        .map(|entry| {
            let price_usd = entry
                .get(3)
                .and_then(serde_json::Value::as_f64)
                .ok_or(PriceError::UnexpectedShape("a trade's price"))?
                as f32;
            if !price_usd.is_finite() {
                return Err(PriceError::InvalidPrice);
            }
            let millis = entry
                .get(1)
                .and_then(serde_json::Value::as_u64)
                .ok_or(PriceError::UnexpectedShape("a trade's timestamp"))?;
            Ok(Price {
                price_usd,
                time: (millis / 1000) as u32,
            })
        })
        .collect::<Result<Vec<Price>, PriceError>>()?;
    median_price(trades)
}

/// OKX's trades response: a coded envelope whose `data` rows carry string
/// prices and millisecond timestamps, reduced to the median.
#[cfg(feature = "socks5-fetch")]
fn parse_okx_trades(body: &str) -> Result<Price, PriceError> {
    let envelope: serde_json::Value = serde_json::from_str(body)?;
    if envelope["code"].as_str() != Some("0") {
        return Err(PriceError::SourceReportedError(
            envelope["msg"]
                .as_str()
                .unwrap_or("unnamed OKX error")
                .to_string(),
        ));
    }
    let trades = envelope["data"]
        .as_array()
        .ok_or(PriceError::UnexpectedShape("the trades array"))?
        .iter()
        .map(|entry| {
            let price_usd: f32 = entry["px"]
                .as_str()
                .ok_or(PriceError::UnexpectedShape("a trade's price"))?
                .parse()?;
            if !price_usd.is_finite() {
                return Err(PriceError::InvalidPrice);
            }
            let millis: u64 = entry["ts"]
                .as_str()
                .ok_or(PriceError::UnexpectedShape("a trade's timestamp"))?
                .parse()
                .map_err(|_| PriceError::UnexpectedShape("a numeric timestamp"))?;
            Ok(Price {
                price_usd,
                time: (millis / 1000) as u32,
            })
        })
        .collect::<Result<Vec<Price>, PriceError>>()?;
    median_price(trades)
}

/// MEXC's trades response: an array of objects carrying string prices and
/// millisecond timestamps, reduced to the median.
#[cfg(feature = "socks5-fetch")]
fn parse_mexc_trades(body: &str) -> Result<Price, PriceError> {
    let entries: Vec<serde_json::Value> = serde_json::from_str(body)?;
    let trades = entries
        .iter()
        .map(|entry| {
            let price_usd: f32 = entry["price"]
                .as_str()
                .ok_or(PriceError::UnexpectedShape("a trade's price"))?
                .parse()?;
            if !price_usd.is_finite() {
                return Err(PriceError::InvalidPrice);
            }
            let millis = entry["time"]
                .as_u64()
                .ok_or(PriceError::UnexpectedShape("a trade's timestamp"))?;
            Ok(Price {
                price_usd,
                time: (millis / 1000) as u32,
            })
        })
        .collect::<Result<Vec<Price>, PriceError>>()?;
    median_price(trades)
}

/// Gate.io's trades response: an array of objects carrying string prices
/// and string second timestamps, reduced to the median.
#[cfg(feature = "socks5-fetch")]
fn parse_gateio_trades(body: &str) -> Result<Price, PriceError> {
    let entries: Vec<serde_json::Value> = serde_json::from_str(body)?;
    let trades = entries
        .iter()
        .map(|entry| {
            let price_usd: f32 = entry["price"]
                .as_str()
                .ok_or(PriceError::UnexpectedShape("a trade's price"))?
                .parse()?;
            if !price_usd.is_finite() {
                return Err(PriceError::InvalidPrice);
            }
            let time: u32 = entry["create_time"]
                .as_str()
                .ok_or(PriceError::UnexpectedShape("a trade's timestamp"))?
                .parse()
                .map_err(|_| PriceError::UnexpectedShape("a numeric timestamp"))?;
            Ok(Price { price_usd, time })
        })
        .collect::<Result<Vec<Price>, PriceError>>()?;
    median_price(trades)
}

/// KuCoin's histories response: a coded envelope whose rows carry string
/// prices and nanosecond timestamps, with the newest
/// [`TRADES_REQUESTED`] rows reduced to the median because the endpoint
/// takes no count parameter.
#[cfg(feature = "socks5-fetch")]
fn parse_kucoin_histories(body: &str) -> Result<Price, PriceError> {
    let envelope: serde_json::Value = serde_json::from_str(body)?;
    if envelope["code"].as_str() != Some("200000") {
        return Err(PriceError::SourceReportedError(
            envelope["msg"]
                .as_str()
                .unwrap_or("unnamed KuCoin error")
                .to_string(),
        ));
    }
    let rows = envelope["data"]
        .as_array()
        .ok_or(PriceError::UnexpectedShape("the trades array"))?;
    let newest = rows.len().saturating_sub(TRADES_REQUESTED);
    let trades = rows[newest..]
        .iter()
        .map(|entry| {
            let price_usd: f32 = entry["price"]
                .as_str()
                .ok_or(PriceError::UnexpectedShape("a trade's price"))?
                .parse()?;
            if !price_usd.is_finite() {
                return Err(PriceError::InvalidPrice);
            }
            let nanos = entry["time"]
                .as_u64()
                .ok_or(PriceError::UnexpectedShape("a trade's timestamp"))?;
            Ok(Price {
                price_usd,
                time: (nanos / 1_000_000_000) as u32,
            })
        })
        .collect::<Result<Vec<Price>, PriceError>>()?;
    median_price(trades)
}

/// The typed signals [`classify_stage`] reads from a [`reqwest::Error`],
/// extracted into a plain struct so the classification table is a pure
/// function testable with fabricated inputs (a `reqwest::Error` cannot be
/// constructed by hand).
#[cfg(feature = "socks5-fetch")]
#[derive(Clone, Copy, Debug, Default)]
struct RequestSignals {
    is_timeout: bool,
    is_connect: bool,
    is_status: bool,
    is_decode_or_body: bool,
}

#[cfg(feature = "socks5-fetch")]
impl RequestSignals {
    fn of(error: &reqwest::Error) -> Self {
        RequestSignals {
            is_timeout: error.is_timeout(),
            is_connect: error.is_connect(),
            is_status: error.is_status(),
            is_decode_or_body: error.is_decode() || error.is_body(),
        }
    }
}

/// The classification table (`docs/agents/net-diag-design.md`), checked in
/// order, pure over the extracted signals and the chain's layer texts.
///
/// The boundary between `RemoteTls` and `TunnelTransport` is genuinely fuzzy
/// from reqwest's chain alone: a handshake that reached the target and was
/// refused is `RemoteTls`, while a mid-handshake EOF (`tls handshake eof`,
/// the half-dead-tunnel signature) means the data path broke underneath the
/// handshake and is `TunnelTransport`. Chain-text inspection here is
/// classification input as the design's table specifies, not a decision on
/// the rendered stability contract.
#[cfg(feature = "socks5-fetch")]
fn classify_stage(
    signals: RequestSignals,
    chain: &[String],
    socks5_proxy: Option<&str>,
    bound: Duration,
) -> NetOpStage {
    let chain_mentions = |needle: &str| {
        chain
            .iter()
            .any(|layer| layer.to_ascii_lowercase().contains(needle))
    };
    if signals.is_timeout {
        return NetOpStage::TimedOut {
            after_ms: bound.as_millis().try_into().unwrap_or(u64::MAX),
        };
    }
    if signals.is_connect && socks5_proxy.is_some_and(&chain_mentions) {
        return NetOpStage::LocalProxyConnect;
    }
    if chain_mentions("socks") {
        return NetOpStage::SocksHandshake;
    }
    if chain_mentions("tls") || chain_mentions("certificate") || chain_mentions("handshake") {
        if chain_mentions("handshake eof") {
            return NetOpStage::TunnelTransport;
        }
        return NetOpStage::RemoteTls;
    }
    if signals.is_status {
        return NetOpStage::RemoteHttp;
    }
    if signals.is_decode_or_body {
        return NetOpStage::PayloadDecode;
    }
    if signals.is_connect {
        return NetOpStage::RemoteConnect;
    }
    NetOpStage::TunnelTransport
}

/// The [`NetOpFailure`] for one failed request: stage from the
/// classification table, target from the failing leg (the SOCKS endpoint for
/// local stages, the price URL otherwise), cause chain captured layer by
/// layer. The `reqwest::Error` itself travels beside it, untouched, in
/// [`PriceError::RequestFailed`].
#[cfg(feature = "socks5-fetch")]
fn classify_request(
    error: &reqwest::Error,
    socks5_proxy: Option<&str>,
    url: &str,
    bound: Duration,
) -> NetOpFailure {
    let chain = chain_texts(error);
    let stage = classify_stage(RequestSignals::of(error), &chain, socks5_proxy, bound);
    let target = match (&stage, socks5_proxy) {
        (NetOpStage::LocalProxyConnect | NetOpStage::SocksHandshake, Some(addr)) => {
            addr.to_string()
        }
        _ => url.to_string(),
    };
    NetOpFailure {
        stage,
        target,
        cause_chain: chain,
    }
}

/// Get current price of ZEC in USD from `url`, optionally through a local
/// SOCKS5 proxy, bounded by the given timeouts.
///
/// Production callers reach this through the wallet's speed-priority
/// wave; tests point `url`
/// at a local server and shrink the bounds.
#[cfg(all(test, feature = "socks5-fetch"))]
async fn get_current_price(
    socks5_proxy: Option<&str>,
    url: &str,
    request_timeout: Duration,
    connect_timeout: Duration,
) -> Result<Price, PriceError> {
    fetch_over(
        PriceSource::Gemini,
        socks5_proxy,
        url,
        request_timeout,
        connect_timeout,
    )
    .await
}

/// Fetch one source's answer over the shared HTTP leg and hand the body to
/// the source's parser. Transport failures classify through the net-diag
/// table; a body that arrives but does not parse is the parser's typed
/// refusal.
#[cfg(feature = "socks5-fetch")]
pub async fn get_source_price(
    source: PriceSource,
    dial: &zingo_netutils::conduit::ConduitDial,
    url: &str,
    request_timeout: Duration,
    connect_timeout: Duration,
) -> Result<Price, PriceError> {
    let proxy = dial.socks5().to_string();
    fetch_over(source, Some(&proxy), url, request_timeout, connect_timeout).await
}

/// The shared fetch, over `socks5_proxy` when one is given.
///
/// Private because a price fetch is mixnet-only (ADR 0011), and the public
/// entry takes a conduit guard to say so in the type. The crate's own tests
/// reach the clearnet leg through here, where no caller outside can.
#[cfg(feature = "socks5-fetch")]
async fn fetch_over(
    source: PriceSource,
    socks5_proxy: Option<&str>,
    url: &str,
    request_timeout: Duration,
    connect_timeout: Duration,
) -> Result<Price, PriceError> {
    ensure_default_crypto_provider();
    let typed = |error: reqwest::Error| PriceError::RequestFailed {
        failure: classify_request(&error, socks5_proxy, url, request_timeout),
        source: error,
    };
    let mut builder = reqwest::Client::builder()
        .timeout(request_timeout)
        .connect_timeout(connect_timeout);
    if let Some(addr) = socks5_proxy {
        builder = builder.proxy(reqwest::Proxy::all(format!("socks5h://{addr}")).map_err(typed)?);
    }
    let body = builder
        .build()
        .map_err(typed)?
        .get(url)
        .send()
        .await
        .map_err(typed)?
        .text()
        .await
        .map_err(typed)?;
    source.parse(&body)
}

#[cfg(all(test, feature = "socks5-fetch"))]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Generous bounds for tests whose subject is not the timeout.
    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// HYPOTHESIS: the race report renders a source's two-link cause chain
    /// exactly as the one sanctioned chain walk joined by this module's
    /// separator does, so the report keeps no private copy of the walk.
    /// Falsified if the two renderings differ by a single byte.
    #[test]
    fn the_race_report_matches_the_sanctioned_walk() {
        let parse_failure = "not a price"
            .parse::<f32>()
            .expect_err("the text is not a float");
        let error = PriceError::ParseError(parse_failure);
        let expected = format!(
            "{}: {}",
            PriceSource::Gemini.name(),
            chain_texts(&error).join(": ")
        );
        let failure = PriceRaceFailure {
            failures: vec![(PriceSource::Gemini, error)],
        };

        assert_eq!(failure.report(), expected);
    }

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

        let price = get_current_price(None, &url, TEST_TIMEOUT, TEST_TIMEOUT)
            .await
            .expect("the clearnet fetch parses a valid trades response");

        assert_eq!(
            price.price_usd, 105.0,
            "the returned price is the median (index 5) of the eleven sorted trades"
        );
    }

    /// Smoke test against a real price source over clearnet. Ignored by
    /// default (needs network and a live third party); run with
    /// `cargo test -p zingo-price -- --ignored`. The race that once lived
    /// here is the wallet's speed-priority wave, so this proves the fetch
    /// this crate still owns.
    #[tokio::test]
    #[ignore = "hits a live price-source API over clearnet"]
    async fn live_clearnet_price_fetch_smoke() {
        let source = PriceSource::Kraken;
        let price = fetch_over(source, None, source.url(), REQUEST_TIMEOUT, CONNECT_TIMEOUT)
            .await
            .expect("the live price fetch succeeds");
        assert!(
            price.price_usd > 0.0 && price.price_usd.is_finite(),
            "a live ZEC/USD price is positive and finite, got {}",
            price.price_usd
        );
    }

    #[tokio::test]
    async fn a_black_holed_server_times_out_typed_within_the_bound() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/trades/zecusd", listener.local_addr().unwrap());
        // Keep the listener alive but never accept-and-answer.
        let hold = tokio::spawn(async move {
            let _sock = listener.accept().await;
            std::future::pending::<()>().await;
        });

        let short = Duration::from_millis(300);
        let error = get_current_price(None, &url, short, short)
            .await
            .expect_err("a silent server cannot serve a price");
        match &error {
            PriceError::RequestFailed { failure, source } => {
                assert_eq!(failure.stage, NetOpStage::TimedOut { after_ms: 300 });
                assert!(source.is_timeout(), "the reqwest source keeps its kind");
                assert!(
                    !failure.cause_chain.is_empty(),
                    "the cause chain is captured layer by layer"
                );
            }
            other => panic!("expected a typed RequestFailed, got {other}"),
        }
        hold.abort();
    }

    /// A dead SOCKS5 proxy (the port is closed when the fetch dials it)
    /// surfaces as a typed connect-phase failure with the reqwest source
    /// and the cause chain preserved whole, so a consumer distinguishes a
    /// connect failure from a TLS or HTTP failure by fields instead of
    /// parsing prose.
    #[tokio::test]
    async fn a_dead_proxy_failure_is_typed_with_its_source_intact() {
        // Bind then drop, so the port is closed when the fetch dials it.
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };

        let short = Duration::from_millis(300);
        let error = get_current_price(
            Some(&closed.to_string()),
            "http://127.0.0.1:9/v1/trades/zecusd",
            short,
            short,
        )
        .await
        .expect_err("a closed port cannot serve a price");
        match &error {
            PriceError::RequestFailed { failure, source } => {
                assert!(
                    source.is_connect(),
                    "the reqwest failure must keep its kind: {source}"
                );
                assert!(
                    matches!(
                        failure.stage,
                        NetOpStage::LocalProxyConnect
                            | NetOpStage::SocksHandshake
                            | NetOpStage::RemoteConnect
                    ),
                    "a dead local proxy must classify as a connect-phase stage: {failure}"
                );
                assert!(
                    !failure.cause_chain.is_empty(),
                    "the cause chain is captured layer by layer"
                );
            }
            other => panic!("expected a typed RequestFailed, got {other}"),
        }
    }

    /// A structurally short trades response is a typed refusal, never an
    /// index panic (the design's median-guard requirement).
    #[tokio::test]
    async fn fewer_trades_than_requested_is_a_typed_refusal() {
        let body = r#"[
            {"price":"110","timestamp":1},
            {"price":"100","timestamp":2}
        ]"#;
        let url = spawn_trades_server(body).await;

        let error = get_current_price(None, &url, TEST_TIMEOUT, TEST_TIMEOUT)
            .await
            .expect_err("two trades cannot yield the median of eleven");
        assert!(
            matches!(error, PriceError::InsufficientTrades { received: 2 }),
            "the refusal must be typed with the received count: {error}"
        );
    }

    /// Eleven Kraken-shaped trades around a 43.00 median. Trade entries are
    /// [price, volume, time, buy/sell, market/limit, misc, id].
    const KRAKEN_ELEVEN_TRADES: &str = r#"{
        "error": [],
        "result": {
            "XZECZUSD": [
                ["42.80","1.0",1700000001.1,"b","m","",1],
                ["43.20","0.5",1700000002.2,"s","l","",2],
                ["42.90","2.0",1700000003.3,"b","m","",3],
                ["43.10","1.1",1700000004.4,"s","m","",4],
                ["43.00","0.7",1700000005.5,"b","l","",5],
                ["42.70","0.2",1700000006.6,"s","m","",6],
                ["43.30","1.4",1700000007.7,"b","m","",7],
                ["42.60","0.9",1700000008.8,"s","l","",8],
                ["43.40","0.3",1700000009.9,"b","m","",9],
                ["42.95","1.8",1700000010.1,"s","m","",10],
                ["43.05","0.6",1700000011.2,"b","l","",11]
            ],
            "last": "1700000011200000000"
        }
    }"#;

    #[test]
    fn kraken_trades_parse_to_the_median_price() {
        let median = parse_kraken_trades(KRAKEN_ELEVEN_TRADES).expect("eleven finite trades parse");
        assert_eq!(median.price_usd, 43.00);
        assert_eq!(median.time, 1700000005);
    }

    #[test]
    fn a_kraken_in_band_error_is_a_typed_source_report() {
        let body = r#"{"error":["EQuery:Unknown asset pair"],"result":{}}"#;
        let error = parse_kraken_trades(body).expect_err("a reported error is never a price");
        assert!(
            matches!(&error, PriceError::SourceReportedError(report)
                if report.contains("EQuery:Unknown asset pair")),
            "the report must carry the source's own words: {error}"
        );
    }

    #[test]
    fn a_kraken_response_without_a_pair_is_a_typed_shape_refusal() {
        let body = r#"{"error":[],"result":{"last":"1700000011200000000"}}"#;
        let error = parse_kraken_trades(body).expect_err("no pair key, no trades");
        assert!(
            matches!(error, PriceError::UnexpectedShape(_)),
            "the refusal must name the missing shape: {error}"
        );
    }

    #[test]
    fn coingecko_simple_price_parses_with_its_update_time() {
        let body = r#"{"zcash":{"usd":41.37,"last_updated_at":1700000123}}"#;
        let spot = parse_coingecko_simple(body).expect("a spot quote parses");
        assert_eq!(spot.price_usd, 41.37);
        assert_eq!(spot.time, 1700000123);
    }

    #[test]
    fn a_non_finite_coingecko_price_is_a_typed_refusal() {
        let body = r#"{"zcash":{"usd":1e39,"last_updated_at":1700000123}}"#;
        let error = parse_coingecko_simple(body).expect_err("an overflowed float is no price");
        assert!(
            matches!(error, PriceError::InvalidPrice),
            "the refusal must be the typed invalid-price arm: {error}"
        );
    }

    /// A server that answers every connection with this body, until dropped.
    async fn spawn_answering_server(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(response.as_bytes()).await;
            }
        });
        url
    }

    /// A server that accepts and never answers, until dropped.
    async fn spawn_silent_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _sock = listener.accept().await;
            std::future::pending::<()>().await;
        });
        url
    }

    /// HYPOTHESIS: the first source that answered supplies the quote, and
    /// the sources that failed before it are discarded rather than
    /// reported. Falsified if a later answer wins or a failure survives a
    /// success.
    #[tokio::test]
    async fn the_first_answer_supplies_the_quote() {
        let garbage = spawn_answering_server("not json at all").await;
        let kraken = spawn_answering_server(KRAKEN_ELEVEN_TRADES).await;
        let short = Duration::from_millis(500);

        let refused = fetch_over(PriceSource::Gemini, None, &garbage, short, short).await;
        let answered = fetch_over(PriceSource::Kraken, None, &kraken, short, short).await;
        let won = first_quote(vec![
            (PriceSource::Gemini, refused),
            (PriceSource::Kraken, answered),
        ])
        .expect("one healthy source supplies the quote");
        assert_eq!(won.source, PriceSource::Kraken);
        assert_eq!(won.price.price_usd, 43.00);
    }

    /// HYPOTHESIS: when no source answered, the report carries every
    /// source's typed failure and names each operator, so a total outage is
    /// diagnosable per operator rather than as one opaque refusal.
    /// Falsified if a failure or a name leaves the report.
    #[tokio::test]
    async fn a_run_where_every_source_fails_reports_them_all() {
        let garbage_one = spawn_answering_server("not json at all").await;
        let garbage_two = spawn_answering_server(r#"{"unexpected":true}"#).await;
        let silent = spawn_silent_server().await;
        let short = Duration::from_millis(300);

        let mut outcomes = Vec::new();
        for (source, url) in [
            (PriceSource::Gemini, garbage_one),
            (PriceSource::Kraken, garbage_two),
            (PriceSource::CoinGecko, silent),
        ] {
            outcomes.push((source, fetch_over(source, None, &url, short, short).await));
        }
        let failure = first_quote(outcomes).expect_err("no source answered with a price");
        let named: Vec<&str> = failure
            .failures
            .iter()
            .map(|(source, _)| source.name())
            .collect();
        assert_eq!(failure.failures.len(), 3);
        for name in ["gemini", "kraken", "coingecko"] {
            assert!(named.contains(&name), "missing {name} in {named:?}");
            assert!(
                failure.to_string().contains(name),
                "the rendered report must name {name}: {failure}"
            );
        }
    }
}
