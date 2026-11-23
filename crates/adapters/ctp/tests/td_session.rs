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

//! Trading session behavior: login flow, order lifecycle, fills, cancels, and queries.

use std::sync::{Arc, Mutex};

use nautilus_core::string::secret::SecretString;
use nautilus_ctp::{
    md::RspError,
    td::{
        AccountRecord, LoginInfo, OrderCancelRequest, OrderInsertRequest, OrderRecord,
        PositionRecord, QueryResponse, ResumeType, TdBackend, TdEvent, TradeRecord, direction,
        offset, position_direction,
        session::{SessionStatus, TdCredentials, TdOutput, TdSession},
        status, submit_status,
    },
};
use nautilus_model::identifiers::ClientOrderId;
use rstest::rstest;

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Register(String),
    Topics(ResumeType, ResumeType),
    Init,
    Authenticate(String),
    Login(String),
    Logout,
    ConfirmSettlement,
    Insert(OrderInsertRequest, i32),
    Cancel(OrderCancelRequest, i32),
    QueryOrders(i32),
    QueryTrades(i32),
    QueryPositions(i32),
    QueryAccount(i32),
}

#[derive(Debug, Clone, Default)]
struct RecordingBackend {
    calls: Arc<Mutex<Vec<Call>>>,
    reject_insert: bool,
}

impl RecordingBackend {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }
}

impl TdBackend for RecordingBackend {
    fn register_front(&mut self, address: &str) -> anyhow::Result<()> {
        self.record(Call::Register(address.to_string()));
        Ok(())
    }

    fn subscribe_topics(&mut self, private: ResumeType, public: ResumeType) -> anyhow::Result<()> {
        self.record(Call::Topics(private, public));
        Ok(())
    }

    fn init(&mut self) -> anyhow::Result<()> {
        self.record(Call::Init);
        Ok(())
    }

    fn authenticate(
        &mut self,
        _broker_id: &str,
        _user_id: &str,
        app_id: &str,
        _auth_code: &str,
        _product_info: &str,
        _request_id: i32,
    ) -> anyhow::Result<()> {
        self.record(Call::Authenticate(app_id.to_string()));
        Ok(())
    }

    fn login(
        &mut self,
        broker_id: &str,
        _user_id: &str,
        _password: &str,
        _product_info: &str,
        _request_id: i32,
    ) -> anyhow::Result<()> {
        self.record(Call::Login(broker_id.to_string()));
        Ok(())
    }

    fn logout(&mut self, _broker_id: &str, _user_id: &str, _request_id: i32) -> anyhow::Result<()> {
        self.record(Call::Logout);
        Ok(())
    }

    fn confirm_settlement(
        &mut self,
        _broker_id: &str,
        _investor_id: &str,
        _request_id: i32,
    ) -> anyhow::Result<()> {
        self.record(Call::ConfirmSettlement);
        Ok(())
    }

    fn insert_order(&mut self, order: &OrderInsertRequest, request_id: i32) -> anyhow::Result<()> {
        self.record(Call::Insert(order.clone(), request_id));
        if self.reject_insert {
            anyhow::bail!("insert rejected synchronously");
        }
        Ok(())
    }

    fn cancel_order(&mut self, cancel: &OrderCancelRequest, request_id: i32) -> anyhow::Result<()> {
        self.record(Call::Cancel(cancel.clone(), request_id));
        Ok(())
    }

    fn query_orders(
        &mut self,
        _broker_id: &str,
        _investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        self.record(Call::QueryOrders(request_id));
        Ok(())
    }

    fn query_trades(
        &mut self,
        _broker_id: &str,
        _investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        self.record(Call::QueryTrades(request_id));
        Ok(())
    }

    fn query_positions(
        &mut self,
        _broker_id: &str,
        _investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        self.record(Call::QueryPositions(request_id));
        Ok(())
    }

    fn query_account(
        &mut self,
        _broker_id: &str,
        _investor_id: &str,
        request_id: i32,
    ) -> anyhow::Result<()> {
        self.record(Call::QueryAccount(request_id));
        Ok(())
    }
}

fn credentials(app_id: Option<&str>) -> TdCredentials {
    TdCredentials {
        broker_id: "9999".to_string(),
        investor_id: "123456".to_string(),
        password: SecretString::from("secret"),
        app_id: app_id.map(str::to_string),
        auth_code: app_id.map(|_| SecretString::from("code")),
        product_info: String::new(),
    }
}

fn ready_session(app_id: Option<&str>, backend: RecordingBackend) -> TdSession<RecordingBackend> {
    let mut session = TdSession::new(backend, credentials(app_id));
    session.start("tcp://127.0.0.1:10130").unwrap();
    session.handle_event(TdEvent::FrontConnected);
    if app_id.is_some() {
        session.handle_event(TdEvent::Authenticate {
            request_id: 1,
            result: Ok(()),
        });
    }
    session.handle_event(TdEvent::Login {
        request_id: 2,
        result: Ok(LoginInfo {
            trading_day: "20251122".to_string(),
            front_id: 7,
            session_id: 42,
            max_order_ref: "100".to_string(),
        }),
    });
    session.handle_event(TdEvent::SettlementConfirm {
        request_id: 3,
        result: Ok(()),
    });
    session
}

fn limit_order(volume: i32) -> OrderInsertRequest {
    OrderInsertRequest {
        broker_id: String::new(),
        investor_id: String::new(),
        instrument_id: "rb2510".to_string(),
        exchange_id: "SHFE".to_string(),
        order_ref: String::new(),
        direction: direction::BUY,
        price_type: '2',
        limit_price: 3100.0,
        volume,
        time_condition: '3',
        volume_condition: '1',
        offset_flag: offset::OPEN,
    }
}

fn order_return(order_ref: &str, order_status: char, submit: char, traded: i32) -> OrderRecord {
    OrderRecord {
        front_id: 7,
        session_id: 42,
        order_ref: order_ref.to_string(),
        exchange_id: "SHFE".to_string(),
        order_sys_id: "5001".to_string(),
        instrument_id: "rb2510".to_string(),
        direction: direction::BUY,
        order_status,
        order_submit_status: submit,
        volume_original: 10,
        volume_traded: traded,
        volume_remaining: 10 - traded,
        limit_price: 3100.0,
        status_msg: String::new(),
        insert_date: "20251122".to_string(),
        insert_time: "09:30:00".to_string(),
        update_time: "09:30:01".to_string(),
        trading_day: "20251122".to_string(),
    }
}

fn trade(trade_id: &str, order_ref: &str, volume: i32) -> TradeRecord {
    TradeRecord {
        trade_id: trade_id.to_string(),
        order_ref: order_ref.to_string(),
        order_sys_id: "5001".to_string(),
        exchange_id: "SHFE".to_string(),
        instrument_id: "rb2510".to_string(),
        direction: direction::BUY,
        offset_flag: offset::OPEN,
        price: 3100.2,
        volume,
        trade_date: "20251122".to_string(),
        trade_time: "09:30:02".to_string(),
        trading_day: "20251122".to_string(),
    }
}

fn client_id(value: &str) -> ClientOrderId {
    ClientOrderId::from(value)
}

#[rstest]
fn test_login_without_authentication_reaches_ready() {
    let backend = RecordingBackend::default();
    let mut session = TdSession::new(backend.clone(), credentials(None));
    let status = session.status();
    session.start("tcp://127.0.0.1:10130").unwrap();
    assert_eq!(
        backend.calls(),
        vec![
            Call::Register("tcp://127.0.0.1:10130".to_string()),
            Call::Topics(ResumeType::Restart, ResumeType::Quick),
            Call::Init,
        ]
    );

    session.handle_event(TdEvent::FrontConnected);
    assert!(backend.calls().contains(&Call::Login("9999".to_string())));
    session.handle_event(TdEvent::Login {
        request_id: 1,
        result: Ok(LoginInfo {
            trading_day: "20251122".to_string(),
            front_id: 7,
            session_id: 42,
            max_order_ref: "0".to_string(),
        }),
    });
    assert!(backend.calls().contains(&Call::ConfirmSettlement));
    assert!(!session.is_ready());

    session.handle_event(TdEvent::SettlementConfirm {
        request_id: 2,
        result: Ok(()),
    });
    assert!(session.is_ready());
    assert_eq!(*status.borrow(), SessionStatus::Ready);
}

#[rstest]
fn test_login_with_authentication_runs_authenticate_first() {
    let backend = RecordingBackend::default();
    let mut session = TdSession::new(backend.clone(), credentials(Some("simnow_client")));
    session.start("tcp://127.0.0.1:10130").unwrap();
    session.handle_event(TdEvent::FrontConnected);

    assert!(
        backend
            .calls()
            .contains(&Call::Authenticate("simnow_client".to_string()))
    );
    assert!(!backend.calls().iter().any(|c| matches!(c, Call::Login(_))));

    session.handle_event(TdEvent::Authenticate {
        request_id: 1,
        result: Ok(()),
    });
    assert!(backend.calls().iter().any(|c| matches!(c, Call::Login(_))));
}

#[rstest]
fn test_authentication_failure_sets_failed_status() {
    let backend = RecordingBackend::default();
    let mut session = TdSession::new(backend, credentials(Some("app")));
    let status = session.status();
    session.start("tcp://127.0.0.1:10130").unwrap();
    session.handle_event(TdEvent::FrontConnected);
    session.handle_event(TdEvent::Authenticate {
        request_id: 1,
        result: Err(RspError {
            error_id: 3,
            message: "bad auth code".to_string(),
        }),
    });

    assert!(
        matches!(*status.borrow(), SessionStatus::Failed(ref m) if m.contains("bad auth code"))
    );
}

#[rstest]
fn test_submit_before_ready_is_rejected() {
    let mut session = TdSession::new(RecordingBackend::default(), credentials(None));
    let err = session
        .submit(client_id("O-1"), limit_order(10))
        .unwrap_err();
    assert!(err.to_string().contains("not ready"));
}

#[rstest]
fn test_submit_allocates_order_ref_above_front_maximum() {
    let backend = RecordingBackend::default();
    let mut session = ready_session(None, backend.clone());
    session.submit(client_id("O-1"), limit_order(10)).unwrap();

    let inserted = backend
        .calls()
        .into_iter()
        .find_map(|c| match c {
            Call::Insert(order, _) => Some(order),
            _ => None,
        })
        .unwrap();
    assert_eq!(inserted.order_ref, "101");
    assert_eq!(inserted.broker_id, "9999");
    assert_eq!(inserted.investor_id, "123456");
}

#[rstest]
fn test_duplicate_client_order_id_is_rejected() {
    let mut session = ready_session(None, RecordingBackend::default());
    session.submit(client_id("O-1"), limit_order(10)).unwrap();
    let err = session
        .submit(client_id("O-1"), limit_order(10))
        .unwrap_err();
    assert!(err.to_string().contains("already in use"));
}

#[rstest]
fn test_synchronous_insert_failure_leaves_no_order_behind() {
    let backend = RecordingBackend {
        reject_insert: true,
        ..Default::default()
    };
    let mut session = ready_session(None, backend);
    assert!(session.submit(client_id("O-1"), limit_order(10)).is_err());

    // The same client order ID can be used again because nothing was tracked.
    let mut retry = ready_session(None, RecordingBackend::default());
    retry.submit(client_id("O-1"), limit_order(10)).unwrap();
}

#[rstest]
fn test_order_accepted_then_filled_in_two_fills() {
    let mut session = ready_session(None, RecordingBackend::default());
    session.submit(client_id("O-1"), limit_order(10)).unwrap();

    let out = session.handle_event(TdEvent::Order(Box::new(order_return(
        "101",
        status::NO_TRADE_QUEUEING,
        submit_status::ACCEPTED,
        0,
    ))));
    assert_eq!(
        out,
        vec![TdOutput::Accepted {
            client_order_id: client_id("O-1"),
            venue_order_id: "5001".to_string(),
        }]
    );

    let out = session.handle_event(TdEvent::Trade(Box::new(trade("T1", "101", 4))));
    assert!(matches!(out.as_slice(), [TdOutput::Filled { trade, .. }] if trade.volume == 4));

    let out = session.handle_event(TdEvent::Trade(Box::new(trade("T2", "101", 6))));
    assert!(matches!(out.as_slice(), [TdOutput::Filled { trade, .. }] if trade.volume == 6));

    // A later order return for a terminal order produces nothing further.
    let out = session.handle_event(TdEvent::Order(Box::new(order_return(
        "101",
        status::ALL_TRADED,
        submit_status::ACCEPTED,
        10,
    ))));
    assert!(out.is_empty());
}

#[rstest]
fn test_fill_before_acceptance_emits_accept_first() {
    let mut session = ready_session(None, RecordingBackend::default());
    session.submit(client_id("O-1"), limit_order(10)).unwrap();

    let out = session.handle_event(TdEvent::Trade(Box::new(trade("T1", "101", 10))));
    assert!(matches!(
        out.as_slice(),
        [TdOutput::Accepted { .. }, TdOutput::Filled { .. }]
    ));
}

#[rstest]
fn test_duplicate_trade_is_ignored() {
    let mut session = ready_session(None, RecordingBackend::default());
    session.submit(client_id("O-1"), limit_order(10)).unwrap();
    session.handle_event(TdEvent::Order(Box::new(order_return(
        "101",
        status::NO_TRADE_QUEUEING,
        submit_status::ACCEPTED,
        0,
    ))));
    session.handle_event(TdEvent::Trade(Box::new(trade("T1", "101", 4))));

    // The session replays the same return after a reconnect.
    let out = session.handle_event(TdEvent::Trade(Box::new(trade("T1", "101", 4))));
    assert!(out.is_empty());
}

#[rstest]
fn test_insert_rejection_from_front_reports_rejected() {
    let mut session = ready_session(None, RecordingBackend::default());
    session.submit(client_id("O-1"), limit_order(10)).unwrap();

    let out = session.handle_event(TdEvent::OrderInsertRejected {
        order_ref: "101".to_string(),
        error: RspError {
            error_id: 22,
            message: "price out of range".to_string(),
        },
    });
    assert_eq!(
        out,
        vec![TdOutput::Rejected {
            client_order_id: client_id("O-1"),
            reason: "[22] price out of range".to_string(),
        }]
    );
}

#[rstest]
fn test_exchange_rejection_without_fills_reports_rejected() {
    let mut session = ready_session(None, RecordingBackend::default());
    session.submit(client_id("O-1"), limit_order(10)).unwrap();

    let mut rec = order_return(
        "101",
        status::NO_TRADE_NOT_QUEUEING,
        submit_status::ACCEPTED,
        0,
    );
    rec.status_msg = "fill or kill failed".to_string();
    let out = session.handle_event(TdEvent::Order(Box::new(rec)));
    assert_eq!(
        out,
        vec![TdOutput::Rejected {
            client_order_id: client_id("O-1"),
            reason: "fill or kill failed".to_string(),
        }]
    );
}

#[rstest]
fn test_cancel_then_canceled_return() {
    let backend = RecordingBackend::default();
    let mut session = ready_session(None, backend.clone());
    session.submit(client_id("O-1"), limit_order(10)).unwrap();
    session.handle_event(TdEvent::Order(Box::new(order_return(
        "101",
        status::NO_TRADE_QUEUEING,
        submit_status::ACCEPTED,
        0,
    ))));

    session.cancel(client_id("O-1")).unwrap();
    let cancel = backend
        .calls()
        .into_iter()
        .find_map(|c| match c {
            Call::Cancel(cancel, _) => Some(cancel),
            _ => None,
        })
        .unwrap();
    assert_eq!(cancel.front_id, 7);
    assert_eq!(cancel.session_id, 42);
    assert_eq!(cancel.order_ref, "101");
    assert_eq!(cancel.order_sys_id, "5001");

    // A second cancel while one is pending is refused.
    assert!(session.cancel(client_id("O-1")).is_err());

    let out = session.handle_event(TdEvent::Order(Box::new(order_return(
        "101",
        status::CANCELED,
        submit_status::ACCEPTED,
        0,
    ))));
    assert_eq!(
        out,
        vec![TdOutput::Canceled {
            client_order_id: client_id("O-1"),
            venue_order_id: Some("5001".to_string()),
        }]
    );
}

#[rstest]
fn test_cancel_rejection_clears_pending_flag() {
    let mut session = ready_session(None, RecordingBackend::default());
    session.submit(client_id("O-1"), limit_order(10)).unwrap();
    session.handle_event(TdEvent::Order(Box::new(order_return(
        "101",
        status::NO_TRADE_QUEUEING,
        submit_status::ACCEPTED,
        0,
    ))));
    session.cancel(client_id("O-1")).unwrap();

    let out = session.handle_event(TdEvent::OrderActionRejected {
        order_ref: "101".to_string(),
        order_sys_id: "5001".to_string(),
        error: RspError {
            error_id: 31,
            message: "already filled".to_string(),
        },
    });
    assert!(matches!(out.as_slice(), [TdOutput::CancelRejected { .. }]));

    // The order is still live, so cancellation can be retried.
    session.cancel(client_id("O-1")).unwrap();
}

#[rstest]
fn test_cancel_unknown_order_is_rejected() {
    let mut session = ready_session(None, RecordingBackend::default());
    assert!(session.cancel(client_id("nope")).is_err());
}

#[rstest]
fn test_foreign_order_return_is_reported_as_external() {
    let mut session = ready_session(None, RecordingBackend::default());
    let out = session.handle_event(TdEvent::Order(Box::new(order_return(
        "999",
        status::NO_TRADE_QUEUEING,
        submit_status::ACCEPTED,
        0,
    ))));
    assert!(matches!(out.as_slice(), [TdOutput::ExternalOrder(rec)] if rec.order_ref == "999"));
}

fn last_request_id(backend: &RecordingBackend) -> i32 {
    backend
        .calls()
        .iter()
        .rev()
        .find_map(|c| match c {
            Call::QueryOrders(id)
            | Call::QueryTrades(id)
            | Call::QueryPositions(id)
            | Call::QueryAccount(id) => Some(*id),
            _ => None,
        })
        .expect("a query was sent")
}

#[rstest]
fn test_order_query_completes_on_last_response() {
    let backend = RecordingBackend::default();
    let mut session = ready_session(None, backend.clone());
    let mut receiver = session.query_orders().unwrap();
    let request_id = last_request_id(&backend);

    session.handle_event(TdEvent::QryOrder(QueryResponse {
        request_id,
        is_last: false,
        result: Ok(Some(order_return(
            "1",
            status::NO_TRADE_QUEUEING,
            submit_status::ACCEPTED,
            0,
        ))),
    }));
    assert!(receiver.try_recv().is_err());

    session.handle_event(TdEvent::QryOrder(QueryResponse {
        request_id,
        is_last: true,
        result: Ok(None),
    }));
    let rows = receiver.try_recv().unwrap().unwrap();
    assert_eq!(rows.len(), 1);
}

#[rstest]
fn test_query_error_fails_the_receiver() {
    let backend = RecordingBackend::default();
    let mut session = ready_session(None, backend.clone());
    let mut receiver = session.query_trades().unwrap();
    let request_id = last_request_id(&backend);

    session.handle_event(TdEvent::QryTrade(QueryResponse {
        request_id,
        is_last: true,
        result: Err(RspError {
            error_id: 9,
            message: "flow control".to_string(),
        }),
    }));
    let err = receiver.try_recv().unwrap().unwrap_err();
    assert!(err.to_string().contains("flow control"));
}

#[rstest]
fn test_position_and_account_queries_complete() {
    let backend = RecordingBackend::default();
    let mut session = ready_session(None, backend.clone());
    let mut positions = session.query_positions().unwrap();
    let mut account = session.query_account().unwrap();

    let request_id = backend
        .calls()
        .iter()
        .find_map(|c| match c {
            Call::QueryPositions(id) => Some(*id),
            _ => None,
        })
        .unwrap();
    session.handle_event(TdEvent::QryPosition(QueryResponse {
        request_id,
        is_last: true,
        result: Ok(Some(PositionRecord {
            exchange_id: "SHFE".to_string(),
            instrument_id: "rb2510".to_string(),
            direction: position_direction::LONG,
            position: 3,
            yd_position: 1,
            position_cost: 9300.0,
        })),
    }));
    assert_eq!(positions.try_recv().unwrap().unwrap().len(), 1);

    let request_id = backend
        .calls()
        .iter()
        .find_map(|c| match c {
            Call::QueryAccount(id) => Some(*id),
            _ => None,
        })
        .unwrap();
    session.handle_event(TdEvent::QryAccount(QueryResponse {
        request_id,
        is_last: true,
        result: Ok(Some(AccountRecord {
            account_id: "123456".to_string(),
            currency_id: "CNY".to_string(),
            trading_day: "20251122".to_string(),
            balance: 100_000.0,
            available: 90_000.0,
            curr_margin: 5_000.0,
            commission: 12.0,
            position_profit: 0.0,
            close_profit: 0.0,
            deposit: 100_000.0,
            withdraw: 0.0,
        })),
    }));
    assert_eq!(
        account.try_recv().unwrap().unwrap().unwrap().balance,
        100_000.0
    );
}

#[rstest]
fn test_disconnect_fails_pending_queries_and_resets_flow() {
    let mut session = ready_session(None, RecordingBackend::default());
    let mut receiver = session.query_orders().unwrap();

    session.handle_event(TdEvent::FrontDisconnected { reason: 0x1001 });
    assert!(!session.is_ready());
    assert!(receiver.try_recv().unwrap().is_err());
    assert!(session.query_orders().is_err());
}
