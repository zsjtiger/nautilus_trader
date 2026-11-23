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

#include "ctp_td_shim.h"

#include <cstring>
#include <string>

#include "ThostFtdcTraderApi.h"

namespace {

template <size_t N>
void copy_str(char (&dst)[N], const char* src) {
    if (src == nullptr) {
        dst[0] = '\0';
        return;
    }
    std::strncpy(dst, src, N - 1);
    dst[N - 1] = '\0';
}

class TdSpi final : public CThostFtdcTraderSpi {
public:
    TdSpi(NautilusCtpTdEventCallback callback, void* user_data)
        : callback_(callback), user_data_(user_data) {}

    void OnFrontConnected() override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_FRONT_CONNECTED;
        emit(event);
    }

    void OnFrontDisconnected(int reason) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_FRONT_DISCONNECTED;
        event.reason = reason;
        emit(event);
    }

    void OnHeartBeatWarning(int time_lapse) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_HEARTBEAT_WARNING;
        event.reason = time_lapse;
        emit(event);
    }

    void OnRspAuthenticate(CThostFtdcRspAuthenticateField* /*auth*/,
                           CThostFtdcRspInfoField* info,
                           int request_id,
                           bool is_last) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_AUTHENTICATE;
        set_response(event, info, request_id, is_last);
        emit(event);
    }

    void OnRspUserLogin(CThostFtdcRspUserLoginField* login,
                        CThostFtdcRspInfoField* info,
                        int request_id,
                        bool is_last) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_LOGIN;
        set_response(event, info, request_id, is_last);
        if (login != nullptr) {
            copy_str(event.trading_day, login->TradingDay);
            copy_str(event.broker_id, login->BrokerID);
            copy_str(event.investor_id, login->UserID);
            copy_str(event.max_order_ref, login->MaxOrderRef);
            event.front_id = login->FrontID;
            event.session_id = login->SessionID;
        }
        emit(event);
    }

    void OnRspUserLogout(CThostFtdcUserLogoutField* /*logout*/,
                         CThostFtdcRspInfoField* info,
                         int request_id,
                         bool is_last) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_LOGOUT;
        set_response(event, info, request_id, is_last);
        emit(event);
    }

    void OnRspSettlementInfoConfirm(CThostFtdcSettlementInfoConfirmField* /*confirm*/,
                                    CThostFtdcRspInfoField* info,
                                    int request_id,
                                    bool is_last) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_SETTLEMENT_CONFIRM;
        set_response(event, info, request_id, is_last);
        emit(event);
    }

    void OnRspOrderInsert(CThostFtdcInputOrderField* order,
                          CThostFtdcRspInfoField* info,
                          int request_id,
                          bool is_last) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_ORDER_INSERT_RSP;
        set_response(event, info, request_id, is_last);
        if (order != nullptr) {
            set_input_order(event, order);
        }
        emit(event);
    }

    void OnErrRtnOrderInsert(CThostFtdcInputOrderField* order,
                             CThostFtdcRspInfoField* info) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_ORDER_INSERT_ERR;
        set_response(event, info, 0, true);
        if (order != nullptr) {
            set_input_order(event, order);
        }
        emit(event);
    }

    void OnRspOrderAction(CThostFtdcInputOrderActionField* action,
                          CThostFtdcRspInfoField* info,
                          int request_id,
                          bool is_last) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_ORDER_ACTION_RSP;
        set_response(event, info, request_id, is_last);
        if (action != nullptr) {
            set_action_fields(event, action->OrderRef, action->InstrumentID, action->ExchangeID,
                              action->OrderSysID, action->FrontID, action->SessionID);
        }
        emit(event);
    }

    void OnErrRtnOrderAction(CThostFtdcOrderActionField* action,
                             CThostFtdcRspInfoField* info) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_ORDER_ACTION_ERR;
        set_response(event, info, 0, true);
        if (action != nullptr) {
            set_action_fields(event, action->OrderRef, action->InstrumentID, action->ExchangeID,
                              action->OrderSysID, action->FrontID, action->SessionID);
        }
        emit(event);
    }

    void OnRtnOrder(CThostFtdcOrderField* order) override {
        if (order == nullptr) {
            return;
        }
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_ORDER;
        set_order_fields(event, order);
        emit(event);
    }

    void OnRtnTrade(CThostFtdcTradeField* trade) override {
        if (trade == nullptr) {
            return;
        }
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_TRADE;
        set_trade_fields(event, trade);
        emit(event);
    }

    void OnRspQryOrder(CThostFtdcOrderField* order,
                       CThostFtdcRspInfoField* info,
                       int request_id,
                       bool is_last) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_QRY_ORDER;
        set_response(event, info, request_id, is_last);
        if (order != nullptr) {
            event.has_record = 1;
            set_order_fields(event, order);
        }
        emit(event);
    }

    void OnRspQryTrade(CThostFtdcTradeField* trade,
                       CThostFtdcRspInfoField* info,
                       int request_id,
                       bool is_last) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_QRY_TRADE;
        set_response(event, info, request_id, is_last);
        if (trade != nullptr) {
            event.has_record = 1;
            set_trade_fields(event, trade);
        }
        emit(event);
    }

    void OnRspQryInvestorPosition(CThostFtdcInvestorPositionField* position,
                                  CThostFtdcRspInfoField* info,
                                  int request_id,
                                  bool is_last) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_QRY_POSITION;
        set_response(event, info, request_id, is_last);
        if (position != nullptr) {
            event.has_record = 1;
            copy_str(event.instrument_id, position->InstrumentID);
            copy_str(event.exchange_id, position->ExchangeID);
            event.pos_direction = position->PosiDirection;
            event.position = position->Position;
            event.yd_position = position->YdPosition;
            event.position_cost = position->PositionCost;
        }
        emit(event);
    }

    void OnRspQryTradingAccount(CThostFtdcTradingAccountField* account,
                                CThostFtdcRspInfoField* info,
                                int request_id,
                                bool is_last) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_QRY_ACCOUNT;
        set_response(event, info, request_id, is_last);
        if (account != nullptr) {
            event.has_record = 1;
            copy_str(event.account_id, account->AccountID);
            copy_str(event.currency_id, account->CurrencyID);
            copy_str(event.trading_day, account->TradingDay);
            event.balance = account->Balance;
            event.available = account->Available;
            event.curr_margin = account->CurrMargin;
            event.commission = account->Commission;
            event.position_profit = account->PositionProfit;
            event.close_profit = account->CloseProfit;
            event.deposit = account->Deposit;
            event.withdraw = account->Withdraw;
        }
        emit(event);
    }

    void OnRspError(CThostFtdcRspInfoField* info, int request_id, bool is_last) override {
        NautilusCtpTdEvent event{};
        event.kind = NAUTILUS_CTP_TD_EVENT_RSP_ERROR;
        set_response(event, info, request_id, is_last);
        emit(event);
    }

private:
    static void set_response(NautilusCtpTdEvent& event,
                             CThostFtdcRspInfoField* info,
                             int request_id,
                             bool is_last) {
        event.request_id = request_id;
        event.is_last = is_last ? 1 : 0;
        if (info != nullptr) {
            event.error_id = info->ErrorID;
            event.ok = info->ErrorID == 0 ? 1 : 0;
            copy_str(event.error_msg, info->ErrorMsg);
        } else {
            event.ok = 1;
        }
    }

    static void set_input_order(NautilusCtpTdEvent& event, CThostFtdcInputOrderField* order) {
        event.volume_original = order->VolumeTotalOriginal;
        copy_str(event.broker_id, order->BrokerID);
        copy_str(event.investor_id, order->InvestorID);
        copy_str(event.instrument_id, order->InstrumentID);
        copy_str(event.exchange_id, order->ExchangeID);
        copy_str(event.order_ref, order->OrderRef);
        event.direction = order->Direction;
    }

    static void set_action_fields(NautilusCtpTdEvent& event,
                                  const char* order_ref,
                                  const char* instrument_id,
                                  const char* exchange_id,
                                  const char* order_sys_id,
                                  int front_id,
                                  int session_id) {
        copy_str(event.order_ref, order_ref);
        copy_str(event.instrument_id, instrument_id);
        copy_str(event.exchange_id, exchange_id);
        copy_str(event.order_sys_id, order_sys_id);
        event.front_id = front_id;
        event.session_id = session_id;
    }

    static void set_order_fields(NautilusCtpTdEvent& event, CThostFtdcOrderField* order) {
        copy_str(event.trading_day, order->TradingDay);
        copy_str(event.broker_id, order->BrokerID);
        copy_str(event.investor_id, order->InvestorID);
        copy_str(event.instrument_id, order->InstrumentID);
        copy_str(event.exchange_id, order->ExchangeID);
        copy_str(event.order_ref, order->OrderRef);
        copy_str(event.order_sys_id, order->OrderSysID);
        copy_str(event.status_msg, order->StatusMsg);
        copy_str(event.insert_date, order->InsertDate);
        copy_str(event.insert_time, order->InsertTime);
        copy_str(event.update_time, order->UpdateTime);
        event.front_id = order->FrontID;
        event.session_id = order->SessionID;
        event.order_status = order->OrderStatus;
        event.order_submit_status = order->OrderSubmitStatus;
        event.direction = order->Direction;
        event.volume_total = order->VolumeTotal;
        event.volume_original = order->VolumeTotalOriginal;
        event.volume_traded = order->VolumeTraded;
        event.limit_price = order->LimitPrice;
    }

    static void set_trade_fields(NautilusCtpTdEvent& event, CThostFtdcTradeField* trade) {
        copy_str(event.trading_day, trade->TradingDay);
        copy_str(event.broker_id, trade->BrokerID);
        copy_str(event.investor_id, trade->InvestorID);
        copy_str(event.instrument_id, trade->InstrumentID);
        copy_str(event.exchange_id, trade->ExchangeID);
        copy_str(event.order_ref, trade->OrderRef);
        copy_str(event.order_sys_id, trade->OrderSysID);
        copy_str(event.trade_id, trade->TradeID);
        copy_str(event.trade_date, trade->TradeDate);
        copy_str(event.trade_time, trade->TradeTime);
        event.direction = trade->Direction;
        event.offset_flag = trade->OffsetFlag;
        event.trade_price = trade->Price;
        event.trade_volume = trade->Volume;
    }

    void emit(const NautilusCtpTdEvent& event) const {
        if (callback_ != nullptr) {
            callback_(user_data_, &event);
        }
    }

    NautilusCtpTdEventCallback callback_;
    void* user_data_;
};

}  // namespace

struct NautilusCtpTdApi {
    CThostFtdcTraderApi* api;
    TdSpi* spi;
};

extern "C" {

const char* nautilus_ctp_td_api_version(void) {
    return CThostFtdcTraderApi::GetApiVersion();
}

NautilusCtpTdApi* nautilus_ctp_td_create(
    const char* flow_path,
    int32_t production_mode,
    NautilusCtpTdEventCallback callback,
    void* user_data) {
    if (callback == nullptr) {
        return nullptr;
    }
    CThostFtdcTraderApi* api = CThostFtdcTraderApi::CreateFtdcTraderApi(
        flow_path == nullptr ? "" : flow_path,
        production_mode != 0);
    if (api == nullptr) {
        return nullptr;
    }
    TdSpi* spi = new TdSpi(callback, user_data);
    api->RegisterSpi(spi);
    return new NautilusCtpTdApi{api, spi};
}

void nautilus_ctp_td_release(NautilusCtpTdApi* handle) {
    if (handle == nullptr) {
        return;
    }
    handle->api->Release();
    delete handle->spi;
    delete handle;
}

int32_t nautilus_ctp_td_register_front(NautilusCtpTdApi* handle, const char* address) {
    if (handle == nullptr || address == nullptr) {
        return -1;
    }
    std::string buffer(address);
    handle->api->RegisterFront(&buffer[0]);
    return 0;
}

static THOST_TE_RESUME_TYPE to_resume(int32_t resume_type) {
    switch (resume_type) {
        case 0:
            return THOST_TERT_RESTART;
        case 1:
            return THOST_TERT_QUICK;
        case 2:
            return THOST_TERT_RESUME;
        default:
            return THOST_TERT_NONE;
    }
}

int32_t nautilus_ctp_td_subscribe_private_topic(NautilusCtpTdApi* handle, int32_t resume_type) {
    if (handle == nullptr) {
        return -1;
    }
    handle->api->SubscribePrivateTopic(to_resume(resume_type));
    return 0;
}

int32_t nautilus_ctp_td_subscribe_public_topic(NautilusCtpTdApi* handle, int32_t resume_type) {
    if (handle == nullptr) {
        return -1;
    }
    handle->api->SubscribePublicTopic(to_resume(resume_type));
    return 0;
}

int32_t nautilus_ctp_td_init(NautilusCtpTdApi* handle) {
    if (handle == nullptr) {
        return -1;
    }
    handle->api->Init();
    return 0;
}

int32_t nautilus_ctp_td_authenticate(
    NautilusCtpTdApi* handle,
    const char* broker_id,
    const char* user_id,
    const char* app_id,
    const char* auth_code,
    const char* product_info,
    int32_t request_id) {
    if (handle == nullptr) {
        return -1;
    }
    CThostFtdcReqAuthenticateField request{};
    copy_str(request.BrokerID, broker_id);
    copy_str(request.UserID, user_id);
    copy_str(request.AppID, app_id);
    copy_str(request.AuthCode, auth_code);
    copy_str(request.UserProductInfo, product_info);
    return handle->api->ReqAuthenticate(&request, request_id);
}

int32_t nautilus_ctp_td_login(
    NautilusCtpTdApi* handle,
    const char* broker_id,
    const char* user_id,
    const char* password,
    const char* product_info,
    int32_t request_id) {
    if (handle == nullptr) {
        return -1;
    }
    CThostFtdcReqUserLoginField request{};
    copy_str(request.BrokerID, broker_id);
    copy_str(request.UserID, user_id);
    copy_str(request.Password, password);
    copy_str(request.UserProductInfo, product_info);
    return handle->api->ReqUserLogin(&request, request_id);
}

int32_t nautilus_ctp_td_logout(
    NautilusCtpTdApi* handle,
    const char* broker_id,
    const char* user_id,
    int32_t request_id) {
    if (handle == nullptr) {
        return -1;
    }
    CThostFtdcUserLogoutField request{};
    copy_str(request.BrokerID, broker_id);
    copy_str(request.UserID, user_id);
    return handle->api->ReqUserLogout(&request, request_id);
}

int32_t nautilus_ctp_td_settlement_confirm(
    NautilusCtpTdApi* handle,
    const char* broker_id,
    const char* investor_id,
    int32_t request_id) {
    if (handle == nullptr) {
        return -1;
    }
    CThostFtdcSettlementInfoConfirmField request{};
    copy_str(request.BrokerID, broker_id);
    copy_str(request.InvestorID, investor_id);
    return handle->api->ReqSettlementInfoConfirm(&request, request_id);
}

int32_t nautilus_ctp_td_insert_order(
    NautilusCtpTdApi* handle,
    const NautilusCtpOrderInsert* order,
    int32_t request_id) {
    if (handle == nullptr || order == nullptr) {
        return -1;
    }
    CThostFtdcInputOrderField request{};
    copy_str(request.BrokerID, order->broker_id);
    copy_str(request.InvestorID, order->investor_id);
    copy_str(request.InstrumentID, order->instrument_id);
    copy_str(request.ExchangeID, order->exchange_id);
    copy_str(request.OrderRef, order->order_ref);
    copy_str(request.UserID, order->investor_id);
    request.Direction = order->direction;
    request.OrderPriceType = order->price_type;
    request.LimitPrice = order->limit_price;
    request.VolumeTotalOriginal = order->volume;
    request.TimeCondition = order->time_condition;
    request.VolumeCondition = order->volume_condition;
    request.CombOffsetFlag[0] = order->offset_flag;
    request.CombOffsetFlag[1] = '\0';
    request.CombHedgeFlag[0] = '1';  // Speculation.
    request.CombHedgeFlag[1] = '\0';
    request.MinVolume = 1;
    request.ContingentCondition = '1';  // Immediately.
    request.ForceCloseReason = '0';     // Not a forced close.
    request.IsAutoSuspend = 0;
    request.UserForceClose = 0;
    request.StopPrice = 0.0;
    request.RequestID = request_id;
    return handle->api->ReqOrderInsert(&request, request_id);
}

int32_t nautilus_ctp_td_cancel_order(
    NautilusCtpTdApi* handle,
    const NautilusCtpOrderCancel* cancel,
    int32_t request_id) {
    if (handle == nullptr || cancel == nullptr) {
        return -1;
    }
    CThostFtdcInputOrderActionField request{};
    copy_str(request.BrokerID, cancel->broker_id);
    copy_str(request.InvestorID, cancel->investor_id);
    copy_str(request.InstrumentID, cancel->instrument_id);
    copy_str(request.ExchangeID, cancel->exchange_id);
    copy_str(request.OrderRef, cancel->order_ref);
    copy_str(request.OrderSysID, cancel->order_sys_id);
    copy_str(request.UserID, cancel->investor_id);
    request.FrontID = cancel->front_id;
    request.SessionID = cancel->session_id;
    request.ActionFlag = THOST_FTDC_AF_Delete;
    request.RequestID = request_id;
    return handle->api->ReqOrderAction(&request, request_id);
}

int32_t nautilus_ctp_td_query_orders(
    NautilusCtpTdApi* handle,
    const char* broker_id,
    const char* investor_id,
    int32_t request_id) {
    if (handle == nullptr) {
        return -1;
    }
    CThostFtdcQryOrderField request{};
    copy_str(request.BrokerID, broker_id);
    copy_str(request.InvestorID, investor_id);
    return handle->api->ReqQryOrder(&request, request_id);
}

int32_t nautilus_ctp_td_query_trades(
    NautilusCtpTdApi* handle,
    const char* broker_id,
    const char* investor_id,
    int32_t request_id) {
    if (handle == nullptr) {
        return -1;
    }
    CThostFtdcQryTradeField request{};
    copy_str(request.BrokerID, broker_id);
    copy_str(request.InvestorID, investor_id);
    return handle->api->ReqQryTrade(&request, request_id);
}

int32_t nautilus_ctp_td_query_positions(
    NautilusCtpTdApi* handle,
    const char* broker_id,
    const char* investor_id,
    int32_t request_id) {
    if (handle == nullptr) {
        return -1;
    }
    CThostFtdcQryInvestorPositionField request{};
    copy_str(request.BrokerID, broker_id);
    copy_str(request.InvestorID, investor_id);
    return handle->api->ReqQryInvestorPosition(&request, request_id);
}

int32_t nautilus_ctp_td_query_account(
    NautilusCtpTdApi* handle,
    const char* broker_id,
    const char* investor_id,
    int32_t request_id) {
    if (handle == nullptr) {
        return -1;
    }
    CThostFtdcQryTradingAccountField request{};
    copy_str(request.BrokerID, broker_id);
    copy_str(request.InvestorID, investor_id);
    return handle->api->ReqQryTradingAccount(&request, request_id);
}

}  // extern "C"
