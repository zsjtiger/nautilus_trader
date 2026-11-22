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
//! The `nautilus-ctp` crate provides live market data (quotes and inferred trades) and instrument
//! publication over the CTP market data API, reached through the openctp CTP-compatible SDK.
//! Instruments are declared in configuration because the market data feed carries no contract
//! reference data.
//!
//! # Feature Flags
//!
//! - `ctp-native`: Links the CTP SDK and enables [`CtpDataClient`]. Requires the `CTP_SDK_DIR` and
//!   `CTP_LIB_DIR` environment variables at build time. Without it the crate builds the protocol and
//!   configuration layers only.
//! - `high-precision` (default): Enables
//!   [high-precision mode](https://nautilustrader.io/docs/nightly/getting_started/installation/#precision-mode)
//!   to use 128-bit value types.

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
pub mod instruments;
pub mod md;
pub mod native;

pub use config::{CtpDataClientConfig, CtpInstrumentDefinition};
pub use data::CtpDataClient;
