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

//! [NautilusTrader](https://nautilustrader.io) adapter for [CTP](https://www.sfit.com.cn/), the
//! Comprehensive Transaction Platform used by Chinese futures exchanges.
//!
//! The `nautilus-ctp` crate provides live market data (quotes and inferred trades), order entry and
//! cancellation, order, fill, position, and account reports, and Python bindings for the
//! configuration types and client factories. It reaches the market through the openctp
//! CTP-compatible SDK. Instruments are declared in configuration because the feed carries no
//! contract reference data.
//!
//! # Feature Flags
//!
//! - `ctp-native`: Links the CTP SDK. Requires the `CTP_SDK_DIR` and `CTP_LIB_DIR` environment
//!   variables at build time. Without it the clients build and fail on `connect`.
//! - `extension-module`: Builds as a Python extension module.
//! - `high-precision` (default): Enables
//!   [high-precision mode](https://nautilustrader.io/docs/nightly/getting_started/installation/#precision-mode)
//!   to use 128-bit value types.
//! - `python`: Enables the Python bindings from [PyO3](https://pyo3.rs).

#![warn(rustc::all)]
#![deny(unsafe_code)]
#![deny(nonstandard_style)]
#![deny(missing_debug_implementations)]
#![deny(clippy::missing_panics_doc)]
#![allow(
    unknown_lints,
    reason = "Tolerate nightly-only clippy lint names in crate attributes on the pinned stable toolchain"
)]

pub mod config;
pub mod data;
pub mod exec;
pub mod factories;
pub mod instruments;
pub mod md;
pub mod native;
#[cfg(feature = "python")]
pub mod python;
pub mod td;

pub use config::{CtpDataClientConfig, CtpExecClientConfig, CtpInstrumentDefinition};
pub use data::CtpDataClient;
pub use exec::CtpExecutionClient;
pub use factories::{CtpDataClientFactory, CtpExecutionClientFactory};
