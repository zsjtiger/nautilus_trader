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

//! Python constructors for CTP configuration types.

use std::str::FromStr;

use nautilus_core::{python::to_pyvalue_err, string::secret::SecretString};
use nautilus_model::enums::AssetClass;
use pyo3::prelude::*;
use rust_decimal::Decimal;

use crate::config::{CtpDataClientConfig, CtpExecClientConfig, CtpInstrumentDefinition};

fn parse_decimal(value: &str, name: &str) -> PyResult<Decimal> {
    Decimal::from_str(value).map_err(|e| to_pyvalue_err(format!("invalid `{name}` `{value}`: {e}")))
}

#[pymethods]
#[pyo3_stub_gen::derive::gen_stub_pymethods]
impl CtpInstrumentDefinition {
    /// Declares one CTP futures instrument.
    ///
    /// `price_increment` and `multiplier` are decimal strings, such as `"0.1"` and `"10"`, so the
    /// price precision is exact.
    #[new]
    #[expect(
        clippy::too_many_arguments,
        reason = "thin projection of the Rust definition fields"
    )]
    fn py_new(
        ctp_symbol: String,
        exchange: String,
        underlying: String,
        asset_class: AssetClass,
        currency: String,
        expiry_date: String,
        price_increment: &str,
        multiplier: &str,
    ) -> PyResult<Self> {
        let definition = Self {
            ctp_symbol,
            exchange,
            underlying,
            asset_class,
            currency,
            expiry_date,
            price_increment: parse_decimal(price_increment, "price_increment")?,
            multiplier: parse_decimal(multiplier, "multiplier")?,
        };
        definition.validate().map_err(to_pyvalue_err)?;
        Ok(definition)
    }

    fn __repr__(&self) -> String {
        format!("{self:?}")
    }

    fn __str__(&self) -> String {
        self.__repr__()
    }
}

#[pymethods]
#[pyo3_stub_gen::derive::gen_stub_pymethods]
impl CtpDataClientConfig {
    /// Configuration for the CTP market data client.
    ///
    /// `flow_path` is the directory for SDK session files. The default is the working directory.
    #[new]
    #[pyo3(signature = (
        broker_id,
        user_id,
        password,
        front_address,
        instruments,
        flow_path = String::new(),
        production_mode = true,
        login_timeout_secs = 30,
    ))]
    #[expect(
        clippy::too_many_arguments,
        reason = "thin projection of the Rust config fields"
    )]
    fn py_new(
        broker_id: String,
        user_id: String,
        password: String,
        front_address: String,
        instruments: Vec<CtpInstrumentDefinition>,
        flow_path: String,
        production_mode: bool,
        login_timeout_secs: u64,
    ) -> PyResult<Self> {
        let config = Self {
            broker_id,
            user_id,
            password: SecretString::from(password),
            front_address,
            flow_path,
            production_mode,
            login_timeout_secs,
            instruments,
        };
        config.validate().map_err(to_pyvalue_err)?;
        Ok(config)
    }

    fn __repr__(&self) -> String {
        format!("{self:?}")
    }

    fn __str__(&self) -> String {
        self.__repr__()
    }
}

#[pymethods]
#[pyo3_stub_gen::derive::gen_stub_pymethods]
impl CtpExecClientConfig {
    /// Configuration for the CTP trading client.
    ///
    /// `app_id` and `auth_code` are both required for terminal authentication, and both must be
    /// omitted when the front does not require it.
    #[new]
    #[pyo3(signature = (
        broker_id,
        investor_id,
        password,
        trade_front_address,
        instruments,
        app_id = None,
        auth_code = None,
        product_info = String::new(),
        flow_path = String::new(),
        production_mode = true,
        login_timeout_secs = 30,
        query_timeout_secs = 30,
    ))]
    #[expect(
        clippy::too_many_arguments,
        reason = "thin projection of the Rust config fields"
    )]
    fn py_new(
        broker_id: String,
        investor_id: String,
        password: String,
        trade_front_address: String,
        instruments: Vec<CtpInstrumentDefinition>,
        app_id: Option<String>,
        auth_code: Option<String>,
        product_info: String,
        flow_path: String,
        production_mode: bool,
        login_timeout_secs: u64,
        query_timeout_secs: u64,
    ) -> PyResult<Self> {
        let config = Self {
            broker_id,
            investor_id,
            password: SecretString::from(password),
            trade_front_address,
            app_id,
            auth_code: auth_code.map(SecretString::from),
            product_info,
            flow_path,
            production_mode,
            login_timeout_secs,
            query_timeout_secs,
            instruments,
        };
        config.validate().map_err(to_pyvalue_err)?;
        Ok(config)
    }

    fn __repr__(&self) -> String {
        format!("{self:?}")
    }

    fn __str__(&self) -> String {
        self.__repr__()
    }
}
