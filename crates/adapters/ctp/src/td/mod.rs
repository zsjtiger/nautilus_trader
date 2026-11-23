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

//! Trading protocol boundary for CTP.
//!
//! [`TdEvent`] is the Rust projection of the trader SDK callbacks, and [`TdBackend`] is the
//! command surface the session drives. The native SDK implements it behind the `ctp-native`
//! feature.

pub mod parse;
pub mod session;

use std::fmt::Debug;

use crate::md::RspError;

/// Order status codes (`OrderStatus` in the CTP protocol).
pub mod status {
    /// All volume traded.
    pub const ALL_TRADED: char = '0';
    /// Partly traded, still queued.
    pub const PART_TRADED_QUEUEING: char = '1';
    /// Partly traded, no longer queued.
    pub const PART_TRADED_NOT_QUEUEING: char = '2';
    /// Not traded, queued.
    pub const NO_TRADE_QUEUEING: char = '3';
    /// Not traded, no longer queued.
    pub const NO_TRADE_NOT_QUEUEING: char = '4';
    /// Canceled.
    pub const CANCELED: char = '5';
    /// Unknown: the exchange has not yet acknowledged the order.
    pub const UNKNOWN: char = 'a';
}

/// Order submit status codes (`OrderSubmitStatus` in the CTP protocol).
pub mod submit_status {
    /// The insert is pending at the exchange.
    pub const INSERT_SUBMITTED: char = '0';
    /// The cancel is pending at the exchange.
    pub const CANCEL_SUBMITTED: char = '1';
    /// The order was accepted by the exchange.
    pub const ACCEPTED: char = '3';
    /// The insert was rejected.
    pub const INSERT_REJECTED: char = '4';
    /// The cancel was rejected.
    pub const CANCEL_REJECTED: char = '5';
}

/// Direction codes (`Direction` in the CTP protocol).
pub mod direction {
    pub const BUY: char = '0';
    pub const SELL: char = '1';
}

/// Position direction codes (`PosiDirection` in the CTP protocol).
pub mod position_direction {
    pub const NET: char = '1';
    pub const LONG: char = '2';
    pub const SHORT: char = '3';
}

/// Offset flag codes (`OffsetFlag` in the CTP protocol).
pub mod offset {
    pub const OPEN: char = '0';
    pub const CLOSE: char = '1';
    pub const CLOSE_TODAY: char = '3';
    pub const CLOSE_YESTERDAY: char = '4';
}

/// Result of a successful login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginInfo {
    pub trading_day: String,
    pub front_id: i32,
    pub session_id: i32,
    /// Largest order reference already used by this front and session. New references start
    /// above it.
    pub max_order_ref: String,
}

/// An order record from an order return or an order query.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderRecord {
    pub front_id: i32,
    pub session_id: i32,
    pub order_ref: String,
    pub exchange_id: String,
    pub order_sys_id: String,
    pub instrument_id: String,
    pub direction: char,
    pub order_status: char,
    pub order_submit_status: char,
    pub volume_original: i32,
    pub volume_traded: i32,
    pub volume_remaining: i32,
    pub limit_price: f64,
    pub status_msg: String,
    pub insert_date: String,
    pub insert_time: String,
    pub update_time: String,
    pub trading_day: String,
}

/// A trade record from a trade return or a trade query.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeRecord {
    pub trade_id: String,
    pub order_ref: String,
    pub order_sys_id: String,
    pub exchange_id: String,
    pub instrument_id: String,
    pub direction: char,
    pub offset_flag: char,
    pub price: f64,
    pub volume: i32,
    pub trade_date: String,
    pub trade_time: String,
    pub trading_day: String,
}

/// A position record from a position query. CTP reports long and short positions separately.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionRecord {
    pub exchange_id: String,
    pub instrument_id: String,
    pub direction: char,
    pub position: i32,
    pub yd_position: i32,
    pub position_cost: f64,
}

/// An account record from an account query.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountRecord {
    pub account_id: String,
    pub currency_id: String,
    pub trading_day: String,
    pub balance: f64,
    pub available: f64,
    pub curr_margin: f64,
    pub commission: f64,
    pub position_profit: f64,
    pub close_profit: f64,
    pub deposit: f64,
    pub withdraw: f64,
}

/// One response to a query. Records arrive one per response, and `is_last` marks the final one.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryResponse<T> {
    pub request_id: i32,
    pub is_last: bool,
    pub result: Result<Option<T>, RspError>,
}

/// An event from the trader SDK, delivered to the session in arrival order.
#[derive(Debug, Clone, PartialEq)]
pub enum TdEvent {
    FrontConnected,
    FrontDisconnected {
        reason: i32,
    },
    HeartbeatWarning {
        time_lapse: i32,
    },
    Authenticate {
        request_id: i32,
        result: Result<(), RspError>,
    },
    Login {
        request_id: i32,
        result: Result<LoginInfo, RspError>,
    },
    Logout {
        request_id: i32,
        result: Result<(), RspError>,
    },
    SettlementConfirm {
        request_id: i32,
        result: Result<(), RspError>,
    },
    /// An insert was rejected, either synchronously or by the front. Identified by order reference.
    OrderInsertRejected {
        order_ref: String,
        error: RspError,
    },
    /// A cancel was rejected, either synchronously or by the front.
    OrderActionRejected {
        order_ref: String,
        order_sys_id: String,
        error: RspError,
    },
    Order(Box<OrderRecord>),
    Trade(Box<TradeRecord>),
    QryOrder(QueryResponse<OrderRecord>),
    QryTrade(QueryResponse<TradeRecord>),
    QryPosition(QueryResponse<PositionRecord>),
    QryAccount(QueryResponse<AccountRecord>),
    Error {
        request_id: i32,
        error: RspError,
    },
}

/// Insert request for one order, using the protocol field values the session has chosen.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderInsertRequest {
    pub broker_id: String,
    pub investor_id: String,
    pub instrument_id: String,
    pub exchange_id: String,
    pub order_ref: String,
    pub direction: char,
    /// `'1'` any price (market) or `'2'` limit.
    pub price_type: char,
    pub limit_price: f64,
    pub volume: i32,
    /// `'1'` IOC, `'3'` GFD, or `'5'` GTC.
    pub time_condition: char,
    /// `'1'` any volume or `'3'` complete volume.
    pub volume_condition: char,
    pub offset_flag: char,
}

/// Cancel request for one live order, routed by front, session, and order reference.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderCancelRequest {
    pub broker_id: String,
    pub investor_id: String,
    pub instrument_id: String,
    pub exchange_id: String,
    pub order_ref: String,
    pub front_id: i32,
    pub session_id: i32,
    pub order_sys_id: String,
}

/// Session resume mode for private and public topics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeType {
    /// Replay the day's data from the beginning.
    Restart,
    /// Replay data after the last received sequence.
    Resume,
    /// Only data after subscription.
    Quick,
}

/// Command surface for a CTP trading session.
///
/// Calls return once the SDK accepts the request. Outcomes arrive later as [`TdEvent`]s.
pub trait TdBackend: Send + Debug {
    /// Registers the trading front address.
    ///
    /// # Errors
    ///
    /// Returns an error if the address is rejected.
    fn register_front(&mut self, address: &str) -> anyhow::Result<()>;

    /// Sets the private and public topic resume modes. Call before [`TdBackend::init`].
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the call.
    fn subscribe_topics(&mut self, private: ResumeType, public: ResumeType) -> anyhow::Result<()>;

    /// Starts connecting to the registered front.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK cannot start.
    fn init(&mut self) -> anyhow::Result<()>;

    /// Sends an authentication request.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request.
    fn authenticate(
        &mut self,
        broker_id: &str,
        user_id: &str,
        app_id: &str,
        auth_code: &str,
        product_info: &str,
        request_id: i32,
    ) -> anyhow::Result<()>;

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
        product_info: &str,
        request_id: i32,
    ) -> anyhow::Result<()>;

    /// Sends a logout request.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request.
    fn logout(&mut self, broker_id: &str, user_id: &str, request_id: i32) -> anyhow::Result<()>;

    /// Confirms the settlement information for the day, which the exchange requires before
    /// trading.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request.
    fn confirm_settlement(
        &mut self,
        broker_id: &str,
        investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()>;

    /// Sends an order insert.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request synchronously.
    fn insert_order(&mut self, order: &OrderInsertRequest, request_id: i32) -> anyhow::Result<()>;

    /// Sends an order cancel.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request synchronously.
    fn cancel_order(&mut self, cancel: &OrderCancelRequest, request_id: i32) -> anyhow::Result<()>;

    /// Queries the day's orders.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request.
    fn query_orders(
        &mut self,
        broker_id: &str,
        investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()>;

    /// Queries the day's trades.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request.
    fn query_trades(
        &mut self,
        broker_id: &str,
        investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()>;

    /// Queries investor positions.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request.
    fn query_positions(
        &mut self,
        broker_id: &str,
        investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()>;

    /// Queries the trading account.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK rejects the request.
    fn query_account(
        &mut self,
        broker_id: &str,
        investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()>;
}
