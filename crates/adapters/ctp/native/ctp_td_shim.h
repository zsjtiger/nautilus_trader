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

// C ABI over the CTP trader C++ API (`CThostFtdcTraderApi`).
//
// Callbacks arrive on the SDK's internal thread. Each event is valid only for the duration of
// the callback. Request functions return 0 when the SDK accepted the request for sending; the
// outcome arrives later as an event.

#ifndef NAUTILUS_CTP_TD_SHIM_H
#define NAUTILUS_CTP_TD_SHIM_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct NautilusCtpTdApi NautilusCtpTdApi;

typedef enum NautilusCtpTdEventKind {
    NAUTILUS_CTP_TD_EVENT_FRONT_CONNECTED = 1,
    NAUTILUS_CTP_TD_EVENT_FRONT_DISCONNECTED = 2,
    NAUTILUS_CTP_TD_EVENT_HEARTBEAT_WARNING = 3,
    NAUTILUS_CTP_TD_EVENT_AUTHENTICATE = 4,
    NAUTILUS_CTP_TD_EVENT_LOGIN = 5,
    NAUTILUS_CTP_TD_EVENT_LOGOUT = 6,
    NAUTILUS_CTP_TD_EVENT_SETTLEMENT_CONFIRM = 7,
    NAUTILUS_CTP_TD_EVENT_ORDER_INSERT_RSP = 8,
    NAUTILUS_CTP_TD_EVENT_ORDER_INSERT_ERR = 9,
    NAUTILUS_CTP_TD_EVENT_ORDER_ACTION_RSP = 10,
    NAUTILUS_CTP_TD_EVENT_ORDER_ACTION_ERR = 11,
    NAUTILUS_CTP_TD_EVENT_ORDER = 12,
    NAUTILUS_CTP_TD_EVENT_TRADE = 13,
    NAUTILUS_CTP_TD_EVENT_QRY_ORDER = 14,
    NAUTILUS_CTP_TD_EVENT_QRY_TRADE = 15,
    NAUTILUS_CTP_TD_EVENT_QRY_POSITION = 16,
    NAUTILUS_CTP_TD_EVENT_QRY_ACCOUNT = 17,
    NAUTILUS_CTP_TD_EVENT_RSP_ERROR = 18,
} NautilusCtpTdEventKind;

// Flat event. Only the fields relevant to `kind` are set; the rest are zeroed.
// Strings are NUL-terminated and truncated to their capacity.
typedef struct NautilusCtpTdEvent {
    int32_t kind;
    int32_t ok;                    // Response events: 1 when the SDK reported no error.
    int32_t error_id;
    int32_t request_id;
    int32_t is_last;               // Query responses: 1 on the final record.
    int32_t reason;                // FRONT_DISCONNECTED reason, HEARTBEAT_WARNING time lapse.
    int32_t has_record;            // Query responses: 1 when a record is present.
    char error_msg[128];           // Raw SDK bytes (GBK encoded).
    char trading_day[16];          // LOGIN, ORDER, TRADE, QRY_*.
    int32_t front_id;              // LOGIN, ORDER, ORDER_ACTION_*, QRY_ORDER.
    int32_t session_id;            // LOGIN, ORDER, ORDER_ACTION_*, QRY_ORDER.
    char max_order_ref[16];        // LOGIN.
    char broker_id[16];
    char investor_id[32];
    char instrument_id[32];
    char exchange_id[16];
    char order_ref[16];
    char order_sys_id[32];
    char order_status;             // ORDER, QRY_ORDER: OrderStatus character.
    char order_submit_status;      // ORDER, QRY_ORDER, ORDER_INSERT_ERR: OrderSubmitStatus.
    char direction;                // ORDER, TRADE, QRY_ORDER: '0' buy, '1' sell.
    char offset_flag;              // TRADE, QRY_TRADE: '0' open, '1' close, '3' close today, '4' close yesterday.
    char status_msg[128];          // ORDER, QRY_ORDER: exchange status text.
    char insert_date[16];          // ORDER, QRY_ORDER: YYYYMMDD calendar date of insertion.
    char insert_time[16];          // ORDER, QRY_ORDER: HH:MM:SS.
    char update_time[16];          // ORDER, QRY_ORDER: HH:MM:SS.
    int32_t volume_total;          // ORDER, QRY_ORDER: remaining (unfilled) volume.
    int32_t volume_original;       // ORDER, QRY_ORDER, ORDER_INSERT_RSP, ORDER_INSERT_ERR: original volume.
    int32_t volume_traded;         // ORDER, QRY_ORDER.
    double limit_price;            // ORDER, QRY_ORDER.
    char trade_id[32];             // TRADE, QRY_TRADE.
    double trade_price;            // TRADE, QRY_TRADE.
    int32_t trade_volume;          // TRADE, QRY_TRADE.
    char trade_date[16];           // TRADE, QRY_TRADE: YYYYMMDD calendar date of the trade.
    char trade_time[16];           // TRADE, QRY_TRADE: HH:MM:SS.
    char pos_direction;            // QRY_POSITION: '1' net, '2' long, '3' short.
    int32_t position;              // QRY_POSITION.
    int32_t yd_position;           // QRY_POSITION.
    double position_cost;          // QRY_POSITION.
    char account_id[32];           // QRY_ACCOUNT.
    char currency_id[8];           // QRY_ACCOUNT.
    double balance;                // QRY_ACCOUNT.
    double available;              // QRY_ACCOUNT.
    double curr_margin;            // QRY_ACCOUNT.
    double commission;             // QRY_ACCOUNT.
    double position_profit;        // QRY_ACCOUNT.
    double close_profit;           // QRY_ACCOUNT.
    double deposit;                // QRY_ACCOUNT.
    double withdraw;               // QRY_ACCOUNT.
} NautilusCtpTdEvent;

typedef void (*NautilusCtpTdEventCallback)(void* user_data, const NautilusCtpTdEvent* event);

// Order insert request. Defaults that do not vary for this adapter (speculative hedge flag,
// immediate contingency, no force close, minimum volume 1) are set by the shim.
typedef struct NautilusCtpOrderInsert {
    char broker_id[16];
    char investor_id[32];
    char instrument_id[32];
    char exchange_id[16];
    char order_ref[16];
    char direction;                // '0' buy, '1' sell.
    char price_type;               // '1' any price, '2' limit.
    double limit_price;
    int32_t volume;
    char time_condition;           // '1' IOC, '3' GFD, '5' GTC.
    char volume_condition;         // '1' any volume, '3' complete volume (FOK).
    char offset_flag;              // '0' open, '1' close.
} NautilusCtpOrderInsert;

// Order cancel request. Identifies the order by session route and order reference.
typedef struct NautilusCtpOrderCancel {
    char broker_id[16];
    char investor_id[32];
    char instrument_id[32];
    char exchange_id[16];
    char order_ref[16];
    int32_t front_id;
    int32_t session_id;
    char order_sys_id[32];
} NautilusCtpOrderCancel;

// Returns the SDK version string. The pointer is owned by the SDK.
const char* nautilus_ctp_td_api_version(void);

NautilusCtpTdApi* nautilus_ctp_td_create(
    const char* flow_path,
    int32_t production_mode,
    NautilusCtpTdEventCallback callback,
    void* user_data);

// Releases the SDK instance. No callback runs after this call returns.
void nautilus_ctp_td_release(NautilusCtpTdApi* api);

int32_t nautilus_ctp_td_register_front(NautilusCtpTdApi* api, const char* address);

// Resume types: 0 restart, 1 quick, 2 resume, 3 none. Call before `init`.
int32_t nautilus_ctp_td_subscribe_private_topic(NautilusCtpTdApi* api, int32_t resume_type);
int32_t nautilus_ctp_td_subscribe_public_topic(NautilusCtpTdApi* api, int32_t resume_type);
int32_t nautilus_ctp_td_init(NautilusCtpTdApi* api);

int32_t nautilus_ctp_td_authenticate(
    NautilusCtpTdApi* api,
    const char* broker_id,
    const char* user_id,
    const char* app_id,
    const char* auth_code,
    const char* product_info,
    int32_t request_id);

int32_t nautilus_ctp_td_login(
    NautilusCtpTdApi* api,
    const char* broker_id,
    const char* user_id,
    const char* password,
    const char* product_info,
    int32_t request_id);

int32_t nautilus_ctp_td_logout(
    NautilusCtpTdApi* api,
    const char* broker_id,
    const char* user_id,
    int32_t request_id);

int32_t nautilus_ctp_td_settlement_confirm(
    NautilusCtpTdApi* api,
    const char* broker_id,
    const char* investor_id,
    int32_t request_id);

int32_t nautilus_ctp_td_insert_order(
    NautilusCtpTdApi* api,
    const NautilusCtpOrderInsert* order,
    int32_t request_id);

int32_t nautilus_ctp_td_cancel_order(
    NautilusCtpTdApi* api,
    const NautilusCtpOrderCancel* cancel,
    int32_t request_id);

int32_t nautilus_ctp_td_query_orders(
    NautilusCtpTdApi* api,
    const char* broker_id,
    const char* investor_id,
    int32_t request_id);

int32_t nautilus_ctp_td_query_trades(
    NautilusCtpTdApi* api,
    const char* broker_id,
    const char* investor_id,
    int32_t request_id);

int32_t nautilus_ctp_td_query_positions(
    NautilusCtpTdApi* api,
    const char* broker_id,
    const char* investor_id,
    int32_t request_id);

int32_t nautilus_ctp_td_query_account(
    NautilusCtpTdApi* api,
    const char* broker_id,
    const char* investor_id,
    int32_t request_id);

#ifdef __cplusplus
}
#endif

#endif  // NAUTILUS_CTP_TD_SHIM_H
