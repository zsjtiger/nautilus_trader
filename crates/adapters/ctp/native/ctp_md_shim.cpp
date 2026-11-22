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

#include "ctp_md_shim.h"

#include <cstring>
#include <string>
#include <vector>

#include "ThostFtdcMdApi.h"

namespace {

// Copies a NUL-terminated source into a fixed buffer, truncating and always terminating.
template <size_t N>
void copy_str(char (&dst)[N], const char* src) {
    if (src == nullptr) {
        dst[0] = '\0';
        return;
    }
    std::strncpy(dst, src, N - 1);
    dst[N - 1] = '\0';
}

// Bridges SDK callbacks to the Rust callback. Each override fills one flat event and
// forwards it synchronously, so no SDK-owned pointer outlives the call.
class MdSpi final : public CThostFtdcMdSpi {
public:
    MdSpi(NautilusCtpMdEventCallback callback, void* user_data)
        : callback_(callback), user_data_(user_data) {}

    void OnFrontConnected() override {
        NautilusCtpMdEvent event{};
        event.kind = NAUTILUS_CTP_MD_EVENT_FRONT_CONNECTED;
        emit(event);
    }

    void OnFrontDisconnected(int reason) override {
        NautilusCtpMdEvent event{};
        event.kind = NAUTILUS_CTP_MD_EVENT_FRONT_DISCONNECTED;
        event.reason = reason;
        emit(event);
    }

    void OnHeartBeatWarning(int time_lapse) override {
        NautilusCtpMdEvent event{};
        event.kind = NAUTILUS_CTP_MD_EVENT_HEARTBEAT_WARNING;
        event.reason = time_lapse;
        emit(event);
    }

    void OnRspUserLogin(CThostFtdcRspUserLoginField* login,
                        CThostFtdcRspInfoField* info,
                        int request_id,
                        bool /*is_last*/) override {
        NautilusCtpMdEvent event{};
        event.kind = NAUTILUS_CTP_MD_EVENT_LOGIN;
        set_response(event, info, request_id);
        if (login != nullptr) {
            copy_str(event.trading_day, login->TradingDay);
        }
        emit(event);
    }

    void OnRspUserLogout(CThostFtdcUserLogoutField* /*logout*/,
                         CThostFtdcRspInfoField* info,
                         int request_id,
                         bool /*is_last*/) override {
        NautilusCtpMdEvent event{};
        event.kind = NAUTILUS_CTP_MD_EVENT_LOGOUT;
        set_response(event, info, request_id);
        emit(event);
    }

    void OnRspSubMarketData(CThostFtdcSpecificInstrumentField* instrument,
                            CThostFtdcRspInfoField* info,
                            int request_id,
                            bool /*is_last*/) override {
        NautilusCtpMdEvent event{};
        event.kind = NAUTILUS_CTP_MD_EVENT_SUBSCRIBE;
        set_response(event, info, request_id);
        if (instrument != nullptr) {
            copy_str(event.instrument_id, instrument->InstrumentID);
        }
        emit(event);
    }

    void OnRspUnSubMarketData(CThostFtdcSpecificInstrumentField* instrument,
                              CThostFtdcRspInfoField* info,
                              int request_id,
                              bool /*is_last*/) override {
        NautilusCtpMdEvent event{};
        event.kind = NAUTILUS_CTP_MD_EVENT_UNSUBSCRIBE;
        set_response(event, info, request_id);
        if (instrument != nullptr) {
            copy_str(event.instrument_id, instrument->InstrumentID);
        }
        emit(event);
    }

    void OnRspError(CThostFtdcRspInfoField* info, int request_id, bool /*is_last*/) override {
        NautilusCtpMdEvent event{};
        event.kind = NAUTILUS_CTP_MD_EVENT_RSP_ERROR;
        set_response(event, info, request_id);
        emit(event);
    }

    void OnRtnDepthMarketData(CThostFtdcDepthMarketDataField* depth) override {
        if (depth == nullptr) {
            return;
        }
        NautilusCtpMdEvent event{};
        event.kind = NAUTILUS_CTP_MD_EVENT_DEPTH;
        copy_str(event.instrument_id, depth->InstrumentID);
        copy_str(event.exchange_id, depth->ExchangeID);
        copy_str(event.action_day, depth->ActionDay);
        copy_str(event.update_time, depth->UpdateTime);
        event.update_millisec = depth->UpdateMillisec;
        event.last_price = depth->LastPrice;
        event.bid_price1 = depth->BidPrice1;
        event.bid_volume1 = depth->BidVolume1;
        event.ask_price1 = depth->AskPrice1;
        event.ask_volume1 = depth->AskVolume1;
        event.volume = depth->Volume;
        event.turnover = depth->Turnover;
        emit(event);
    }

private:
    static void set_response(NautilusCtpMdEvent& event, CThostFtdcRspInfoField* info, int request_id) {
        event.request_id = request_id;
        if (info != nullptr) {
            event.error_id = info->ErrorID;
            event.ok = info->ErrorID == 0 ? 1 : 0;
            copy_str(event.error_msg, info->ErrorMsg);
        } else {
            event.ok = 1;
        }
    }

    void emit(const NautilusCtpMdEvent& event) const {
        if (callback_ != nullptr) {
            callback_(user_data_, &event);
        }
    }

    NautilusCtpMdEventCallback callback_;
    void* user_data_;
};

}  // namespace

struct NautilusCtpMdApi {
    CThostFtdcMdApi* api;
    MdSpi* spi;
};

extern "C" {

const char* nautilus_ctp_md_api_version(void) {
    return CThostFtdcMdApi::GetApiVersion();
}

NautilusCtpMdApi* nautilus_ctp_md_create(
    const char* flow_path,
    int32_t production_mode,
    NautilusCtpMdEventCallback callback,
    void* user_data) {
    if (callback == nullptr) {
        return nullptr;
    }
    CThostFtdcMdApi* api = CThostFtdcMdApi::CreateFtdcMdApi(
        flow_path == nullptr ? "" : flow_path,
        false,
        false,
        production_mode != 0);
    if (api == nullptr) {
        return nullptr;
    }
    MdSpi* spi = new MdSpi(callback, user_data);
    api->RegisterSpi(spi);
    return new NautilusCtpMdApi{api, spi};
}

void nautilus_ctp_md_release(NautilusCtpMdApi* handle) {
    if (handle == nullptr) {
        return;
    }
    // The SDK stops invoking callbacks once Release returns, so the SPI can be freed after it.
    handle->api->Release();
    delete handle->spi;
    delete handle;
}

int32_t nautilus_ctp_md_register_front(NautilusCtpMdApi* handle, const char* address) {
    if (handle == nullptr || address == nullptr) {
        return -1;
    }
    // The SDK signature takes a non-const pointer but does not modify the string.
    std::string buffer(address);
    handle->api->RegisterFront(&buffer[0]);
    return 0;
}

int32_t nautilus_ctp_md_init(NautilusCtpMdApi* handle) {
    if (handle == nullptr) {
        return -1;
    }
    handle->api->Init();
    return 0;
}

int32_t nautilus_ctp_md_login(
    NautilusCtpMdApi* handle,
    const char* broker_id,
    const char* user_id,
    const char* password,
    int32_t request_id) {
    if (handle == nullptr || broker_id == nullptr || user_id == nullptr || password == nullptr) {
        return -1;
    }
    CThostFtdcReqUserLoginField request{};
    copy_str(request.BrokerID, broker_id);
    copy_str(request.UserID, user_id);
    copy_str(request.Password, password);
    return handle->api->ReqUserLogin(&request, request_id);
}

int32_t nautilus_ctp_md_logout(
    NautilusCtpMdApi* handle,
    const char* broker_id,
    const char* user_id,
    int32_t request_id) {
    if (handle == nullptr || broker_id == nullptr || user_id == nullptr) {
        return -1;
    }
    CThostFtdcUserLogoutField request{};
    copy_str(request.BrokerID, broker_id);
    copy_str(request.UserID, user_id);
    return handle->api->ReqUserLogout(&request, request_id);
}

int32_t nautilus_ctp_md_subscribe(
    NautilusCtpMdApi* handle,
    const char* const* instrument_ids,
    int32_t count) {
    if (handle == nullptr || (count > 0 && instrument_ids == nullptr)) {
        return -1;
    }
    // The SDK takes a mutable array of mutable strings but does not modify them. Copy so the
    // caller's constant strings are never written through.
    std::vector<std::string> storage(instrument_ids, instrument_ids + count);
    std::vector<char*> pointers(static_cast<size_t>(count));
    for (int32_t i = 0; i < count; ++i) {
        pointers[i] = &storage[i][0];
    }
    return handle->api->SubscribeMarketData(pointers.data(), count);
}

int32_t nautilus_ctp_md_unsubscribe(
    NautilusCtpMdApi* handle,
    const char* const* instrument_ids,
    int32_t count) {
    if (handle == nullptr || (count > 0 && instrument_ids == nullptr)) {
        return -1;
    }
    std::vector<std::string> storage(instrument_ids, instrument_ids + count);
    std::vector<char*> pointers(static_cast<size_t>(count));
    for (int32_t i = 0; i < count; ++i) {
        pointers[i] = &storage[i][0];
    }
    return handle->api->UnSubscribeMarketData(pointers.data(), count);
}

}  // extern "C"
