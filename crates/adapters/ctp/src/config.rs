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

//! Configuration for the CTP market data client.

use std::collections::HashSet;

use nautilus_core::string::secret::SecretString;
use nautilus_model::enums::AssetClass;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Describes one CTP futures instrument subscribed by the market data client.
///
/// CTP market data carries no contract reference data, so each instrument is declared here.
/// The fields follow the exchange contract specification for the product.
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader._libnautilus.ctp", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.ctp")
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CtpInstrumentDefinition {
    /// The CTP instrument symbol used on the wire, e.g. `rb2510`.
    pub ctp_symbol: String,
    /// The exchange code, e.g. `SHFE`. Forms the venue suffix of the instrument ID.
    pub exchange: String,
    /// The underlying product code, e.g. `rb`.
    pub underlying: String,
    /// The asset class: `Commodity`, `Index`, `Equity`, `Debt`, or `FX`.
    pub asset_class: AssetClass,
    /// The settlement currency code, e.g. `CNY`.
    pub currency: String,
    /// The last trading day as `YYYYMMDD`. Expiration is stamped at 15:00 exchange time on this day.
    pub expiry_date: String,
    /// The minimum price step. Its scale sets the price precision.
    pub price_increment: Decimal,
    /// The contract multiplier, in currency units per price point.
    pub multiplier: Decimal,
}

/// Configuration for the CTP market data client.
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader._libnautilus.ctp", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.ctp")
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CtpDataClientConfig {
    /// The broker ID assigned by the futures company.
    pub broker_id: String,
    /// The investor account number.
    pub user_id: String,
    /// The account password. Redacted in debug output.
    pub password: SecretString,
    /// The market data front address, e.g. `tcp://180.168.146.187:10131`.
    pub front_address: String,
    /// Directory where the SDK stores session flow files. Created by the SDK if missing.
    pub flow_path: String,
    /// Whether the SDK runs in production mode (`true`) or against a test environment (`false`).
    pub production_mode: bool,
    /// Seconds to wait for the first successful login before `connect` fails.
    pub login_timeout_secs: u64,
    /// Instruments the client can subscribe to.
    pub instruments: Vec<CtpInstrumentDefinition>,
}

impl CtpDataClientConfig {
    /// Validates the configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if a required field is empty, the login timeout is zero, the instrument
    /// list is empty, or an instrument definition is invalid or duplicated.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.broker_id.trim().is_empty(),
            "`broker_id` must not be empty"
        );
        anyhow::ensure!(
            !self.user_id.trim().is_empty(),
            "`user_id` must not be empty"
        );
        anyhow::ensure!(
            !self.front_address.trim().is_empty(),
            "`front_address` must not be empty"
        );
        anyhow::ensure!(
            self.login_timeout_secs > 0,
            "`login_timeout_secs` must be positive"
        );
        anyhow::ensure!(
            !self.instruments.is_empty(),
            "`instruments` must contain at least one definition"
        );

        let mut seen = HashSet::new();
        for definition in &self.instruments {
            definition.validate()?;
            anyhow::ensure!(
                seen.insert(definition.ctp_symbol.as_str()),
                "duplicate CTP instrument `{}`",
                definition.ctp_symbol
            );
        }
        Ok(())
    }
}

impl CtpInstrumentDefinition {
    /// Validates the definition.
    ///
    /// # Errors
    ///
    /// Returns an error if a symbol or code is empty or not ASCII alphanumeric, the expiry date
    /// is not `YYYYMMDD`, or a numeric field is not positive.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            is_identifier(&self.ctp_symbol),
            "invalid CTP symbol `{}`",
            self.ctp_symbol
        );
        anyhow::ensure!(
            is_identifier(&self.exchange),
            "invalid exchange `{}`",
            self.exchange
        );
        anyhow::ensure!(
            is_identifier(&self.underlying),
            "invalid underlying `{}`",
            self.underlying
        );
        anyhow::ensure!(
            is_identifier(&self.currency),
            "invalid currency `{}`",
            self.currency
        );
        anyhow::ensure!(
            self.expiry_date.len() == 8 && self.expiry_date.bytes().all(|b| b.is_ascii_digit()),
            "expiry date must be YYYYMMDD: `{}`",
            self.expiry_date
        );
        anyhow::ensure!(
            self.price_increment > Decimal::ZERO,
            "`price_increment` must be positive"
        );
        anyhow::ensure!(
            self.multiplier > Decimal::ZERO,
            "`multiplier` must be positive"
        );
        Ok(())
    }
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Configuration for the CTP trading (execution) client.
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader._libnautilus.ctp", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.ctp")
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CtpExecClientConfig {
    /// The broker ID assigned by the futures company.
    pub broker_id: String,
    /// The investor account number.
    pub investor_id: String,
    /// The account password. Redacted in debug output.
    pub password: SecretString,
    /// The trading front address, e.g. `tcp://180.168.146.187:10130`.
    pub trade_front_address: String,
    /// Application ID for terminal authentication. Required together with `auth_code`.
    pub app_id: Option<String>,
    /// Authentication code for terminal authentication. Required together with `app_id`.
    pub auth_code: Option<SecretString>,
    /// Product information sent at login.
    pub product_info: String,
    /// Directory where the SDK stores session flow files. Created by the SDK if missing.
    pub flow_path: String,
    /// Whether the SDK runs in production mode (`true`) or against a test environment (`false`).
    pub production_mode: bool,
    /// Seconds to wait for trading login before `connect` fails.
    pub login_timeout_secs: u64,
    /// Seconds to wait for a query response before the report request fails.
    pub query_timeout_secs: u64,
    /// Instruments the client can trade.
    pub instruments: Vec<CtpInstrumentDefinition>,
}

impl CtpExecClientConfig {
    /// Validates the configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if a required field is empty, a timeout is zero, authentication is only
    /// partly configured, the instrument list is empty, or an instrument definition is invalid
    /// or duplicated.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.broker_id.trim().is_empty(),
            "`broker_id` must not be empty"
        );
        anyhow::ensure!(
            !self.investor_id.trim().is_empty(),
            "`investor_id` must not be empty"
        );
        anyhow::ensure!(
            !self.trade_front_address.trim().is_empty(),
            "`trade_front_address` must not be empty"
        );
        anyhow::ensure!(
            self.app_id.is_some() == self.auth_code.is_some(),
            "`app_id` and `auth_code` must be set together"
        );
        anyhow::ensure!(
            self.login_timeout_secs > 0,
            "`login_timeout_secs` must be positive"
        );
        anyhow::ensure!(
            self.query_timeout_secs > 0,
            "`query_timeout_secs` must be positive"
        );
        anyhow::ensure!(
            !self.instruments.is_empty(),
            "`instruments` must contain at least one definition"
        );

        let mut seen = HashSet::new();
        for definition in &self.instruments {
            definition.validate()?;
            anyhow::ensure!(
                seen.insert(definition.ctp_symbol.as_str()),
                "duplicate CTP instrument `{}`",
                definition.ctp_symbol
            );
        }
        Ok(())
    }
}
