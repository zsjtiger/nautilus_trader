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

//! Provides a configuration for `RiskEngine` instances.

use ahash::{AHashMap, AHashSet};
use nautilus_common::{
    config::{ConfigError, ConfigErrorCollector, ConfigResult},
    throttler::RateLimit,
};
use nautilus_core::DurationNanos;
use nautilus_model::identifiers::{InstrumentId, Venue};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Configuration for `RiskEngineConfig` instances.
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader.risk", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.risk")
)]
#[cfg_attr(
    feature = "python",
    expect(
        clippy::unsafe_derive_deserialize,
        reason = "config deserializes plain fields; unsafe methods come from generated PyO3 integration"
    )
)]
#[derive(Debug, Clone, Deserialize, Serialize, bon::Builder)]
#[builder(finish_fn(name = build_inner, vis = ""))]
#[serde(default, deny_unknown_fields)]
pub struct RiskEngineConfig {
    /// Whether to bypass risk checks and order rate limits.
    #[builder(default)]
    pub bypass: bool,
    /// Rate limit for order submission commands.
    #[builder(default = RateLimit::new(100, DurationNanos::from_secs(1)))]
    pub max_order_submit: RateLimit,
    /// Rate limit for order modifications, counting each batch child separately.
    #[builder(default = RateLimit::new(100, DurationNanos::from_secs(1)))]
    pub max_order_modify: RateLimit,
    /// Maximum notional per order by instrument, in each instrument's quote currency.
    #[builder(default)]
    pub max_notional_per_order: AHashMap<InstrumentId, Decimal>,
    /// Venues whose execution clients enforce whole-position conditional exits.
    ///
    /// Validated exits skip bounds that apply only to their placeholder quantity and notional.
    #[builder(default)]
    pub full_position_exit_venues: AHashSet<Venue>,
    /// Venues whose orders must have a quantity that is a whole multiple of the instrument lot size.
    ///
    /// Instruments without a lot size are not checked. Empty by default, so lot sizes are not
    /// enforced unless a venue is listed here.
    #[builder(default)]
    pub lot_size_enforced_venues: AHashSet<Venue>,
    /// Venues whose sells must respect T+1 settlement for shares bought on the same trading day.
    ///
    /// A trading day runs from midnight in UTC+8 (China Standard Time). Empty by default.
    #[builder(default)]
    pub t_plus_one_venues: AHashSet<Venue>,
    /// Venues whose limit orders must fall within the daily price limits.
    ///
    /// Limits are computed from the previous trading day's close: 20% for STAR and `ChiNext` codes
    /// (300, 301, 688) and 10% otherwise. Empty by default.
    #[builder(default)]
    pub price_limit_venues: AHashSet<Venue>,
    /// Daily price limit ratios that override the board default for specific instruments.
    ///
    /// Use `0.05` for ST stocks, which trade within 5%. Empty by default.
    #[builder(default)]
    pub price_limit_ratios: AHashMap<InstrumentId, Decimal>,
    /// Last trading day, inclusive, on which an instrument is exempt from the daily price limits.
    ///
    /// Set this from the exchange calendar for new listings, as a trading day index. Use
    /// [`parse_trading_day`] to convert an ISO date such as `2026-11-02`. Empty by default.
    #[builder(default)]
    pub price_limit_exempt_until: AHashMap<InstrumentId, u64>,
    /// Whether to emit additional debug logs.
    #[builder(default)]
    pub debug: bool,
}

impl<S: risk_engine_config_builder::IsComplete> RiskEngineConfigBuilder<S> {
    /// Validates and builds the [`RiskEngineConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if any field fails validation (see [`RiskEngineConfig::validate`]).
    pub fn build(self) -> ConfigResult<RiskEngineConfig> {
        let config = self.build_inner();
        config.validate()?;
        Ok(config)
    }
}

/// Parses an ISO date (`YYYY-MM-DD`) into a trading day index.
///
/// The index is the number of days since 1970-01-01 in UTC+8, matching the trading day used by the
/// risk engine. Rejects dates that do not exist in the calendar, such as `2026-02-30`.
///
/// # Errors
///
/// Returns an error if the string is not a valid calendar date or precedes 1970-01-01.
pub fn parse_trading_day(date: &str) -> Result<u64, String> {
    let invalid = || format!("invalid trading day {date:?}, expected YYYY-MM-DD");
    let mut parts = date.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid());
    };
    let year: i64 = year.parse().map_err(|_| invalid())?;
    let month: i64 = month.parse().map_err(|_| invalid())?;
    let day: i64 = day.parse().map_err(|_| invalid())?;

    let days = days_from_civil(year, month, day);
    if civil_from_days(days) != (year, month, day) {
        return Err(invalid());
    }
    u64::try_from(days).map_err(|_| invalid())
}

/// Formats a trading day index as an ISO date (`YYYY-MM-DD`).
///
/// Indices beyond `i64::MAX` days are not representable and clamp to the largest date.
#[must_use]
pub fn format_trading_day(trading_day: u64) -> String {
    let days = i64::try_from(trading_day).unwrap_or(i64::MAX);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

// Inverse of `days_from_civil`, returning (year, month, day).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

impl RiskEngineConfig {
    /// Validates the risk engine configuration, collecting every field violation.
    ///
    /// # Errors
    ///
    /// Returns an error if any field fails validation. Multiple validation failures are collected
    /// into one error.
    pub fn validate(&self) -> ConfigResult<()> {
        let mut errors = ConfigErrorCollector::new();

        for (instrument_id, notional) in &self.max_notional_per_order {
            errors.check(
                *notional > Decimal::ZERO,
                ConfigError::range(
                    "max_notional_per_order",
                    format!("notional for {instrument_id} must be positive, was {notional}"),
                ),
            );
        }

        errors.into_result()
    }
}

impl Default for RiskEngineConfig {
    fn default() -> Self {
        Self::builder()
            .build()
            .expect("default `RiskEngineConfig` should be valid")
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn test_parse_trading_day_round_trips_iso_dates() {
        assert_eq!(parse_trading_day("1970-01-01"), Ok(0));
        assert_eq!(format_trading_day(0), "1970-01-01");

        let index = parse_trading_day("2026-11-02").unwrap();
        assert_eq!(format_trading_day(index), "2026-11-02");
    }

    #[rstest]
    #[case("2026-02-30")]
    #[case("2026-13-01")]
    #[case("2026-11")]
    #[case("2026-11-02-01")]
    #[case("x-y-z")]
    #[case("1969-12-31")]
    fn test_parse_trading_day_rejects_invalid_dates(#[case] date: &str) {
        assert!(parse_trading_day(date).is_err());
    }

    #[rstest]
    fn test_default_config_is_valid() {
        let config = RiskEngineConfig::builder().build().unwrap();

        assert!(config.full_position_exit_venues.is_empty());
        assert!(config.lot_size_enforced_venues.is_empty());
        assert!(config.t_plus_one_venues.is_empty());
        assert!(config.price_limit_venues.is_empty());
    }

    #[rstest]
    #[case(Decimal::ZERO)]
    #[case(Decimal::from(-1))]
    fn test_non_positive_notional_rejected(#[case] notional: Decimal) {
        let mut notionals = AHashMap::new();
        notionals.insert(InstrumentId::from("ESZ21.GLBX"), notional);
        let result = RiskEngineConfig::builder()
            .max_notional_per_order(notionals)
            .build();
        assert!(
            matches!(result, Err(ConfigError::Range { field, .. }) if field == "max_notional_per_order")
        );
    }

    #[rstest]
    fn test_positive_notional_accepted() {
        let mut notionals = AHashMap::new();
        notionals.insert(InstrumentId::from("ESZ21.GLBX"), Decimal::from(1_000_000));
        let result = RiskEngineConfig::builder()
            .max_notional_per_order(notionals)
            .build();
        assert!(result.is_ok());
    }

    #[rstest]
    fn test_multiple_violations_collected() {
        let mut notionals = AHashMap::new();
        notionals.insert(InstrumentId::from("ESZ21.GLBX"), Decimal::ZERO);
        notionals.insert(InstrumentId::from("CLZ21.NYMEX"), Decimal::from(-1));
        let result = RiskEngineConfig::builder()
            .max_notional_per_order(notionals)
            .build();
        let ConfigError::Multiple { errors } = result.unwrap_err() else {
            panic!("expected ConfigError::Multiple");
        };
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().all(
            |e| matches!(e, ConfigError::Range { field, .. } if field == "max_notional_per_order")
        ));
    }
}
