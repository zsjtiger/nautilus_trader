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

//! Builds Nautilus futures instruments from configured CTP definitions.

use nautilus_core::UnixNanos;
use nautilus_model::{
    identifiers::InstrumentId,
    instruments::FuturesContract,
    types::{Currency, Price, Quantity},
};
use ustr::Ustr;

use crate::{config::CtpInstrumentDefinition, md::parse::exchange_timestamp_ns};

/// Exchange close on the last trading day, used as the contract expiration time.
const EXPIRY_TIME: &str = "15:00:00";

/// Returns the Nautilus instrument ID for a CTP symbol and exchange, e.g. `rb2510.SHFE`.
///
/// # Errors
///
/// Returns an error if the definition is invalid.
pub fn instrument_id(definition: &CtpInstrumentDefinition) -> anyhow::Result<InstrumentId> {
    definition.validate()?;
    let value = format!("{}.{}", definition.ctp_symbol, definition.exchange);
    Ok(InstrumentId::from(value.as_str()))
}

/// Builds a futures contract from a validated definition.
///
/// Price precision comes from the scale of `price_increment`. Size precision is zero because CTP
/// volumes are whole lots. Expiration is 15:00 exchange time on `expiry_date`, and activation is
/// the epoch because the feed does not report it.
///
/// # Errors
///
/// Returns an error if the definition is invalid, the currency is unknown, or a value cannot be
/// represented at the derived precision.
pub fn futures_contract(
    definition: &CtpInstrumentDefinition,
    ts_init: UnixNanos,
) -> anyhow::Result<FuturesContract> {
    let instrument_id = instrument_id(definition)?;
    let price_precision = u8::try_from(definition.price_increment.scale())?;
    let multiplier_precision = u8::try_from(definition.multiplier.scale())?;
    let currency = Currency::try_from_str(&definition.currency)
        .ok_or_else(|| anyhow::anyhow!("unknown currency `{}`", definition.currency))?;
    let expiration_ns = exchange_timestamp_ns(&definition.expiry_date, EXPIRY_TIME, 0)?;

    let contract = FuturesContract::builder()
        .instrument_id(instrument_id)
        .raw_symbol(definition.ctp_symbol.as_str().into())
        .asset_class(definition.asset_class)
        .exchange(Ustr::from(&definition.exchange))
        .underlying(Ustr::from(&definition.underlying))
        .activation_ns(UnixNanos::default())
        .expiration_ns(expiration_ns)
        .currency(currency)
        .price_precision(price_precision)
        .price_increment(Price::from_decimal_dp(
            definition.price_increment,
            price_precision,
        )?)
        .multiplier(Quantity::from_decimal_dp(
            definition.multiplier,
            multiplier_precision,
        )?)
        .lot_size(Quantity::from(1))
        .ts_event(ts_init)
        .ts_init(ts_init)
        .build()?;

    Ok(contract)
}
