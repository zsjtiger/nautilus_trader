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

//! Market data protocol boundary for CTP.
//!
//! [`MdEvent`] is the Rust-side projection of the SDK callbacks. [`MdBackend`] is the command
//! surface the session drives. The native SDK implements it behind the `ctp-native` feature.

pub mod parse;
pub mod session;

use std::fmt::Debug;

/// Error reported by the SDK in a response, with the message decoded from its raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RspError {
    pub error_id: i32,
    pub message: String,
}

/// A depth market data snapshot as reported by the SDK.
///
/// Prices stay as the SDK's IEEE-754 doubles at this boundary. Unset levels are reported as
/// `f64::MAX` and are interpreted during parsing.
#[derive(Debug, Clone, PartialEq)]
pub struct DepthMarketData {
    pub instrument_id: String,
    pub exchange_id: String,
    pub action_day: String,
    pub update_time: String,
    pub update_millisec: i32,
    pub last_price: f64,
    pub bid_price1: f64,
    pub bid_volume1: i32,
    pub ask_price1: f64,
    pub ask_volume1: i32,
    pub volume: i32,
    pub turnover: f64,
}

/// An event from the market data SDK, delivered to the session in arrival order.
#[derive(Debug, Clone, PartialEq)]
pub enum MdEvent {
    FrontConnected,
    FrontDisconnected {
        reason: i32,
    },
    HeartbeatWarning {
        time_lapse: i32,
    },
    Login(Result<String, RspError>),
    Logout(Result<(), RspError>),
    Subscribe {
        instrument_id: String,
        result: Result<(), RspError>,
    },
    Unsubscribe {
        instrument_id: String,
        result: Result<(), RspError>,
    },
    Error(RspError),
    Depth(Box<DepthMarketData>),
}

/// Command surface for a CTP market data session.
///
/// Calls return once the SDK accepts the request. Outcomes arrive later as [`MdEvent`]s.
pub trait MdBackend: Send + Debug {
    /// Registers the front address the SDK connects to.
    ///
    /// # Errors
    ///
    /// Returns an error if the address is rejected.
    fn register_front(&mut self, address: &str) -> anyhow::Result<()>;

    /// Starts connecting to the registered front.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK cannot start.
    fn init(&mut self) -> anyhow::Result<()>;

    /// Sends a login request.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request.
    fn login(
        &mut self,
        broker_id: &str,
        user_id: &str,
        password: &str,
        request_id: i32,
    ) -> anyhow::Result<()>;

    /// Sends a logout request.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request.
    fn logout(&mut self, broker_id: &str, user_id: &str, request_id: i32) -> anyhow::Result<()>;

    /// Subscribes to depth market data for the given CTP instrument symbols.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request.
    fn subscribe(&mut self, instrument_ids: &[String]) -> anyhow::Result<()>;

    /// Unsubscribes from depth market data for the given CTP instrument symbols.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request.
    fn unsubscribe(&mut self, instrument_ids: &[String]) -> anyhow::Result<()>;
}
