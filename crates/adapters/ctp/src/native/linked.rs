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

//! Native market data backend over the CTP-compatible SDK, enabled by the `ctp-native` feature.
//!
//! The only `unsafe` in this crate is here. It calls the C ABI in `native/ctp_md_shim.h`, which
//! wraps the SDK's C++ classes.

#![allow(unsafe_code)]

use std::{
    ffi::{CStr, CString, c_char, c_void},
    ptr::NonNull,
};

use crate::{
    md::{DepthMarketData, MdBackend, MdEvent, RspError},
    td::{
        AccountRecord, LoginInfo, OrderRecord, PositionRecord, QueryResponse, TdEvent, TradeRecord,
    },
};

/// Event kinds, matching `NautilusCtpMdEventKind` in the shim header.
const EVENT_FRONT_CONNECTED: i32 = 1;
const EVENT_FRONT_DISCONNECTED: i32 = 2;
const EVENT_HEARTBEAT_WARNING: i32 = 3;
const EVENT_LOGIN: i32 = 4;
const EVENT_LOGOUT: i32 = 5;
const EVENT_SUBSCRIBE: i32 = 6;
const EVENT_UNSUBSCRIBE: i32 = 7;
const EVENT_RSP_ERROR: i32 = 8;
const EVENT_DEPTH: i32 = 9;

/// Mirrors `NautilusCtpMdEvent` in the shim header. Field order and types must match.
#[repr(C)]
struct RawEvent {
    kind: i32,
    ok: i32,
    error_id: i32,
    request_id: i32,
    reason: i32,
    error_msg: [c_char; 128],
    trading_day: [c_char; 16],
    instrument_id: [c_char; 32],
    exchange_id: [c_char; 16],
    action_day: [c_char; 16],
    update_time: [c_char; 16],
    update_millisec: i32,
    last_price: f64,
    bid_price1: f64,
    bid_volume1: i32,
    ask_price1: f64,
    ask_volume1: i32,
    volume: i32,
    turnover: f64,
}

/// Opaque handle type, matching `NautilusCtpMdApi` in the shim header.
#[repr(C)]
struct RawApi {
    _private: [u8; 0],
}

type RawCallback = unsafe extern "C" fn(*mut c_void, *const RawEvent);

unsafe extern "C" {
    fn nautilus_ctp_md_api_version() -> *const c_char;
    fn nautilus_ctp_md_create(
        flow_path: *const c_char,
        production_mode: i32,
        callback: RawCallback,
        user_data: *mut c_void,
    ) -> *mut RawApi;
    fn nautilus_ctp_md_release(api: *mut RawApi);
    fn nautilus_ctp_md_register_front(api: *mut RawApi, address: *const c_char) -> i32;
    fn nautilus_ctp_md_init(api: *mut RawApi) -> i32;
    fn nautilus_ctp_md_login(
        api: *mut RawApi,
        broker_id: *const c_char,
        user_id: *const c_char,
        password: *const c_char,
        request_id: i32,
    ) -> i32;
    fn nautilus_ctp_md_logout(
        api: *mut RawApi,
        broker_id: *const c_char,
        user_id: *const c_char,
        request_id: i32,
    ) -> i32;
    fn nautilus_ctp_md_subscribe(
        api: *mut RawApi,
        instrument_ids: *const *const c_char,
        count: i32,
    ) -> i32;
    fn nautilus_ctp_md_unsubscribe(
        api: *mut RawApi,
        instrument_ids: *const *const c_char,
        count: i32,
    ) -> i32;
}

/// Returns the version string reported by the linked SDK.
fn sdk_version() -> String {
    // SAFETY: The shim returns a static string owned by the SDK.
    let ptr = unsafe { nautilus_ctp_md_api_version() };
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: The pointer is a valid NUL-terminated string for the process lifetime.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Owns one SDK market data instance and forwards its callbacks to a channel.
#[derive(Debug)]
pub struct NativeMdApi {
    api: NonNull<RawApi>,
    // Heap-allocated sender whose address is passed to the SDK as callback user data.
    // Freed only after the SDK instance has been released.
    events: *mut tokio::sync::mpsc::UnboundedSender<MdEvent>,
}

// SAFETY: The SDK permits API calls from any thread. The session serializes all access behind a
// mutex, and the callback path only sends on a channel, which is thread-safe.
unsafe impl Send for NativeMdApi {}

impl NativeMdApi {
    /// Creates an SDK instance that delivers events to `events`.
    ///
    /// # Errors
    ///
    /// Returns an error if the flow path is invalid or the SDK fails to create the instance.
    pub fn create(
        flow_path: &str,
        production_mode: bool,
        events: tokio::sync::mpsc::UnboundedSender<MdEvent>,
    ) -> anyhow::Result<Self> {
        let flow_path = to_cstring(flow_path, "flow path")?;
        let events = Box::into_raw(Box::new(events));

        // SAFETY: `flow_path` is a valid C string for the duration of the call. `events` stays
        // allocated until `Drop` releases the SDK instance, which the SDK requires before any
        // callback can stop being delivered.
        let raw = unsafe {
            nautilus_ctp_md_create(
                flow_path.as_ptr(),
                i32::from(production_mode),
                on_event,
                events.cast::<c_void>(),
            )
        };

        match NonNull::new(raw) {
            Some(api) => {
                tracing::info!(sdk_version = %sdk_version(), "created CTP market data API");
                Ok(Self { api, events })
            }
            None => {
                // SAFETY: No SDK instance holds the pointer, so reclaiming the box is sound.
                drop(unsafe { Box::from_raw(events) });
                anyhow::bail!("failed to create CTP market data API instance")
            }
        }
    }

    fn check(&self, code: i32, operation: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            code == 0,
            "CTP {operation} request rejected with code {code}"
        );
        Ok(())
    }
}

impl MdBackend for NativeMdApi {
    fn register_front(&mut self, address: &str) -> anyhow::Result<()> {
        let address = to_cstring(address, "front address")?;
        // SAFETY: The handle is valid until `Drop`, and `address` outlives the call.
        let code = unsafe { nautilus_ctp_md_register_front(self.api.as_ptr(), address.as_ptr()) };
        self.check(code, "register front")
    }

    fn init(&mut self) -> anyhow::Result<()> {
        // SAFETY: The handle is valid until `Drop`.
        let code = unsafe { nautilus_ctp_md_init(self.api.as_ptr()) };
        self.check(code, "init")
    }

    fn login(
        &mut self,
        broker_id: &str,
        user_id: &str,
        password: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        let broker_id = to_cstring(broker_id, "broker ID")?;
        let user_id = to_cstring(user_id, "user ID")?;
        let password = to_cstring(password, "password")?;
        // SAFETY: The handle is valid until `Drop`, and all strings outlive the call.
        let code = unsafe {
            nautilus_ctp_md_login(
                self.api.as_ptr(),
                broker_id.as_ptr(),
                user_id.as_ptr(),
                password.as_ptr(),
                request_id,
            )
        };
        self.check(code, "login")
    }

    fn logout(&mut self, broker_id: &str, user_id: &str, request_id: i32) -> anyhow::Result<()> {
        let broker_id = to_cstring(broker_id, "broker ID")?;
        let user_id = to_cstring(user_id, "user ID")?;
        // SAFETY: The handle is valid until `Drop`, and all strings outlive the call.
        let code = unsafe {
            nautilus_ctp_md_logout(
                self.api.as_ptr(),
                broker_id.as_ptr(),
                user_id.as_ptr(),
                request_id,
            )
        };
        self.check(code, "logout")
    }

    fn subscribe(&mut self, instrument_ids: &[String]) -> anyhow::Result<()> {
        self.send_instrument_list(instrument_ids, nautilus_ctp_md_subscribe, "subscribe")
    }

    fn unsubscribe(&mut self, instrument_ids: &[String]) -> anyhow::Result<()> {
        self.send_instrument_list(instrument_ids, nautilus_ctp_md_unsubscribe, "unsubscribe")
    }
}

impl NativeMdApi {
    fn send_instrument_list(
        &mut self,
        instrument_ids: &[String],
        call: unsafe extern "C" fn(*mut RawApi, *const *const c_char, i32) -> i32,
        operation: &str,
    ) -> anyhow::Result<()> {
        if instrument_ids.is_empty() {
            return Ok(());
        }
        let owned = instrument_ids
            .iter()
            .map(|id| to_cstring(id, "instrument ID"))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let pointers: Vec<*const c_char> = owned.iter().map(|id| id.as_ptr()).collect();
        let count = i32::try_from(pointers.len())?;

        // SAFETY: The handle is valid until `Drop`. `pointers` and `owned` both outlive the call,
        // and the shim copies each string before handing it to the SDK.
        let code = unsafe { call(self.api.as_ptr(), pointers.as_ptr(), count) };
        self.check(code, operation)
    }
}

impl Drop for NativeMdApi {
    fn drop(&mut self) {
        // SAFETY: The handle was created by `create` and is released exactly once. The SDK stops
        // invoking callbacks when release returns, so the sender can be freed afterwards.
        unsafe { nautilus_ctp_md_release(self.api.as_ptr()) };
        // SAFETY: `events` came from `Box::into_raw` in `create`, and no callback can run now.
        drop(unsafe { Box::from_raw(self.events) });
    }
}

/// Converts SDK callbacks to [`MdEvent`]s. Runs on the SDK thread.
///
/// # Safety
///
/// `user_data` must point to a live `tokio::sync::mpsc::UnboundedSender<MdEvent>` and `event` to a valid
/// `RawEvent`. The shim guarantees both for the duration of the call.
unsafe extern "C" fn on_event(user_data: *mut c_void, event: *const RawEvent) {
    if user_data.is_null() || event.is_null() {
        return;
    }
    // SAFETY: Guaranteed by the caller contract above.
    let sender = unsafe { &*user_data.cast::<tokio::sync::mpsc::UnboundedSender<MdEvent>>() };
    // SAFETY: Guaranteed by the caller contract above.
    let raw = unsafe { &*event };

    if let Some(event) = convert(raw) {
        // A closed channel means the session is shutting down, so the event can be dropped.
        let _ = sender.send(event);
    }
}

fn convert(raw: &RawEvent) -> Option<MdEvent> {
    let response = || {
        if raw.ok == 1 {
            Ok(())
        } else {
            Err(RspError {
                error_id: raw.error_id,
                message: decode(&raw.error_msg),
            })
        }
    };

    match raw.kind {
        EVENT_FRONT_CONNECTED => Some(MdEvent::FrontConnected),
        EVENT_FRONT_DISCONNECTED => Some(MdEvent::FrontDisconnected { reason: raw.reason }),
        EVENT_HEARTBEAT_WARNING => Some(MdEvent::HeartbeatWarning {
            time_lapse: raw.reason,
        }),
        EVENT_LOGIN => Some(MdEvent::Login(match response() {
            Ok(()) => Ok(decode(&raw.trading_day)),
            Err(e) => Err(e),
        })),
        EVENT_LOGOUT => Some(MdEvent::Logout(response())),
        EVENT_SUBSCRIBE => Some(MdEvent::Subscribe {
            instrument_id: decode(&raw.instrument_id),
            result: response(),
        }),
        EVENT_UNSUBSCRIBE => Some(MdEvent::Unsubscribe {
            instrument_id: decode(&raw.instrument_id),
            result: response(),
        }),
        EVENT_RSP_ERROR => match response() {
            Err(e) => Some(MdEvent::Error(e)),
            Ok(()) => None,
        },
        EVENT_DEPTH => Some(MdEvent::Depth(Box::new(DepthMarketData {
            instrument_id: decode(&raw.instrument_id),
            exchange_id: decode(&raw.exchange_id),
            action_day: decode(&raw.action_day),
            update_time: decode(&raw.update_time),
            update_millisec: raw.update_millisec,
            last_price: raw.last_price,
            bid_price1: raw.bid_price1,
            bid_volume1: raw.bid_volume1,
            ask_price1: raw.ask_price1,
            ask_volume1: raw.ask_volume1,
            volume: raw.volume,
            turnover: raw.turnover,
        }))),
        _ => None,
    }
}

/// Decodes a NUL-terminated SDK buffer. Invalid UTF-8 (for example GBK error text) is replaced.
fn decode(buffer: &[c_char]) -> String {
    // SAFETY: `c_char` and `u8` have the same size and alignment, and the slice is only read.
    let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len()) };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn to_cstring(value: &str, name: &str) -> anyhow::Result<CString> {
    CString::new(value).map_err(|_| anyhow::anyhow!("{name} must not contain NUL bytes"))
}

// ---------------------------------------------------------------------------------------------
// Trader API
// ---------------------------------------------------------------------------------------------

const TD_EVENT_FRONT_CONNECTED: i32 = 1;
const TD_EVENT_FRONT_DISCONNECTED: i32 = 2;
const TD_EVENT_HEARTBEAT_WARNING: i32 = 3;
const TD_EVENT_AUTHENTICATE: i32 = 4;
const TD_EVENT_LOGIN: i32 = 5;
const TD_EVENT_LOGOUT: i32 = 6;
const TD_EVENT_SETTLEMENT_CONFIRM: i32 = 7;
const TD_EVENT_ORDER_INSERT_RSP: i32 = 8;
const TD_EVENT_ORDER_INSERT_ERR: i32 = 9;
const TD_EVENT_ORDER_ACTION_RSP: i32 = 10;
const TD_EVENT_ORDER_ACTION_ERR: i32 = 11;
const TD_EVENT_ORDER: i32 = 12;
const TD_EVENT_TRADE: i32 = 13;
const TD_EVENT_QRY_ORDER: i32 = 14;
const TD_EVENT_QRY_TRADE: i32 = 15;
const TD_EVENT_QRY_POSITION: i32 = 16;
const TD_EVENT_QRY_ACCOUNT: i32 = 17;
const TD_EVENT_RSP_ERROR: i32 = 18;

/// Mirrors `NautilusCtpTdEvent` in `ctp_td_shim.h`. Field order and types must match exactly.
#[repr(C)]
struct RawTdEvent {
    kind: i32,
    ok: i32,
    error_id: i32,
    request_id: i32,
    is_last: i32,
    reason: i32,
    has_record: i32,
    error_msg: [c_char; 128],
    trading_day: [c_char; 16],
    front_id: i32,
    session_id: i32,
    max_order_ref: [c_char; 16],
    broker_id: [c_char; 16],
    investor_id: [c_char; 32],
    instrument_id: [c_char; 32],
    exchange_id: [c_char; 16],
    order_ref: [c_char; 16],
    order_sys_id: [c_char; 32],
    order_status: c_char,
    order_submit_status: c_char,
    direction: c_char,
    offset_flag: c_char,
    status_msg: [c_char; 128],
    insert_date: [c_char; 16],
    insert_time: [c_char; 16],
    update_time: [c_char; 16],
    volume_total: i32,
    volume_original: i32,
    volume_traded: i32,
    limit_price: f64,
    trade_id: [c_char; 32],
    trade_price: f64,
    trade_volume: i32,
    trade_date: [c_char; 16],
    trade_time: [c_char; 16],
    pos_direction: c_char,
    position: i32,
    yd_position: i32,
    position_cost: f64,
    account_id: [c_char; 32],
    currency_id: [c_char; 8],
    balance: f64,
    available: f64,
    curr_margin: f64,
    commission: f64,
    position_profit: f64,
    close_profit: f64,
    deposit: f64,
    withdraw: f64,
}

/// Mirrors `NautilusCtpOrderInsert` in `ctp_td_shim.h`.
#[repr(C)]
struct RawOrderInsert {
    broker_id: [c_char; 16],
    investor_id: [c_char; 32],
    instrument_id: [c_char; 32],
    exchange_id: [c_char; 16],
    order_ref: [c_char; 16],
    direction: c_char,
    price_type: c_char,
    limit_price: f64,
    volume: i32,
    time_condition: c_char,
    volume_condition: c_char,
    offset_flag: c_char,
}

/// Mirrors `NautilusCtpOrderCancel` in `ctp_td_shim.h`.
#[repr(C)]
struct RawOrderCancel {
    broker_id: [c_char; 16],
    investor_id: [c_char; 32],
    instrument_id: [c_char; 32],
    exchange_id: [c_char; 16],
    order_ref: [c_char; 16],
    front_id: i32,
    session_id: i32,
    order_sys_id: [c_char; 32],
}

type RawTdCallback = unsafe extern "C" fn(*mut c_void, *const RawTdEvent);

unsafe extern "C" {
    fn nautilus_ctp_td_create(
        flow_path: *const c_char,
        production_mode: i32,
        callback: RawTdCallback,
        user_data: *mut c_void,
    ) -> *mut RawApi;
    fn nautilus_ctp_td_release(api: *mut RawApi);
    fn nautilus_ctp_td_register_front(api: *mut RawApi, address: *const c_char) -> i32;
    fn nautilus_ctp_td_subscribe_private_topic(api: *mut RawApi, resume_type: i32) -> i32;
    fn nautilus_ctp_td_subscribe_public_topic(api: *mut RawApi, resume_type: i32) -> i32;
    fn nautilus_ctp_td_init(api: *mut RawApi) -> i32;
    fn nautilus_ctp_td_authenticate(
        api: *mut RawApi,
        broker_id: *const c_char,
        user_id: *const c_char,
        app_id: *const c_char,
        auth_code: *const c_char,
        product_info: *const c_char,
        request_id: i32,
    ) -> i32;
    fn nautilus_ctp_td_login(
        api: *mut RawApi,
        broker_id: *const c_char,
        user_id: *const c_char,
        password: *const c_char,
        product_info: *const c_char,
        request_id: i32,
    ) -> i32;
    fn nautilus_ctp_td_logout(
        api: *mut RawApi,
        broker_id: *const c_char,
        user_id: *const c_char,
        request_id: i32,
    ) -> i32;
    fn nautilus_ctp_td_settlement_confirm(
        api: *mut RawApi,
        broker_id: *const c_char,
        investor_id: *const c_char,
        request_id: i32,
    ) -> i32;
    fn nautilus_ctp_td_insert_order(
        api: *mut RawApi,
        order: *const RawOrderInsert,
        request_id: i32,
    ) -> i32;
    fn nautilus_ctp_td_cancel_order(
        api: *mut RawApi,
        cancel: *const RawOrderCancel,
        request_id: i32,
    ) -> i32;
    fn nautilus_ctp_td_query_orders(
        api: *mut RawApi,
        broker_id: *const c_char,
        investor_id: *const c_char,
        request_id: i32,
    ) -> i32;
    fn nautilus_ctp_td_query_trades(
        api: *mut RawApi,
        broker_id: *const c_char,
        investor_id: *const c_char,
        request_id: i32,
    ) -> i32;
    fn nautilus_ctp_td_query_positions(
        api: *mut RawApi,
        broker_id: *const c_char,
        investor_id: *const c_char,
        request_id: i32,
    ) -> i32;
    fn nautilus_ctp_td_query_account(
        api: *mut RawApi,
        broker_id: *const c_char,
        investor_id: *const c_char,
        request_id: i32,
    ) -> i32;
}

/// Owns one SDK trader instance and forwards its callbacks to a channel.
#[derive(Debug)]
pub struct NativeTdApi {
    api: NonNull<RawApi>,
    events: *mut tokio::sync::mpsc::UnboundedSender<TdEvent>,
}

// SAFETY: The SDK permits API calls from any thread. The session serializes all access behind a
// mutex, and the callback path only sends on a channel.
unsafe impl Send for NativeTdApi {}

impl NativeTdApi {
    /// Creates an SDK trader instance that delivers events to `events`.
    ///
    /// # Errors
    ///
    /// Returns an error if the flow path is invalid or the SDK fails to create the instance.
    pub fn create(
        flow_path: &str,
        production_mode: bool,
        events: tokio::sync::mpsc::UnboundedSender<TdEvent>,
    ) -> anyhow::Result<Self> {
        let flow_path = to_cstring(flow_path, "flow path")?;
        let events = Box::into_raw(Box::new(events));

        // SAFETY: `flow_path` is a valid C string for the call. `events` stays allocated until
        // `Drop` releases the SDK instance, which stops callbacks first.
        let raw = unsafe {
            nautilus_ctp_td_create(
                flow_path.as_ptr(),
                i32::from(production_mode),
                on_td_event,
                events.cast::<c_void>(),
            )
        };

        match NonNull::new(raw) {
            Some(api) => {
                tracing::info!(sdk_version = %td_sdk_version(), "created CTP trader API");
                Ok(Self { api, events })
            }
            None => {
                // SAFETY: No SDK instance holds the pointer, so reclaiming the box is sound.
                drop(unsafe { Box::from_raw(events) });
                anyhow::bail!("failed to create CTP trader API instance")
            }
        }
    }

    fn check(code: i32, operation: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            code == 0,
            "CTP {operation} request rejected with code {code}"
        );
        Ok(())
    }
}

fn resume_code(resume: crate::td::ResumeType) -> i32 {
    match resume {
        crate::td::ResumeType::Restart => 0,
        crate::td::ResumeType::Quick => 1,
        crate::td::ResumeType::Resume => 2,
    }
}

impl crate::td::TdBackend for NativeTdApi {
    fn register_front(&mut self, address: &str) -> anyhow::Result<()> {
        let address = to_cstring(address, "trade front address")?;
        // SAFETY: The handle is valid until `Drop`, and `address` outlives the call.
        let code = unsafe { nautilus_ctp_td_register_front(self.api.as_ptr(), address.as_ptr()) };
        Self::check(code, "register front")
    }

    fn subscribe_topics(
        &mut self,
        private: crate::td::ResumeType,
        public: crate::td::ResumeType,
    ) -> anyhow::Result<()> {
        // SAFETY: The handle is valid until `Drop`.
        let code = unsafe {
            nautilus_ctp_td_subscribe_private_topic(self.api.as_ptr(), resume_code(private))
        };
        Self::check(code, "subscribe private topic")?;
        // SAFETY: The handle is valid until `Drop`.
        let code = unsafe {
            nautilus_ctp_td_subscribe_public_topic(self.api.as_ptr(), resume_code(public))
        };
        Self::check(code, "subscribe public topic")
    }

    fn init(&mut self) -> anyhow::Result<()> {
        // SAFETY: The handle is valid until `Drop`.
        let code = unsafe { nautilus_ctp_td_init(self.api.as_ptr()) };
        Self::check(code, "init")
    }

    fn authenticate(
        &mut self,
        broker_id: &str,
        user_id: &str,
        app_id: &str,
        auth_code: &str,
        product_info: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        let broker_id = to_cstring(broker_id, "broker ID")?;
        let user_id = to_cstring(user_id, "investor ID")?;
        let app_id = to_cstring(app_id, "app ID")?;
        let auth_code = to_cstring(auth_code, "auth code")?;
        let product_info = to_cstring(product_info, "product info")?;
        // SAFETY: The handle is valid until `Drop`, and all strings outlive the call.
        let code = unsafe {
            nautilus_ctp_td_authenticate(
                self.api.as_ptr(),
                broker_id.as_ptr(),
                user_id.as_ptr(),
                app_id.as_ptr(),
                auth_code.as_ptr(),
                product_info.as_ptr(),
                request_id,
            )
        };
        Self::check(code, "authenticate")
    }

    fn login(
        &mut self,
        broker_id: &str,
        user_id: &str,
        password: &str,
        product_info: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        let broker_id = to_cstring(broker_id, "broker ID")?;
        let user_id = to_cstring(user_id, "investor ID")?;
        let password = to_cstring(password, "password")?;
        let product_info = to_cstring(product_info, "product info")?;
        // SAFETY: The handle is valid until `Drop`, and all strings outlive the call.
        let code = unsafe {
            nautilus_ctp_td_login(
                self.api.as_ptr(),
                broker_id.as_ptr(),
                user_id.as_ptr(),
                password.as_ptr(),
                product_info.as_ptr(),
                request_id,
            )
        };
        Self::check(code, "login")
    }

    fn logout(&mut self, broker_id: &str, user_id: &str, request_id: i32) -> anyhow::Result<()> {
        let broker_id = to_cstring(broker_id, "broker ID")?;
        let user_id = to_cstring(user_id, "investor ID")?;
        // SAFETY: The handle is valid until `Drop`, and all strings outlive the call.
        let code = unsafe {
            nautilus_ctp_td_logout(
                self.api.as_ptr(),
                broker_id.as_ptr(),
                user_id.as_ptr(),
                request_id,
            )
        };
        Self::check(code, "logout")
    }

    fn confirm_settlement(
        &mut self,
        broker_id: &str,
        investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        let broker_id = to_cstring(broker_id, "broker ID")?;
        let investor_id = to_cstring(investor_id, "investor ID")?;
        // SAFETY: The handle is valid until `Drop`, and all strings outlive the call.
        let code = unsafe {
            nautilus_ctp_td_settlement_confirm(
                self.api.as_ptr(),
                broker_id.as_ptr(),
                investor_id.as_ptr(),
                request_id,
            )
        };
        Self::check(code, "settlement confirm")
    }

    fn insert_order(
        &mut self,
        order: &crate::td::OrderInsertRequest,
        request_id: i32,
    ) -> anyhow::Result<()> {
        let raw = RawOrderInsert {
            broker_id: fixed(&order.broker_id, "broker ID")?,
            investor_id: fixed(&order.investor_id, "investor ID")?,
            instrument_id: fixed(&order.instrument_id, "instrument ID")?,
            exchange_id: fixed(&order.exchange_id, "exchange ID")?,
            order_ref: fixed(&order.order_ref, "order reference")?,
            direction: order.direction as c_char,
            price_type: order.price_type as c_char,
            limit_price: order.limit_price,
            volume: order.volume,
            time_condition: order.time_condition as c_char,
            volume_condition: order.volume_condition as c_char,
            offset_flag: order.offset_flag as c_char,
        };
        // SAFETY: The handle is valid until `Drop`. `raw` is a fully initialized struct that
        // outlives the call, and the shim copies it before returning.
        let code = unsafe {
            nautilus_ctp_td_insert_order(self.api.as_ptr(), std::ptr::from_ref(&raw), request_id)
        };
        Self::check(code, "insert order")
    }

    fn cancel_order(
        &mut self,
        cancel: &crate::td::OrderCancelRequest,
        request_id: i32,
    ) -> anyhow::Result<()> {
        let raw = RawOrderCancel {
            broker_id: fixed(&cancel.broker_id, "broker ID")?,
            investor_id: fixed(&cancel.investor_id, "investor ID")?,
            instrument_id: fixed(&cancel.instrument_id, "instrument ID")?,
            exchange_id: fixed(&cancel.exchange_id, "exchange ID")?,
            order_ref: fixed(&cancel.order_ref, "order reference")?,
            front_id: cancel.front_id,
            session_id: cancel.session_id,
            order_sys_id: fixed(&cancel.order_sys_id, "order system ID")?,
        };
        // SAFETY: As for `insert_order`.
        let code = unsafe {
            nautilus_ctp_td_cancel_order(self.api.as_ptr(), std::ptr::from_ref(&raw), request_id)
        };
        Self::check(code, "cancel order")
    }

    fn query_orders(
        &mut self,
        broker_id: &str,
        investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        let broker_id = to_cstring(broker_id, "broker ID")?;
        let investor_id = to_cstring(investor_id, "investor ID")?;
        // SAFETY: The handle is valid until `Drop`, and all strings outlive the call.
        let code = unsafe {
            nautilus_ctp_td_query_orders(
                self.api.as_ptr(),
                broker_id.as_ptr(),
                investor_id.as_ptr(),
                request_id,
            )
        };
        Self::check(code, "query orders")
    }

    fn query_trades(
        &mut self,
        broker_id: &str,
        investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        let broker_id = to_cstring(broker_id, "broker ID")?;
        let investor_id = to_cstring(investor_id, "investor ID")?;
        // SAFETY: The handle is valid until `Drop`, and all strings outlive the call.
        let code = unsafe {
            nautilus_ctp_td_query_trades(
                self.api.as_ptr(),
                broker_id.as_ptr(),
                investor_id.as_ptr(),
                request_id,
            )
        };
        Self::check(code, "query trades")
    }

    fn query_positions(
        &mut self,
        broker_id: &str,
        investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        let broker_id = to_cstring(broker_id, "broker ID")?;
        let investor_id = to_cstring(investor_id, "investor ID")?;
        // SAFETY: The handle is valid until `Drop`, and all strings outlive the call.
        let code = unsafe {
            nautilus_ctp_td_query_positions(
                self.api.as_ptr(),
                broker_id.as_ptr(),
                investor_id.as_ptr(),
                request_id,
            )
        };
        Self::check(code, "query positions")
    }

    fn query_account(
        &mut self,
        broker_id: &str,
        investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        let broker_id = to_cstring(broker_id, "broker ID")?;
        let investor_id = to_cstring(investor_id, "investor ID")?;
        // SAFETY: The handle is valid until `Drop`, and all strings outlive the call.
        let code = unsafe {
            nautilus_ctp_td_query_account(
                self.api.as_ptr(),
                broker_id.as_ptr(),
                investor_id.as_ptr(),
                request_id,
            )
        };
        Self::check(code, "query account")
    }
}

impl Drop for NativeTdApi {
    fn drop(&mut self) {
        // SAFETY: Created by `create` and released exactly once. Callbacks stop when release
        // returns, so the sender can be freed afterwards.
        unsafe { nautilus_ctp_td_release(self.api.as_ptr()) };
        // SAFETY: `events` came from `Box::into_raw` in `create`, and no callback can run now.
        drop(unsafe { Box::from_raw(self.events) });
    }
}

fn td_sdk_version() -> String {
    // SAFETY: The shim returns a static string owned by the SDK.
    let ptr = unsafe { nautilus_ctp_td_api_version() };
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: The pointer is a valid NUL-terminated string for the process lifetime.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

unsafe extern "C" {
    fn nautilus_ctp_td_api_version() -> *const c_char;
}

/// Converts trader SDK callbacks to [`TdEvent`]s. Runs on the SDK thread.
///
/// # Safety
///
/// `user_data` must point to a live `UnboundedSender<TdEvent>` and `event` to a valid
/// `RawTdEvent`. The shim guarantees both for the duration of the call.
unsafe extern "C" fn on_td_event(user_data: *mut c_void, event: *const RawTdEvent) {
    if user_data.is_null() || event.is_null() {
        return;
    }
    // SAFETY: Guaranteed by the caller contract above.
    let sender = unsafe { &*user_data.cast::<tokio::sync::mpsc::UnboundedSender<TdEvent>>() };
    // SAFETY: Guaranteed by the caller contract above.
    let raw = unsafe { &*event };

    if let Some(event) = convert_td(raw) {
        // A closed channel means the session is shutting down, so the event can be dropped.
        let _ = sender.send(event);
    }
}

fn rsp_error(raw: &RawTdEvent) -> RspError {
    RspError {
        error_id: raw.error_id,
        message: decode(&raw.error_msg),
    }
}

fn rsp_result(raw: &RawTdEvent) -> Result<(), RspError> {
    if raw.ok == 1 {
        Ok(())
    } else {
        Err(rsp_error(raw))
    }
}

fn char_of(value: c_char) -> char {
    char::from(value as u8)
}

fn order_record(raw: &RawTdEvent) -> OrderRecord {
    OrderRecord {
        front_id: raw.front_id,
        session_id: raw.session_id,
        order_ref: decode(&raw.order_ref),
        exchange_id: decode(&raw.exchange_id),
        order_sys_id: decode(&raw.order_sys_id),
        instrument_id: decode(&raw.instrument_id),
        direction: char_of(raw.direction),
        order_status: char_of(raw.order_status),
        order_submit_status: char_of(raw.order_submit_status),
        volume_original: raw.volume_original,
        volume_traded: raw.volume_traded,
        volume_remaining: raw.volume_total,
        limit_price: raw.limit_price,
        status_msg: decode(&raw.status_msg),
        insert_date: decode(&raw.insert_date),
        insert_time: decode(&raw.insert_time),
        update_time: decode(&raw.update_time),
        trading_day: decode(&raw.trading_day),
    }
}

fn trade_record(raw: &RawTdEvent) -> TradeRecord {
    TradeRecord {
        trade_id: decode(&raw.trade_id),
        order_ref: decode(&raw.order_ref),
        order_sys_id: decode(&raw.order_sys_id),
        exchange_id: decode(&raw.exchange_id),
        instrument_id: decode(&raw.instrument_id),
        direction: char_of(raw.direction),
        offset_flag: char_of(raw.offset_flag),
        price: raw.trade_price,
        volume: raw.trade_volume,
        trade_date: decode(&raw.trade_date),
        trade_time: decode(&raw.trade_time),
        trading_day: decode(&raw.trading_day),
    }
}

fn query_response<T>(raw: &RawTdEvent, record: impl FnOnce() -> T) -> QueryResponse<T> {
    let result = if raw.ok != 1 {
        Err(rsp_error(raw))
    } else if raw.has_record == 1 {
        Ok(Some(record()))
    } else {
        Ok(None)
    };
    QueryResponse {
        request_id: raw.request_id,
        is_last: raw.is_last == 1,
        result,
    }
}

fn convert_td(raw: &RawTdEvent) -> Option<TdEvent> {
    Some(match raw.kind {
        TD_EVENT_FRONT_CONNECTED => TdEvent::FrontConnected,
        TD_EVENT_FRONT_DISCONNECTED => TdEvent::FrontDisconnected { reason: raw.reason },
        TD_EVENT_HEARTBEAT_WARNING => TdEvent::HeartbeatWarning {
            time_lapse: raw.reason,
        },
        TD_EVENT_AUTHENTICATE => TdEvent::Authenticate {
            request_id: raw.request_id,
            result: rsp_result(raw),
        },
        TD_EVENT_LOGIN => TdEvent::Login {
            request_id: raw.request_id,
            result: match rsp_result(raw) {
                Ok(()) => Ok(LoginInfo {
                    trading_day: decode(&raw.trading_day),
                    front_id: raw.front_id,
                    session_id: raw.session_id,
                    max_order_ref: decode(&raw.max_order_ref),
                }),
                Err(e) => Err(e),
            },
        },
        TD_EVENT_LOGOUT => TdEvent::Logout {
            request_id: raw.request_id,
            result: rsp_result(raw),
        },
        TD_EVENT_SETTLEMENT_CONFIRM => TdEvent::SettlementConfirm {
            request_id: raw.request_id,
            result: rsp_result(raw),
        },
        TD_EVENT_ORDER_INSERT_RSP | TD_EVENT_ORDER_INSERT_ERR => {
            // Success responses to an insert are informational; the order return carries the
            // state. Only failures change the order.
            if raw.ok == 1 {
                return None;
            }
            TdEvent::OrderInsertRejected {
                order_ref: decode(&raw.order_ref),
                error: rsp_error(raw),
            }
        }
        TD_EVENT_ORDER_ACTION_RSP | TD_EVENT_ORDER_ACTION_ERR => {
            if raw.ok == 1 {
                return None;
            }
            TdEvent::OrderActionRejected {
                order_ref: decode(&raw.order_ref),
                order_sys_id: decode(&raw.order_sys_id),
                error: rsp_error(raw),
            }
        }
        TD_EVENT_ORDER => TdEvent::Order(Box::new(order_record(raw))),
        TD_EVENT_TRADE => TdEvent::Trade(Box::new(trade_record(raw))),
        TD_EVENT_QRY_ORDER => TdEvent::QryOrder(query_response(raw, || order_record(raw))),
        TD_EVENT_QRY_TRADE => TdEvent::QryTrade(query_response(raw, || trade_record(raw))),
        TD_EVENT_QRY_POSITION => TdEvent::QryPosition(query_response(raw, || PositionRecord {
            exchange_id: decode(&raw.exchange_id),
            instrument_id: decode(&raw.instrument_id),
            direction: char_of(raw.pos_direction),
            position: raw.position,
            yd_position: raw.yd_position,
            position_cost: raw.position_cost,
        })),
        TD_EVENT_QRY_ACCOUNT => TdEvent::QryAccount(query_response(raw, || AccountRecord {
            account_id: decode(&raw.account_id),
            currency_id: decode(&raw.currency_id),
            trading_day: decode(&raw.trading_day),
            balance: raw.balance,
            available: raw.available,
            curr_margin: raw.curr_margin,
            commission: raw.commission,
            position_profit: raw.position_profit,
            close_profit: raw.close_profit,
            deposit: raw.deposit,
            withdraw: raw.withdraw,
        })),
        TD_EVENT_RSP_ERROR => match rsp_result(raw) {
            Ok(()) => return None,
            Err(e) => TdEvent::Error {
                request_id: raw.request_id,
                error: e,
            },
        },
        _ => return None,
    })
}

/// Encodes a string into a fixed-size C buffer, failing rather than truncating.
fn fixed<const N: usize>(value: &str, name: &str) -> anyhow::Result<[c_char; N]> {
    let bytes = value.as_bytes();
    anyhow::ensure!(bytes.len() < N, "{name} `{value}` exceeds {} bytes", N - 1);
    anyhow::ensure!(!bytes.contains(&0), "{name} must not contain NUL bytes");
    let mut out = [0 as c_char; N];
    for (dst, src) in out.iter_mut().zip(bytes) {
        *dst = *src as c_char;
    }
    Ok(out)
}
