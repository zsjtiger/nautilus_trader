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

// C ABI over the CTP market data C++ API (`CThostFtdcMdApi`).
//
// The CTP SDK exposes C++ classes with virtual callbacks, which Rust cannot call safely
// without a shim. This header is the only surface the Rust side uses. Callbacks arrive on
// the SDK's internal thread; the event payload is valid only for the duration of the call.

#ifndef NAUTILUS_CTP_MD_SHIM_H
#define NAUTILUS_CTP_MD_SHIM_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct NautilusCtpMdApi NautilusCtpMdApi;

typedef enum NautilusCtpMdEventKind {
    NAUTILUS_CTP_MD_EVENT_FRONT_CONNECTED = 1,
    NAUTILUS_CTP_MD_EVENT_FRONT_DISCONNECTED = 2,
    NAUTILUS_CTP_MD_EVENT_HEARTBEAT_WARNING = 3,
    NAUTILUS_CTP_MD_EVENT_LOGIN = 4,
    NAUTILUS_CTP_MD_EVENT_LOGOUT = 5,
    NAUTILUS_CTP_MD_EVENT_SUBSCRIBE = 6,
    NAUTILUS_CTP_MD_EVENT_UNSUBSCRIBE = 7,
    NAUTILUS_CTP_MD_EVENT_RSP_ERROR = 8,
    NAUTILUS_CTP_MD_EVENT_DEPTH = 9,
} NautilusCtpMdEventKind;

// Flat, fixed-size event. Strings are NUL-terminated and truncated to their capacity.
// Prices are passed as the SDK's IEEE-754 doubles; unset values are reported as DBL_MAX
// by the SDK and are interpreted on the Rust side.
typedef struct NautilusCtpMdEvent {
    int32_t kind;
    int32_t ok;               // Response events: 1 when the SDK reported no error.
    int32_t error_id;         // Response events: SDK error code.
    int32_t request_id;       // Response events: SDK request id.
    int32_t reason;           // FRONT_DISCONNECTED reason, HEARTBEAT_WARNING time lapse.
    char error_msg[128];      // Raw SDK bytes (GBK encoded); decoded on the Rust side.
    char trading_day[16];     // LOGIN.
    char instrument_id[32];   // SUBSCRIBE, UNSUBSCRIBE, DEPTH.
    char exchange_id[16];     // DEPTH.
    char action_day[16];      // DEPTH: YYYYMMDD.
    char update_time[16];     // DEPTH: HH:MM:SS (exchange clock, UTC+8).
    int32_t update_millisec;  // DEPTH.
    double last_price;        // DEPTH.
    double bid_price1;        // DEPTH.
    int32_t bid_volume1;      // DEPTH.
    double ask_price1;        // DEPTH.
    int32_t ask_volume1;      // DEPTH.
    int32_t volume;           // DEPTH: cumulative session volume.
    double turnover;          // DEPTH: cumulative turnover.
} NautilusCtpMdEvent;

typedef void (*NautilusCtpMdEventCallback)(void* user_data, const NautilusCtpMdEvent* event);

// Returns the SDK version string. The pointer is owned by the SDK.
const char* nautilus_ctp_md_api_version(void);

// Creates an SDK instance and registers the event callback. Returns NULL on failure.
// The callback and `user_data` must stay valid until `nautilus_ctp_md_release` returns.
NautilusCtpMdApi* nautilus_ctp_md_create(
    const char* flow_path,
    int32_t production_mode,
    NautilusCtpMdEventCallback callback,
    void* user_data);

// Releases the SDK instance. No callback runs after this call returns.
void nautilus_ctp_md_release(NautilusCtpMdApi* api);

// Each of the following returns 0 on success. The SDK reports request outcomes through
// events, so a 0 return only means the request was accepted for sending.
int32_t nautilus_ctp_md_register_front(NautilusCtpMdApi* api, const char* address);
int32_t nautilus_ctp_md_init(NautilusCtpMdApi* api);
int32_t nautilus_ctp_md_login(
    NautilusCtpMdApi* api,
    const char* broker_id,
    const char* user_id,
    const char* password,
    int32_t request_id);
int32_t nautilus_ctp_md_logout(
    NautilusCtpMdApi* api,
    const char* broker_id,
    const char* user_id,
    int32_t request_id);
int32_t nautilus_ctp_md_subscribe(
    NautilusCtpMdApi* api,
    const char* const* instrument_ids,
    int32_t count);
int32_t nautilus_ctp_md_unsubscribe(
    NautilusCtpMdApi* api,
    const char* const* instrument_ids,
    int32_t count);

#ifdef __cplusplus
}
#endif

#endif  // NAUTILUS_CTP_MD_SHIM_H
