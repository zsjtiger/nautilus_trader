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

use crate::md::{DepthMarketData, MdBackend, MdEvent, RspError};

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
