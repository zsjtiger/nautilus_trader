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

//! Factories that create CTP data and execution clients for a live node.

use std::{any::Any, cell::RefCell, rc::Rc};

use anyhow::Context;
use nautilus_common::{
    cache::CacheView,
    clients::{DataClient, ExecutionClient},
    clock::Clock,
    factories::{ClientConfig, DataClientFactory, ExecutionClientFactory},
    live::runner::{try_get_data_event_sender, try_get_exec_event_sender},
};
use nautilus_execution::client::core::ExecutionClientCore;
use nautilus_model::{
    enums::{AccountType, OmsType},
    identifiers::{AccountId, ClientId, TraderId, Venue},
    types::Currency,
};

use crate::{
    config::{CtpDataClientConfig, CtpExecClientConfig},
    data::CtpDataClient,
    exec::{CTP_VENUE, CtpExecutionClient},
};

/// Name the CTP factories are registered under.
pub const CTP: &str = "CTP";

impl ClientConfig for CtpDataClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl ClientConfig for CtpExecClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Factory for CTP market data clients.
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader._libnautilus.ctp", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.ctp")
)]
#[derive(Debug, Clone, Default)]
pub struct CtpDataClientFactory;

impl DataClientFactory for CtpDataClientFactory {
    fn create(
        &self,
        name: &str,
        config: &dyn ClientConfig,
        _cache: CacheView,
        _clock: Rc<RefCell<dyn Clock>>,
    ) -> anyhow::Result<Box<dyn DataClient>> {
        let config = config
            .as_any()
            .downcast_ref::<CtpDataClientConfig>()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid config type for CtpDataClientFactory. Expected CtpDataClientConfig, was {config:?}"
                )
            })?;
        let sender = try_get_data_event_sender().context("data event sender is not installed")?;
        Ok(Box::new(CtpDataClient::new(
            ClientId::from(name),
            config.clone(),
            sender,
        )?))
    }

    fn name(&self) -> &str {
        CTP
    }

    fn config_type(&self) -> &'static str {
        "CtpDataClientConfig"
    }
}

/// Factory for CTP execution clients.
#[cfg_attr(
    feature = "python",
    pyo3::pyclass(module = "nautilus_trader._libnautilus.ctp", from_py_object)
)]
#[cfg_attr(
    feature = "python",
    pyo3_stub_gen::derive::gen_stub_pyclass(module = "nautilus_trader.adapters.ctp")
)]
#[derive(Debug, Clone, Default)]
pub struct CtpExecutionClientFactory;

impl ExecutionClientFactory for CtpExecutionClientFactory {
    fn create(
        &self,
        trader_id: TraderId,
        name: &str,
        config: &dyn ClientConfig,
        cache: CacheView,
        _clock: Rc<RefCell<dyn Clock>>,
    ) -> anyhow::Result<Box<dyn ExecutionClient>> {
        let config = config
            .as_any()
            .downcast_ref::<CtpExecClientConfig>()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid config type for CtpExecutionClientFactory. Expected CtpExecClientConfig, was {config:?}"
                )
            })?;

        let account_id = AccountId::from(format!("CTP-{}", config.investor_id).as_str());
        let core = ExecutionClientCore::new(
            trader_id,
            ClientId::from(name),
            Venue::from(CTP_VENUE),
            OmsType::Netting,
            account_id,
            AccountType::Margin,
            Some(Currency::CNY()),
            cache,
        );
        let mut client = CtpExecutionClient::new(core, config.clone())?;
        if let Some(sender) = try_get_exec_event_sender() {
            client.set_event_sender(sender);
        }
        Ok(Box::new(client))
    }

    fn name(&self) -> &str {
        CTP
    }

    fn config_type(&self) -> &'static str {
        "CtpExecClientConfig"
    }
}
