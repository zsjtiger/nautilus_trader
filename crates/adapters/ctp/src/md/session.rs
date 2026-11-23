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

//! Login lifecycle, subscription demand, and depth routing for one market data session.
//!
//! The session is synchronous: the client feeds it [`MdEvent`]s in arrival order and it issues
//! backend commands and emits data events. Keeping the logic here, separate from the async
//! client, makes the reconnect and subscription rules testable without a live front.

use std::collections::{BTreeMap, HashMap, HashSet};

use nautilus_common::{live::sender::EventSender, messages::DataEvent};
use nautilus_core::{string::secret::SecretString, time::AtomicTime};
use nautilus_model::{data::Data, identifiers::InstrumentId};

use super::{
    DepthMarketData, MdBackend, MdEvent,
    parse::{InstrumentState, parse_depth},
};

/// Login progress as seen by the client while it waits for the first successful login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStatus {
    Disconnected,
    Connecting,
    LoggedIn,
    Failed(String),
}

/// Which data types a CTP symbol is subscribed for.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Demand {
    quotes: bool,
    trades: bool,
}

impl Demand {
    fn is_wanted(self) -> bool {
        self.quotes || self.trades
    }
}

/// Credentials used for the market data login.
#[derive(Debug, Clone)]
pub struct LoginCredentials {
    pub broker_id: String,
    pub user_id: String,
    pub password: SecretString,
}

#[derive(Debug)]
pub struct MdSession<B: MdBackend> {
    backend: B,
    credentials: LoginCredentials,
    instruments: HashMap<String, InstrumentState>,
    /// Ordered by CTP symbol so resubscription sends the same request on every run.
    demand: BTreeMap<String, Demand>,
    subscribed: HashSet<String>,
    logged_in: bool,
    next_request_id: i32,
    status: tokio::sync::watch::Sender<LoginStatus>,
    sender: EventSender<DataEvent>,
    clock: &'static AtomicTime,
}

impl<B: MdBackend> MdSession<B> {
    /// Creates a session for the given CTP symbols.
    ///
    /// Each entry keys its instrument by CTP symbol, which is the name the SDK uses on the wire.
    pub fn new(
        backend: B,
        credentials: LoginCredentials,
        instruments: Vec<InstrumentState>,
        sender: EventSender<DataEvent>,
        clock: &'static AtomicTime,
    ) -> Self {
        let instruments = instruments
            .into_iter()
            .map(|state| (raw_symbol(&state.instrument_id), state))
            .collect();
        let (status, _) = tokio::sync::watch::channel(LoginStatus::Disconnected);

        Self {
            backend,
            credentials,
            instruments,
            demand: BTreeMap::new(),
            subscribed: HashSet::new(),
            logged_in: false,
            next_request_id: 0,
            status,
            sender,
            clock,
        }
    }

    /// Returns a receiver that observes login progress.
    pub fn status(&self) -> tokio::sync::watch::Receiver<LoginStatus> {
        self.status.subscribe()
    }

    /// Returns whether the session has completed a login and has not since disconnected.
    pub fn is_logged_in(&self) -> bool {
        self.logged_in
    }

    /// Registers the front and starts connecting.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend rejects the address or cannot start.
    pub fn start(&mut self, front_address: &str) -> anyhow::Result<()> {
        self.set_status(LoginStatus::Connecting);
        self.backend.register_front(front_address)?;
        self.backend.init()
    }

    /// Logs out if logged in. The backend releases the SDK instance when dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if the logout request is rejected.
    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        self.set_status(LoginStatus::Disconnected);
        if !self.logged_in {
            return Ok(());
        }
        self.logged_in = false;
        self.subscribed.clear();
        let request_id = self.next_request_id();
        self.backend.logout(
            &self.credentials.broker_id,
            &self.credentials.user_id,
            request_id,
        )
    }

    /// Records quote demand for a CTP symbol and subscribes if the symbol is newly wanted.
    ///
    /// # Errors
    ///
    /// Returns an error if the symbol is not configured or the subscribe request is rejected.
    pub fn set_quotes_demand(&mut self, ctp_symbol: &str, wanted: bool) -> anyhow::Result<()> {
        self.update_demand(ctp_symbol, |demand| demand.quotes = wanted)
    }

    /// Records trade demand for a CTP symbol and subscribes if the symbol is newly wanted.
    ///
    /// # Errors
    ///
    /// Returns an error if the symbol is not configured or the subscribe request is rejected.
    pub fn set_trades_demand(&mut self, ctp_symbol: &str, wanted: bool) -> anyhow::Result<()> {
        self.update_demand(ctp_symbol, |demand| demand.trades = wanted)
    }

    /// Handles one SDK event in arrival order.
    pub fn handle_event(&mut self, event: MdEvent) {
        match event {
            MdEvent::FrontConnected => self.on_front_connected(),
            MdEvent::FrontDisconnected { reason } => self.on_front_disconnected(reason),
            MdEvent::HeartbeatWarning { time_lapse } => {
                tracing::warn!(time_lapse, "CTP market data heartbeat warning");
            }
            MdEvent::Login(result) => self.on_login(result),
            MdEvent::Logout(result) => {
                self.logged_in = false;
                self.subscribed.clear();
                if let Err(e) = result {
                    tracing::warn!(error_id = e.error_id, message = %e.message, "CTP logout rejected");
                }
            }
            MdEvent::Subscribe {
                instrument_id,
                result,
            } => {
                if let Err(e) = result {
                    tracing::error!(instrument_id, error_id = e.error_id, message = %e.message, "CTP subscribe rejected");
                }
            }
            MdEvent::Unsubscribe {
                instrument_id,
                result,
            } => {
                if let Err(e) = result {
                    tracing::warn!(instrument_id, error_id = e.error_id, message = %e.message, "CTP unsubscribe rejected");
                }
            }
            MdEvent::Error(e) => {
                tracing::error!(error_id = e.error_id, message = %e.message, "CTP market data error");
            }
            MdEvent::Depth(depth) => self.on_depth(&depth),
        }
    }

    fn on_front_connected(&mut self) {
        tracing::info!("CTP market data front connected");
        self.logged_in = false;
        self.subscribed.clear();
        self.set_status(LoginStatus::Connecting);
        let request_id = self.next_request_id();
        if let Err(e) = self.backend.login(
            &self.credentials.broker_id,
            &self.credentials.user_id,
            self.credentials.password.expose_secret(),
            request_id,
        ) {
            self.set_status(LoginStatus::Failed(e.to_string()));
        }
    }

    fn on_front_disconnected(&mut self, reason: i32) {
        tracing::warn!(reason, "CTP market data front disconnected");
        self.logged_in = false;
        self.subscribed.clear();
        self.set_status(LoginStatus::Disconnected);
    }

    fn on_login(&mut self, result: Result<String, super::RspError>) {
        match result {
            Ok(trading_day) => {
                tracing::info!(trading_day, "CTP market data login succeeded");
                self.logged_in = true;
                self.set_status(LoginStatus::LoggedIn);
                self.resubscribe();
            }
            Err(e) => {
                tracing::error!(error_id = e.error_id, message = %e.message, "CTP market data login rejected");
                self.set_status(LoginStatus::Failed(format!(
                    "login rejected: [{}] {}",
                    e.error_id, e.message
                )));
            }
        }
    }

    fn on_depth(&mut self, depth: &DepthMarketData) {
        if !self.logged_in {
            return;
        }
        let Some(state) = self.instruments.get_mut(&depth.instrument_id) else {
            tracing::debug!(instrument_id = %depth.instrument_id, "depth for unconfigured CTP instrument");
            return;
        };
        let demand = self
            .demand
            .get(&depth.instrument_id)
            .copied()
            .unwrap_or_default();

        let parsed = match parse_depth(depth, state, self.clock.get_time_ns()) {
            Ok(parsed) => parsed,
            Err(e) => {
                tracing::warn!(instrument_id = %depth.instrument_id, error = %e, "skipping malformed CTP depth");
                return;
            }
        };

        if demand.quotes
            && let Some(quote) = parsed.quote
        {
            self.emit(Data::Quote(quote));
        }
        if demand.trades
            && let Some(trade) = parsed.trade
        {
            self.emit(Data::Trade(trade));
        }
    }

    fn update_demand(
        &mut self,
        ctp_symbol: &str,
        change: impl FnOnce(&mut Demand),
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.instruments.contains_key(ctp_symbol),
            "CTP instrument `{ctp_symbol}` is not configured"
        );

        let demand = self.demand.entry(ctp_symbol.to_string()).or_default();
        let was_wanted = demand.is_wanted();
        change(demand);
        let is_wanted = demand.is_wanted();

        if self.logged_in {
            if is_wanted && !was_wanted {
                self.backend.subscribe(&[ctp_symbol.to_string()])?;
                self.subscribed.insert(ctp_symbol.to_string());
            } else if !is_wanted && was_wanted {
                self.backend.unsubscribe(&[ctp_symbol.to_string()])?;
                self.subscribed.remove(ctp_symbol);
            }
        }
        Ok(())
    }

    /// Subscribes every wanted symbol after a login. The SDK drops subscriptions on disconnect,
    /// so the session always re-sends them.
    fn resubscribe(&mut self) {
        let symbols: Vec<String> = self
            .demand
            .iter()
            .filter(|(_, demand)| demand.is_wanted())
            .map(|(symbol, _)| symbol.clone())
            .collect();

        if symbols.is_empty() {
            return;
        }

        match self.backend.subscribe(&symbols) {
            Ok(()) => self.subscribed.extend(symbols),
            Err(e) => tracing::error!(error = %e, "failed to resubscribe CTP market data"),
        }
    }

    fn emit(&self, data: Data) {
        if let Err(e) = self.sender.send(DataEvent::Data(data)) {
            tracing::error!(error = %e, "failed to send CTP market data event");
        }
    }

    fn set_status(&self, status: LoginStatus) {
        self.status.send_replace(status);
    }

    fn next_request_id(&mut self) -> i32 {
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.next_request_id
    }
}

/// Returns the CTP symbol for an instrument ID, which is the part before the venue suffix.
pub(crate) fn raw_symbol(instrument_id: &InstrumentId) -> String {
    instrument_id.symbol.as_str().to_string()
}
