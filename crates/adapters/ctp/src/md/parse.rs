// -------------------------------------------------------------------------------------------------
//  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved.
//  https://nautechsystems.io
//
//  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
//  You may not use this file except in compliance with the License.
//  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
// -------------------------------------------------------------------------------------------------

//! Converts CTP depth snapshots into quote and trade ticks.
//!
//! CTP market data carries no trade messages. Trades are inferred from increases in the
//! cumulative session volume between consecutive snapshots, priced at the last traded price.
//! Aggressor side is not reported by the feed and is set to `NoAggressor`.

use anyhow::Context;
use jiff::{civil::DateTime, tz::TimeZone};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{QuoteTick, TradeTick},
    enums::AggressorSide,
    identifiers::{InstrumentId, TradeId},
    types::{Price, Quantity},
};

/// The SDK reports unset prices as `DBL_MAX`. Any value at or above this bound is treated as unset.
const UNSET_PRICE_THRESHOLD: f64 = 1.0e300;

/// Exchange-local clock for CTP timestamps (China Standard Time, UTC+8, no daylight saving).
const EXCHANGE_UTC_OFFSET_HOURS: i8 = 8;

/// Per-instrument state carried between snapshots.
#[derive(Debug, Clone)]
pub struct InstrumentState {
    pub instrument_id: InstrumentId,
    pub price_precision: u8,
    pub size_precision: u8,
    /// Cumulative session volume from the previous snapshot, or `None` before the first one.
    pub last_volume: Option<i32>,
}

/// Quote and trade ticks produced from one snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedDepth {
    pub quote: Option<QuoteTick>,
    pub trade: Option<TradeTick>,
}

/// Converts a snapshot into ticks and advances the instrument's volume baseline.
///
/// The baseline is advanced even when no tick is produced, so the next snapshot is measured
/// from the latest observed volume.
///
/// # Errors
///
/// Returns an error if the exchange timestamp is malformed.
pub fn parse_depth(
    depth: &super::DepthMarketData,
    state: &mut InstrumentState,
    ts_init: UnixNanos,
) -> anyhow::Result<ParsedDepth> {
    let ts_event =
        exchange_timestamp_ns(&depth.action_day, &depth.update_time, depth.update_millisec)?;

    let quote = parse_quote(depth, state, ts_event, ts_init);
    let trade = parse_trade(depth, state, ts_event, ts_init);

    state.last_volume = Some(depth.volume);

    Ok(ParsedDepth { quote, trade })
}

fn parse_quote(
    depth: &super::DepthMarketData,
    state: &InstrumentState,
    ts_event: UnixNanos,
    ts_init: UnixNanos,
) -> Option<QuoteTick> {
    let bid_price = ctp_price(depth.bid_price1, state.price_precision)?;
    let ask_price = ctp_price(depth.ask_price1, state.price_precision)?;
    let bid_size = ctp_size(depth.bid_volume1, state.size_precision)?;
    let ask_size = ctp_size(depth.ask_volume1, state.size_precision)?;

    QuoteTick::new_checked(
        state.instrument_id,
        bid_price,
        ask_price,
        bid_size,
        ask_size,
        ts_event,
        ts_init,
    )
    .ok()
}

fn parse_trade(
    depth: &super::DepthMarketData,
    state: &InstrumentState,
    ts_event: UnixNanos,
    ts_init: UnixNanos,
) -> Option<TradeTick> {
    let previous = state.last_volume?;

    // A decrease means the cumulative volume restarted (new trading day or feed reset). It is
    // not a trade, and the new value becomes the baseline.
    if depth.volume <= previous {
        return None;
    }

    let size = ctp_size(depth.volume - previous, state.size_precision)?;
    let price = ctp_price(depth.last_price, state.price_precision)?;

    TradeTick::new_checked(
        state.instrument_id,
        price,
        size,
        AggressorSide::NoAggressor,
        trade_id(&depth.action_day, depth.volume)?,
        ts_event,
        ts_init,
    )
    .ok()
}

/// Returns a price for a finite, positive SDK value. Unset and invalid values yield `None`.
pub(crate) fn ctp_price(value: f64, precision: u8) -> Option<Price> {
    if !value.is_finite() || value <= 0.0 || value >= UNSET_PRICE_THRESHOLD {
        return None;
    }
    Price::new_checked(value, precision).ok()
}

/// Returns a size for a non-negative SDK volume. Negative values yield `None`.
pub(crate) fn ctp_size(volume: i32, precision: u8) -> Option<Quantity> {
    if volume < 0 {
        return None;
    }
    Quantity::new_checked(f64::from(volume), precision).ok()
}

/// Derives a trade ID from the trading date and the cumulative volume at the trade.
///
/// Cumulative volume is unique within a session, so the pair is unique per instrument.
fn trade_id(action_day: &str, volume: i32) -> Option<TradeId> {
    TradeId::new_checked(format!("{action_day}-{volume}")).ok()
}

/// Converts an exchange `YYYYMMDD` date and `HH:MM:SS` time plus milliseconds to Unix nanoseconds.
///
/// # Errors
///
/// Returns an error if either string is malformed or the date and time are out of range.
pub fn exchange_timestamp_ns(
    action_day: &str,
    update_time: &str,
    millisec: i32,
) -> anyhow::Result<UnixNanos> {
    anyhow::ensure!(action_day.len() == 8, "invalid action day `{action_day}`");
    anyhow::ensure!(
        update_time.len() == 8,
        "invalid update time `{update_time}`"
    );
    anyhow::ensure!(
        (0..1000).contains(&millisec),
        "invalid millisecond `{millisec}`"
    );

    let year: i16 = action_day[0..4]
        .parse()
        .context("invalid action day year")?;
    let month: i8 = action_day[4..6]
        .parse()
        .context("invalid action day month")?;
    let day: i8 = action_day[6..8].parse().context("invalid action day day")?;
    let hour: i8 = update_time[0..2]
        .parse()
        .context("invalid update time hour")?;
    let minute: i8 = update_time[3..5]
        .parse()
        .context("invalid update time minute")?;
    let second: i8 = update_time[6..8]
        .parse()
        .context("invalid update time second")?;
    anyhow::ensure!(
        update_time.as_bytes()[2] == b':' && update_time.as_bytes()[5] == b':',
        "invalid update time `{update_time}`"
    );

    let local = DateTime::new(year, month, day, hour, minute, second, millisec * 1_000_000)
        .with_context(|| {
            format!("invalid exchange time {action_day} {update_time}.{millisec:03}")
        })?;

    let offset = jiff::tz::Offset::from_hours(EXCHANGE_UTC_OFFSET_HOURS)?;
    let zoned = local.to_zoned(TimeZone::fixed(offset))?;
    let nanos = zoned.timestamp().as_nanosecond();

    let nanos = u64::try_from(nanos).context("exchange timestamp precedes the Unix epoch")?;
    Ok(UnixNanos::from(nanos))
}

#[cfg(test)]
mod tests {
    use nautilus_model::identifiers::InstrumentId;
    use rstest::rstest;

    use super::*;
    use crate::md::DepthMarketData;

    fn state(price_precision: u8, size_precision: u8) -> InstrumentState {
        InstrumentState {
            instrument_id: InstrumentId::from("rb2510.SHFE"),
            price_precision,
            size_precision,
            last_volume: None,
        }
    }

    fn depth(volume: i32, last_price: f64) -> DepthMarketData {
        DepthMarketData {
            instrument_id: "rb2510".to_string(),
            exchange_id: "SHFE".to_string(),
            action_day: "20251122".to_string(),
            update_time: "09:30:01".to_string(),
            update_millisec: 500,
            last_price,
            bid_price1: 3100.0,
            bid_volume1: 12,
            ask_price1: 3100.2,
            ask_volume1: 7,
            volume,
            turnover: 0.0,
        }
    }

    #[rstest]
    fn test_exchange_timestamp_is_utc_plus_eight() {
        // 2025-11-22 09:30:01.500 in UTC+8 is 01:30:01.500 UTC.
        let nanos = exchange_timestamp_ns("20251122", "09:30:01", 500).unwrap();
        let expected = jiff::civil::date(2025, 11, 22)
            .at(1, 30, 1, 500_000_000)
            .to_zoned(TimeZone::UTC)
            .unwrap()
            .timestamp()
            .as_nanosecond();
        assert_eq!(i128::from(nanos.as_u64()), expected);
    }

    #[rstest]
    #[case("2025112", "09:30:01", 0)]
    #[case("20251122", "0930:01", 0)]
    #[case("20251122", "09-30-01", 0)]
    #[case("20251322", "09:30:01", 0)]
    #[case("20251122", "09:30:01", 1000)]
    fn test_exchange_timestamp_rejects_malformed_input(
        #[case] day: &str,
        #[case] time: &str,
        #[case] millisec: i32,
    ) {
        assert!(exchange_timestamp_ns(day, time, millisec).is_err());
    }

    #[rstest]
    fn test_unset_prices_are_absent() {
        assert!(ctp_price(f64::MAX, 1).is_none());
        assert!(ctp_price(0.0, 1).is_none());
        assert!(ctp_price(-1.0, 1).is_none());
        assert!(ctp_price(f64::NAN, 1).is_none());
        assert_eq!(ctp_price(3100.2, 1).unwrap().to_string(), "3100.2");
    }

    #[rstest]
    fn test_quote_built_from_top_of_book() {
        let mut st = state(1, 0);
        let parsed = parse_depth(&depth(100, 3100.0), &mut st, 1.into()).unwrap();
        let quote = parsed.quote.unwrap();
        assert_eq!(quote.bid_price.to_string(), "3100.0");
        assert_eq!(quote.ask_price.to_string(), "3100.2");
        assert_eq!(quote.bid_size.to_string(), "12");
        assert_eq!(quote.ask_size.to_string(), "7");
    }

    #[rstest]
    fn test_quote_absent_when_a_side_is_unset() {
        let mut st = state(1, 0);
        let mut snapshot = depth(100, 3100.0);
        snapshot.ask_price1 = f64::MAX;
        let parsed = parse_depth(&snapshot, &mut st, 1.into()).unwrap();
        assert!(parsed.quote.is_none());
    }

    #[rstest]
    fn test_first_snapshot_sets_baseline_without_trade() {
        let mut st = state(1, 0);
        let parsed = parse_depth(&depth(100, 3100.0), &mut st, 1.into()).unwrap();
        assert!(parsed.trade.is_none());
        assert_eq!(st.last_volume, Some(100));
    }

    #[rstest]
    fn test_volume_increase_produces_trade_of_delta() {
        let mut st = state(1, 0);
        parse_depth(&depth(100, 3100.0), &mut st, 1.into()).unwrap();
        let parsed = parse_depth(&depth(105, 3100.2), &mut st, 2.into()).unwrap();
        let trade = parsed.trade.unwrap();
        assert_eq!(trade.price.to_string(), "3100.2");
        assert_eq!(trade.size.to_string(), "5");
        assert_eq!(trade.aggressor_side, AggressorSide::NoAggressor);
        assert_eq!(trade.trade_id.to_string(), "20251122-105");
    }

    #[rstest]
    fn test_unchanged_volume_produces_no_trade() {
        let mut st = state(1, 0);
        parse_depth(&depth(100, 3100.0), &mut st, 1.into()).unwrap();
        let parsed = parse_depth(&depth(100, 3100.2), &mut st, 2.into()).unwrap();
        assert!(parsed.trade.is_none());
    }

    #[rstest]
    fn test_volume_reset_rebaselines_without_trade() {
        let mut st = state(1, 0);
        parse_depth(&depth(5000, 3100.0), &mut st, 1.into()).unwrap();
        let parsed = parse_depth(&depth(3, 3100.0), &mut st, 2.into()).unwrap();
        assert!(parsed.trade.is_none());
        assert_eq!(st.last_volume, Some(3));
    }
}
