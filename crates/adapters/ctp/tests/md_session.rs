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

//! Session behavior: login lifecycle, subscription demand, and depth routing.

use std::sync::{Arc, Mutex};

use nautilus_common::{live::sender::EventSender, messages::DataEvent};
use nautilus_core::{string::secret::SecretString, time::get_atomic_clock_realtime};
use nautilus_ctp::md::{
    DepthMarketData, MdBackend, MdEvent, RspError,
    parse::InstrumentState,
    session::{LoginCredentials, LoginStatus, MdSession},
};
use nautilus_model::{data::Data, identifiers::InstrumentId};
use rstest::rstest;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Login {
        broker_id: String,
        user_id: String,
        password: String,
    },
    Logout,
    Subscribe(Vec<String>),
    Unsubscribe(Vec<String>),
}

/// Records every command the session issues.
#[derive(Debug, Clone, Default)]
struct RecordingBackend {
    calls: Arc<Mutex<Vec<Call>>>,
}

impl RecordingBackend {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }
}

impl MdBackend for RecordingBackend {
    fn register_front(&mut self, _address: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn init(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn login(
        &mut self,
        broker_id: &str,
        user_id: &str,
        password: &str,
        _request_id: i32,
    ) -> anyhow::Result<()> {
        self.record(Call::Login {
            broker_id: broker_id.to_string(),
            user_id: user_id.to_string(),
            password: password.to_string(),
        });
        Ok(())
    }

    fn logout(&mut self, _broker_id: &str, _user_id: &str, _request_id: i32) -> anyhow::Result<()> {
        self.record(Call::Logout);
        Ok(())
    }

    fn subscribe(&mut self, instrument_ids: &[String]) -> anyhow::Result<()> {
        self.record(Call::Subscribe(instrument_ids.to_vec()));
        Ok(())
    }

    fn unsubscribe(&mut self, instrument_ids: &[String]) -> anyhow::Result<()> {
        self.record(Call::Unsubscribe(instrument_ids.to_vec()));
        Ok(())
    }
}

struct Fixture {
    session: MdSession<RecordingBackend>,
    backend: RecordingBackend,
    data: tokio::sync::mpsc::UnboundedReceiver<DataEvent>,
}

fn fixture() -> Fixture {
    let backend = RecordingBackend::default();
    let (tx, data) = tokio::sync::mpsc::unbounded_channel();
    let instruments = vec![InstrumentState {
        instrument_id: InstrumentId::from("rb2510.SHFE"),
        price_precision: 1,
        size_precision: 0,
        last_volume: None,
    }];
    let credentials = LoginCredentials {
        broker_id: "9999".to_string(),
        user_id: "123456".to_string(),
        password: SecretString::from("secret"),
    };
    let session = MdSession::new(
        backend.clone(),
        credentials,
        instruments,
        EventSender::from(tx),
        get_atomic_clock_realtime(),
    );
    Fixture {
        session,
        backend,
        data,
    }
}

fn depth(volume: i32, last_price: f64) -> MdEvent {
    MdEvent::Depth(Box::new(DepthMarketData {
        instrument_id: "rb2510".to_string(),
        exchange_id: "SHFE".to_string(),
        action_day: "20251122".to_string(),
        update_time: "09:30:01".to_string(),
        update_millisec: 500,
        last_price,
        bid_price1: 3100.0,
        bid_volume1: 12,
        ask_price1: 3100.2,
        ask_volume1: 7,
        volume,
        turnover: 0.0,
    }))
}

fn login_ok() -> MdEvent {
    MdEvent::Login(Ok("20251122".to_string()))
}

fn drain(data: &mut tokio::sync::mpsc::UnboundedReceiver<DataEvent>) -> Vec<Data> {
    let mut out = Vec::new();
    while let Ok(event) = data.try_recv() {
        if let DataEvent::Data(data) = event {
            out.push(data);
        }
    }
    out
}

#[rstest]
fn test_front_connected_sends_login_with_credentials() {
    let mut f = fixture();
    f.session.handle_event(MdEvent::FrontConnected);

    assert_eq!(
        f.backend.calls(),
        vec![Call::Login {
            broker_id: "9999".to_string(),
            user_id: "123456".to_string(),
            password: "secret".to_string(),
        }]
    );
}

#[rstest]
fn test_login_success_marks_logged_in() {
    let mut f = fixture();
    let status = f.session.status();
    f.session.handle_event(MdEvent::FrontConnected);
    f.session.handle_event(login_ok());

    assert!(f.session.is_logged_in());
    assert_eq!(*status.borrow(), LoginStatus::LoggedIn);
}

#[rstest]
fn test_login_rejection_reports_failure_with_error_id() {
    let mut f = fixture();
    let status = f.session.status();
    f.session.handle_event(MdEvent::FrontConnected);
    f.session.handle_event(MdEvent::Login(Err(RspError {
        error_id: 3,
        message: "invalid password".to_string(),
    })));

    assert!(!f.session.is_logged_in());
    assert_eq!(
        *status.borrow(),
        LoginStatus::Failed("login rejected: [3] invalid password".to_string())
    );
}

#[rstest]
fn test_demand_before_login_is_sent_on_login() {
    let mut f = fixture();
    f.session.set_quotes_demand("rb2510", true).unwrap();
    assert!(f.backend.calls().is_empty());

    f.session.handle_event(MdEvent::FrontConnected);
    f.session.handle_event(login_ok());

    let subscribes: Vec<Call> = f
        .backend
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::Subscribe(_)))
        .collect();
    assert_eq!(
        subscribes,
        vec![Call::Subscribe(vec!["rb2510".to_string()])]
    );
}

#[rstest]
fn test_demand_is_subscribed_once_and_unsubscribed_when_unused() {
    let mut f = fixture();
    f.session.handle_event(MdEvent::FrontConnected);
    f.session.handle_event(login_ok());

    f.session.set_quotes_demand("rb2510", true).unwrap();
    f.session.set_trades_demand("rb2510", true).unwrap();
    f.session.set_quotes_demand("rb2510", false).unwrap();
    assert_eq!(
        f.backend
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Subscribe(_)))
            .count(),
        1,
        "adding a second data type must not subscribe again"
    );
    assert!(
        !f.backend
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Unsubscribe(_))),
        "a remaining data type keeps the symbol subscribed"
    );

    f.session.set_trades_demand("rb2510", false).unwrap();
    assert_eq!(
        f.backend.calls().last().unwrap(),
        &Call::Unsubscribe(vec!["rb2510".to_string()])
    );
}

#[rstest]
fn test_unconfigured_symbol_is_rejected() {
    let mut f = fixture();
    let err = f.session.set_quotes_demand("ag2512", true).unwrap_err();
    assert!(err.to_string().contains("ag2512"));
}

#[rstest]
fn test_depth_before_login_is_ignored() {
    let mut f = fixture();
    f.session.set_quotes_demand("rb2510", true).unwrap();
    f.session.handle_event(depth(100, 3100.0));

    assert!(drain(&mut f.data).is_empty());
}

#[rstest]
fn test_quote_emitted_only_when_quotes_demanded() {
    let mut f = fixture();
    f.session.handle_event(MdEvent::FrontConnected);
    f.session.handle_event(login_ok());
    f.session.set_trades_demand("rb2510", true).unwrap();

    f.session.handle_event(depth(100, 3100.0));
    let events = drain(&mut f.data);
    assert!(
        !events.iter().any(|d| matches!(d, Data::Quote(_))),
        "no quote is emitted without quote demand"
    );
}

#[rstest]
fn test_trade_inferred_from_volume_increase() {
    let mut f = fixture();
    f.session.handle_event(MdEvent::FrontConnected);
    f.session.handle_event(login_ok());
    f.session.set_quotes_demand("rb2510", true).unwrap();
    f.session.set_trades_demand("rb2510", true).unwrap();

    f.session.handle_event(depth(100, 3100.0));
    f.session.handle_event(depth(105, 3100.2));

    let events = drain(&mut f.data);
    let trades: Vec<_> = events
        .iter()
        .filter_map(|d| match d {
            Data::Trade(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].size.to_string(), "5");
    assert_eq!(trades[0].price.to_string(), "3100.2");
}

#[rstest]
fn test_reconnect_resubscribes_wanted_symbols_and_resets_volume_baseline() {
    let mut f = fixture();
    f.session.handle_event(MdEvent::FrontConnected);
    f.session.handle_event(login_ok());
    f.session.set_quotes_demand("rb2510", true).unwrap();
    f.session.handle_event(depth(100, 3100.0));

    f.session
        .handle_event(MdEvent::FrontDisconnected { reason: 0x1001 });
    assert!(!f.session.is_logged_in());

    f.session.handle_event(MdEvent::FrontConnected);
    f.session.handle_event(login_ok());

    let subscribes = f
        .backend
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::Subscribe(s) if s == &vec!["rb2510".to_string()]))
        .count();
    assert_eq!(
        subscribes, 2,
        "wanted symbols are subscribed again after login"
    );
}

#[rstest]
fn test_malformed_depth_is_skipped_without_emitting() {
    let mut f = fixture();
    f.session.handle_event(MdEvent::FrontConnected);
    f.session.handle_event(login_ok());
    f.session.set_quotes_demand("rb2510", true).unwrap();

    let MdEvent::Depth(mut bad) = depth(100, 3100.0) else {
        unreachable!()
    };
    bad.update_time = "not-a-time".to_string();
    f.session.handle_event(MdEvent::Depth(bad));

    assert!(drain(&mut f.data).is_empty());
}

#[rstest]
fn test_shutdown_logs_out_only_when_logged_in() {
    let mut f = fixture();
    f.session.shutdown().unwrap();
    assert!(f.backend.calls().is_empty());

    f.session.handle_event(MdEvent::FrontConnected);
    f.session.handle_event(login_ok());
    f.session.shutdown().unwrap();
    assert_eq!(f.backend.calls().last().unwrap(), &Call::Logout);
}
