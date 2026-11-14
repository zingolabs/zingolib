use std::{
    collections::HashSet,
    io::{Read, Write},
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use zcash_encoding::{Optional, Vector};

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
        let version = reader.read_u8()?;
        // a version above the one this build writes was written by a newer build in a layout this build cannot
        // read. it is refused so the newer layout is never read as an older one.
        if version > Self::serialized_version() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "PriceList serialized version {version} was written by a newer build. this build reads up to \
                     version {}.",
                    Self::serialized_version()
                ),
            ));
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The reader is given a serialized version above the one the type writes, followed by a price list the current
    /// layout reads.
    #[test]
    fn price_list_read_refuses_a_serialized_version_above_its_own() {
        let mut bytes = Vec::new();
        PriceList::new().write(&mut bytes).unwrap();
        PriceList::read(bytes.as_slice()).expect("the current version is read");

        bytes[0] = PriceList::serialized_version() + 1;

        let error = PriceList::read(bytes.as_slice())
            .map(drop)
            .expect_err("a version above the current version is refused");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
