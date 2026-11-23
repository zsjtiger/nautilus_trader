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

//! Python bindings for the CTP adapter.

mod config;
mod factories;

use std::sync::OnceLock;

use nautilus_common::factories::{ClientConfig, DataClientFactory, ExecutionClientFactory};
use nautilus_core::python::{to_pyruntime_err, to_pyvalue_err};
use nautilus_system::get_global_pyo3_registry;
use pyo3::prelude::*;

use crate::{
    config::{CtpDataClientConfig, CtpExecClientConfig},
    factories::{CTP, CtpDataClientFactory, CtpExecutionClientFactory},
};

#[expect(clippy::needless_pass_by_value)]
fn extract_ctp_data_factory(
    py: Python<'_>,
    factory: Py<PyAny>,
) -> PyResult<Box<dyn DataClientFactory>> {
    match factory.extract::<CtpDataClientFactory>(py) {
        Ok(factory) => Ok(Box::new(factory)),
        Err(e) => Err(to_pyvalue_err(format!(
            "Failed to extract CtpDataClientFactory: {e}"
        ))),
    }
}

#[expect(clippy::needless_pass_by_value)]
fn extract_ctp_exec_factory(
    py: Python<'_>,
    factory: Py<PyAny>,
) -> PyResult<Box<dyn ExecutionClientFactory>> {
    match factory.extract::<CtpExecutionClientFactory>(py) {
        Ok(factory) => Ok(Box::new(factory)),
        Err(e) => Err(to_pyvalue_err(format!(
            "Failed to extract CtpExecutionClientFactory: {e}"
        ))),
    }
}

#[expect(clippy::needless_pass_by_value)]
fn extract_ctp_data_config(py: Python<'_>, config: Py<PyAny>) -> PyResult<Box<dyn ClientConfig>> {
    match config.extract::<CtpDataClientConfig>(py) {
        Ok(config) => Ok(Box::new(config)),
        Err(e) => Err(to_pyvalue_err(format!(
            "Failed to extract CtpDataClientConfig: {e}"
        ))),
    }
}

#[expect(clippy::needless_pass_by_value)]
fn extract_ctp_exec_config(py: Python<'_>, config: Py<PyAny>) -> PyResult<Box<dyn ClientConfig>> {
    match config.extract::<CtpExecClientConfig>(py) {
        Ok(config) => Ok(Box::new(config)),
        Err(e) => Err(to_pyvalue_err(format!(
            "Failed to extract CtpExecClientConfig: {e}"
        ))),
    }
}

#[pymodule]
pub fn ctp(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add(stringify!(CTP), CTP)?;
    m.add_class::<crate::config::CtpInstrumentDefinition>()?;
    m.add_class::<CtpDataClientConfig>()?;
    m.add_class::<CtpDataClientFactory>()?;
    m.add_class::<CtpExecClientConfig>()?;
    m.add_class::<CtpExecutionClientFactory>()?;
    register_extractors()
}

fn register_extractors() -> PyResult<()> {
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
    REGISTERED
        .get_or_init(register_extractors_once)
        .clone()
        .map_err(to_pyruntime_err)
}

fn register_extractors_once() -> Result<(), String> {
    let registry = get_global_pyo3_registry();
    registry
        .register_factory_extractor(CTP.to_string(), extract_ctp_data_factory)
        .map_err(|e| format!("Failed to register CTP data factory extractor: {e}"))?;
    registry
        .register_exec_factory_extractor(CTP.to_string(), extract_ctp_exec_factory)
        .map_err(|e| format!("Failed to register CTP exec factory extractor: {e}"))?;
    registry
        .register_config_extractor("CtpDataClientConfig".to_string(), extract_ctp_data_config)
        .map_err(|e| format!("Failed to register CTP data config extractor: {e}"))?;
    registry
        .register_config_extractor("CtpExecClientConfig".to_string(), extract_ctp_exec_config)
        .map_err(|e| format!("Failed to register CTP exec config extractor: {e}"))?;
    Ok(())
}
