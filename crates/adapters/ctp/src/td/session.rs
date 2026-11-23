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

//! Trading session: login flow, order tracking, fill deduplication, and query correlation.
//!
//! The session is synchronous. The client feeds it [`TdEvent`]s in arrival order and receives
//! [`TdOutput`]s to translate into Nautilus events. Order state is keyed by the order reference
//! this session allocates, and the cache is never touched here.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use nautilus_core::string::secret::SecretString;
use nautilus_model::identifiers::ClientOrderId;

use super::{
    AccountRecord, LoginInfo, OrderCancelRequest, OrderInsertRequest, OrderRecord, PositionRecord,
    QueryResponse, ResumeType, TdBackend, TdEvent, TradeRecord,
    parse::{OrderChange, is_terminal, order_changes},
};
use crate::md::RspError;

/// Maximum number of trade IDs remembered for deduplication.
const TRADE_ID_CAPACITY: usize = 100_000;

/// Session status observed by the client while it waits for the first login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Disconnected,
    Connecting,
    Ready,
    Failed(String),
}

/// Credentials and product identity for the trading login.
#[derive(Debug, Clone)]
pub struct TdCredentials {
    pub broker_id: String,
    pub investor_id: String,
    pub password: SecretString,
    /// Application ID and authentication code. Both are required when authentication is used.
    pub app_id: Option<String>,
    pub auth_code: Option<SecretString>,
    pub product_info: String,
}

/// A lifecycle or fill event for an order, or an external record the session did not place.
#[derive(Debug, Clone, PartialEq)]
pub enum TdOutput {
    Accepted {
        client_order_id: ClientOrderId,
        venue_order_id: String,
    },
    Rejected {
        client_order_id: ClientOrderId,
        reason: String,
    },
    Canceled {
        client_order_id: ClientOrderId,
        venue_order_id: Option<String>,
    },
    CancelRejected {
        client_order_id: ClientOrderId,
        venue_order_id: Option<String>,
        reason: String,
    },
    Filled {
        client_order_id: ClientOrderId,
        venue_order_id: String,
        trade: TradeRecord,
    },
    /// An order this session did not place, reported from a return.
    ExternalOrder(OrderRecord),
    /// A fill for an order this session did not place.
    ExternalTrade(TradeRecord),
}

/// Order state for an order this session placed.
#[derive(Debug, Clone)]
struct TrackedOrder {
    client_order_id: ClientOrderId,
    venue_order_id: Option<String>,
    accepted: bool,
    terminal: bool,
    cancel_pending: bool,
    instrument_id: String,
    exchange_id: String,
}

/// Results for a query awaiting its final response.
#[derive(Debug)]
enum PendingQuery {
    Orders {
        rows: Vec<OrderRecord>,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<Vec<OrderRecord>>>,
    },
    Trades {
        rows: Vec<TradeRecord>,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<Vec<TradeRecord>>>,
    },
    Positions {
        rows: Vec<PositionRecord>,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<Vec<PositionRecord>>>,
    },
    Account {
        row: Option<AccountRecord>,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<Option<AccountRecord>>>,
    },
}

impl PendingQuery {
    fn fail(self, message: String) {
        let e = anyhow::anyhow!(message);
        match self {
            Self::Orders { reply, .. } => {
                let _ = reply.send(Err(e));
            }
            Self::Trades { reply, .. } => {
                let _ = reply.send(Err(e));
            }
            Self::Positions { reply, .. } => {
                let _ = reply.send(Err(e));
            }
            Self::Account { reply, .. } => {
                let _ = reply.send(Err(e));
            }
        }
    }
}

/// Protocol step the login flow is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Idle,
    Authenticating,
    LoggingIn,
    ConfirmingSettlement,
    Ready,
}

/// Bounded set of recently seen trade IDs, evicting the oldest first.
#[derive(Debug, Default)]
struct TradeIdCache {
    order: VecDeque<String>,
    seen: HashSet<String>,
}

impl TradeIdCache {
    /// Records a trade ID and returns `false` if it was already seen.
    fn insert(&mut self, trade_id: &str) -> bool {
        if self.seen.contains(trade_id) {
            return false;
        }
        if self.order.len() == TRADE_ID_CAPACITY
            && let Some(oldest) = self.order.pop_front()
        {
            self.seen.remove(&oldest);
        }
        self.order.push_back(trade_id.to_string());
        self.seen.insert(trade_id.to_string());
        true
    }
}

#[derive(Debug)]
pub struct TdSession<B: TdBackend> {
    backend: B,
    credentials: TdCredentials,
    status: tokio::sync::watch::Sender<SessionStatus>,
    flow: Flow,
    next_request_id: i32,
    front_id: i32,
    session_id: i32,
    next_order_ref: u64,
    /// Keyed by order reference. Ordered iteration keeps any emitted sequence deterministic.
    orders: BTreeMap<String, TrackedOrder>,
    refs_by_client: HashMap<ClientOrderId, String>,
    trades: TradeIdCache,
    /// Ordered by request ID so failing pending queries completes them in a fixed order.
    queries: BTreeMap<i32, PendingQuery>,
}

impl<B: TdBackend> TdSession<B> {
    /// Creates a session. Connection starts with [`TdSession::start`].
    pub fn new(backend: B, credentials: TdCredentials) -> Self {
        let (status, _) = tokio::sync::watch::channel(SessionStatus::Disconnected);
        Self {
            backend,
            credentials,
            status,
            flow: Flow::Idle,
            next_request_id: 0,
            front_id: 0,
            session_id: 0,
            next_order_ref: 1,
            orders: BTreeMap::new(),
            refs_by_client: HashMap::new(),
            trades: TradeIdCache::default(),
            queries: BTreeMap::new(),
        }
    }

    /// Returns a receiver that observes session status.
    pub fn status(&self) -> tokio::sync::watch::Receiver<SessionStatus> {
        self.status.subscribe()
    }

    /// Returns whether the session has completed login and settlement confirmation.
    pub fn is_ready(&self) -> bool {
        self.flow == Flow::Ready
    }

    /// Registers the front, sets topic resume modes, and starts connecting.
    ///
    /// Private topics restart so the session replays the day's returns. Its trade and order
    /// handling deduplicates them, so replay is safe.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend rejects the front, the topic setup, or the start.
    pub fn start(&mut self, front_address: &str) -> anyhow::Result<()> {
        self.set_status(SessionStatus::Connecting);
        self.backend.register_front(front_address)?;
        self.backend
            .subscribe_topics(ResumeType::Restart, ResumeType::Quick)?;
        self.backend.init()
    }

    /// Logs out if ready. The backend releases the SDK instance when dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if the logout request is rejected.
    pub fn shutdown(&mut self) -> anyhow::Result<()> {
        self.set_status(SessionStatus::Disconnected);
        let was_ready = self.flow == Flow::Ready;
        self.flow = Flow::Idle;
        self.fail_queries("trading session shut down");
        if !was_ready {
            return Ok(());
        }
        let request_id = self.next_request_id();
        self.backend.logout(
            &self.credentials.broker_id,
            &self.credentials.investor_id,
            request_id,
        )
    }

    /// Handles one SDK event and returns the outputs it produced.
    pub fn handle_event(&mut self, event: TdEvent) -> Vec<TdOutput> {
        let mut outputs = Vec::new();
        match event {
            TdEvent::FrontConnected => self.on_front_connected(),
            TdEvent::FrontDisconnected { reason } => {
                tracing::warn!(reason, "CTP trader front disconnected");
                self.flow = Flow::Idle;
                self.fail_queries("trader front disconnected");
                if !matches!(*self.status.borrow(), SessionStatus::Failed(_)) {
                    self.set_status(SessionStatus::Disconnected);
                }
            }
            TdEvent::HeartbeatWarning { time_lapse } => {
                tracing::warn!(time_lapse, "CTP trader heartbeat warning");
            }
            TdEvent::Authenticate { result, .. } => match (self.flow, result) {
                (Flow::Authenticating, Ok(())) => self.send_login(),
                (Flow::Authenticating, Err(e)) => self.fail_with("authentication", &e),
                _ => {}
            },
            TdEvent::Login { result, .. } => match result {
                Ok(info) => self.on_login(&info),
                Err(e) => self.fail_with("login", &e),
            },
            TdEvent::Logout { .. } => {
                self.flow = Flow::Idle;
            }
            TdEvent::SettlementConfirm { result, .. } => match result {
                Ok(()) => {
                    self.flow = Flow::Ready;
                    self.set_status(SessionStatus::Ready);
                    tracing::info!("CTP trading session ready");
                }
                Err(e) => self.fail_with("settlement confirmation", &e),
            },
            TdEvent::OrderInsertRejected { order_ref, error } => {
                if let Some(order) = self.orders.get_mut(&order_ref)
                    && !order.terminal
                {
                    order.terminal = true;
                    outputs.push(TdOutput::Rejected {
                        client_order_id: order.client_order_id,
                        reason: format_error(&error),
                    });
                } else {
                    tracing::warn!(order_ref, error = %format_error(&error), "insert rejected for unknown order");
                }
            }
            TdEvent::OrderActionRejected {
                order_ref,
                order_sys_id,
                error,
            } => {
                if let Some(order) = self.orders.get_mut(&order_ref)
                    && !order.terminal
                {
                    order.cancel_pending = false;
                    let venue = order.venue_order_id.clone().or(non_empty(order_sys_id));
                    outputs.push(TdOutput::CancelRejected {
                        client_order_id: order.client_order_id,
                        venue_order_id: venue,
                        reason: format_error(&error),
                    });
                } else {
                    tracing::warn!(order_ref, error = %format_error(&error), "cancel rejected for unknown order");
                }
            }
            TdEvent::Order(record) => self.on_order(*record, &mut outputs),
            TdEvent::Trade(record) => self.on_trade(*record, &mut outputs),
            TdEvent::QryOrder(response) => self.on_query_orders(response),
            TdEvent::QryTrade(response) => self.on_query_trades(response),
            TdEvent::QryPosition(response) => self.on_query_positions(response),
            TdEvent::QryAccount(response) => self.on_query_account(response),
            TdEvent::Error { request_id, error } => {
                if let Some(query) = self.queries.remove(&request_id) {
                    query.fail(format_error(&error));
                } else {
                    tracing::error!(error = %format_error(&error), "CTP trader error");
                }
            }
        }
        outputs
    }

    /// Submits a limit or market order that the session has already validated.
    ///
    /// # Errors
    ///
    /// Returns an error if the session is not ready, the client order ID is already in use, or
    /// the backend rejects the insert synchronously.
    pub fn submit(
        &mut self,
        client_order_id: ClientOrderId,
        mut request: OrderInsertRequest,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.is_ready(), "CTP trading session is not ready");
        anyhow::ensure!(
            !self.refs_by_client.contains_key(&client_order_id),
            "client order ID {client_order_id} is already in use"
        );

        let order_ref = self.next_order_ref.to_string();
        self.next_order_ref += 1;
        request.order_ref = order_ref.clone();
        request.broker_id = self.credentials.broker_id.clone();
        request.investor_id = self.credentials.investor_id.clone();

        let tracked = TrackedOrder {
            client_order_id,
            venue_order_id: None,
            accepted: false,
            terminal: false,
            cancel_pending: false,
            instrument_id: request.instrument_id.clone(),
            exchange_id: request.exchange_id.clone(),
        };
        self.orders.insert(order_ref.clone(), tracked);
        self.refs_by_client
            .insert(client_order_id, order_ref.clone());

        let request_id = self.next_request_id();
        if let Err(e) = self.backend.insert_order(&request, request_id) {
            self.orders.remove(&order_ref);
            self.refs_by_client.remove(&client_order_id);
            return Err(e);
        }
        Ok(())
    }

    /// Requests cancellation of a live order this session placed.
    ///
    /// The outcome arrives as [`TdOutput::Canceled`] or [`TdOutput::CancelRejected`].
    ///
    /// # Errors
    ///
    /// Returns an error if the order is unknown, already terminal, or the backend rejects the
    /// request synchronously.
    pub fn cancel(&mut self, client_order_id: ClientOrderId) -> anyhow::Result<()> {
        let order_ref = self
            .refs_by_client
            .get(&client_order_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown order {client_order_id}"))?;
        let order = self
            .orders
            .get(&order_ref)
            .ok_or_else(|| anyhow::anyhow!("unknown order {client_order_id}"))?;
        anyhow::ensure!(
            !order.terminal,
            "order {client_order_id} is already terminal"
        );
        anyhow::ensure!(
            !order.cancel_pending,
            "cancel already pending for order {client_order_id}"
        );

        let cancel = OrderCancelRequest {
            broker_id: self.credentials.broker_id.clone(),
            investor_id: self.credentials.investor_id.clone(),
            instrument_id: order.instrument_id.clone(),
            exchange_id: order.exchange_id.clone(),
            order_ref: order_ref.clone(),
            front_id: self.front_id,
            session_id: self.session_id,
            order_sys_id: order.venue_order_id.clone().unwrap_or_default(),
        };
        let request_id = self.next_request_id();
        self.backend.cancel_order(&cancel, request_id)?;
        if let Some(order) = self.orders.get_mut(&order_ref) {
            order.cancel_pending = true;
        }
        Ok(())
    }

    /// Starts a query for the day's orders. Await the receiver for the complete result.
    ///
    /// # Errors
    ///
    /// Returns an error if the session is not ready or the backend rejects the request.
    pub fn query_orders(
        &mut self,
    ) -> anyhow::Result<tokio::sync::oneshot::Receiver<anyhow::Result<Vec<OrderRecord>>>> {
        self.ensure_ready()?;
        let (reply, receiver) = tokio::sync::oneshot::channel();
        let request_id = self.next_request_id();
        self.backend.query_orders(
            &self.credentials.broker_id,
            &self.credentials.investor_id,
            request_id,
        )?;
        self.queries.insert(
            request_id,
            PendingQuery::Orders {
                rows: Vec::new(),
                reply,
            },
        );
        Ok(receiver)
    }

    /// Starts a query for the day's trades.
    ///
    /// # Errors
    ///
    /// Returns an error if the session is not ready or the backend rejects the request.
    pub fn query_trades(
        &mut self,
    ) -> anyhow::Result<tokio::sync::oneshot::Receiver<anyhow::Result<Vec<TradeRecord>>>> {
        self.ensure_ready()?;
        let (reply, receiver) = tokio::sync::oneshot::channel();
        let request_id = self.next_request_id();
        self.backend.query_trades(
            &self.credentials.broker_id,
            &self.credentials.investor_id,
            request_id,
        )?;
        self.queries.insert(
            request_id,
            PendingQuery::Trades {
                rows: Vec::new(),
                reply,
            },
        );
        Ok(receiver)
    }

    /// Starts a query for investor positions.
    ///
    /// # Errors
    ///
    /// Returns an error if the session is not ready or the backend rejects the request.
    pub fn query_positions(
        &mut self,
    ) -> anyhow::Result<tokio::sync::oneshot::Receiver<anyhow::Result<Vec<PositionRecord>>>> {
        self.ensure_ready()?;
        let (reply, receiver) = tokio::sync::oneshot::channel();
        let request_id = self.next_request_id();
        self.backend.query_positions(
            &self.credentials.broker_id,
            &self.credentials.investor_id,
            request_id,
        )?;
        self.queries.insert(
            request_id,
            PendingQuery::Positions {
                rows: Vec::new(),
                reply,
            },
        );
        Ok(receiver)
    }

    /// Starts a query for the trading account.
    ///
    /// # Errors
    ///
    /// Returns an error if the session is not ready or the backend rejects the request.
    pub fn query_account(
        &mut self,
    ) -> anyhow::Result<tokio::sync::oneshot::Receiver<anyhow::Result<Option<AccountRecord>>>> {
        self.ensure_ready()?;
        let (reply, receiver) = tokio::sync::oneshot::channel();
        let request_id = self.next_request_id();
        self.backend.query_account(
            &self.credentials.broker_id,
            &self.credentials.investor_id,
            request_id,
        )?;
        self.queries
            .insert(request_id, PendingQuery::Account { row: None, reply });
        Ok(receiver)
    }

    /// Returns the front ID and session ID from login, or zeros before login.
    pub fn route(&self) -> (i32, i32) {
        (self.front_id, self.session_id)
    }

    /// Returns the client order ID for an order return or query row placed by this session.
    ///
    /// Rows from another front or session are not this session's orders and return `None`.
    pub fn client_order_id_for(
        &self,
        front_id: i32,
        session_id: i32,
        order_ref: &str,
    ) -> Option<ClientOrderId> {
        if front_id != self.front_id || session_id != self.session_id {
            return None;
        }
        self.orders
            .get(order_ref)
            .map(|order| order.client_order_id)
    }

    /// Returns the client order ID for a trade row placed by this session.
    ///
    /// Trades carry no route, so the venue order ID must match the one recorded for the order.
    pub fn client_order_id_for_trade(
        &self,
        order_ref: &str,
        order_sys_id: &str,
    ) -> Option<ClientOrderId> {
        let order = self.orders.get(order_ref)?;
        match order.venue_order_id.as_deref() {
            Some(venue) if venue != order_sys_id => None,
            _ => Some(order.client_order_id),
        }
    }

    /// Returns the client order IDs of orders that are neither terminal nor being canceled, in
    /// order reference order.
    pub fn live_client_order_ids(&self) -> Vec<ClientOrderId> {
        self.orders
            .values()
            .filter(|order| !order.terminal && !order.cancel_pending)
            .map(|order| order.client_order_id)
            .collect()
    }

    fn ensure_ready(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.is_ready(), "CTP trading session is not ready");
        Ok(())
    }

    fn on_front_connected(&mut self) {
        tracing::info!("CTP trader front connected");
        self.flow = Flow::Idle;
        self.set_status(SessionStatus::Connecting);
        match self
            .credentials
            .app_id
            .clone()
            .zip(self.credentials.auth_code.clone())
        {
            Some((app_id, auth_code)) => {
                let request_id = self.next_request_id();
                let result = self.backend.authenticate(
                    &self.credentials.broker_id,
                    &self.credentials.investor_id,
                    &app_id,
                    auth_code.expose_secret(),
                    &self.credentials.product_info,
                    request_id,
                );
                match result {
                    Ok(()) => self.flow = Flow::Authenticating,
                    Err(e) => self.set_status(SessionStatus::Failed(e.to_string())),
                }
            }
            None => self.send_login(),
        }
    }

    fn send_login(&mut self) {
        let request_id = self.next_request_id();
        let result = self.backend.login(
            &self.credentials.broker_id,
            &self.credentials.investor_id,
            self.credentials.password.expose_secret(),
            &self.credentials.product_info,
            request_id,
        );
        match result {
            Ok(()) => self.flow = Flow::LoggingIn,
            Err(e) => self.set_status(SessionStatus::Failed(e.to_string())),
        }
    }

    fn on_login(&mut self, info: &LoginInfo) {
        if self.flow != Flow::LoggingIn {
            return;
        }
        self.front_id = info.front_id;
        self.session_id = info.session_id;
        // New references start above the largest one the front has already seen.
        let max_ref = info.max_order_ref.parse::<u64>().unwrap_or(0);
        self.next_order_ref = self.next_order_ref.max(max_ref + 1);
        tracing::info!(
            trading_day = %info.trading_day,
            front_id = info.front_id,
            session_id = info.session_id,
            "CTP trader login succeeded",
        );

        self.flow = Flow::ConfirmingSettlement;
        let request_id = self.next_request_id();
        let result = self.backend.confirm_settlement(
            &self.credentials.broker_id,
            &self.credentials.investor_id,
            request_id,
        );
        if let Err(e) = result {
            self.set_status(SessionStatus::Failed(e.to_string()));
        }
    }

    fn fail_with(&mut self, step: &str, error: &RspError) {
        let message = format!("{step} rejected: {}", format_error(error));
        tracing::error!("{message}");
        self.flow = Flow::Idle;
        self.set_status(SessionStatus::Failed(message));
    }

    fn on_order(&mut self, record: OrderRecord, outputs: &mut Vec<TdOutput>) {
        let ours = record.front_id == self.front_id && record.session_id == self.session_id;
        let tracked = ours && self.orders.contains_key(&record.order_ref);
        if !tracked {
            outputs.push(TdOutput::ExternalOrder(record));
            return;
        }

        let order = self
            .orders
            .get_mut(&record.order_ref)
            .expect("order checked above");
        if order.terminal {
            return;
        }
        if !record.order_sys_id.is_empty() {
            order.venue_order_id = Some(record.order_sys_id.clone());
        }

        for change in order_changes(&record, order.accepted) {
            match change {
                OrderChange::Accepted => {
                    order.accepted = true;
                    if let Some(venue) = order.venue_order_id.clone() {
                        outputs.push(TdOutput::Accepted {
                            client_order_id: order.client_order_id,
                            venue_order_id: venue,
                        });
                    }
                }
                OrderChange::Rejected(reason) => {
                    order.terminal = true;
                    outputs.push(TdOutput::Rejected {
                        client_order_id: order.client_order_id,
                        reason,
                    });
                }
                OrderChange::Canceled => {
                    order.terminal = true;
                    outputs.push(TdOutput::Canceled {
                        client_order_id: order.client_order_id,
                        venue_order_id: order.venue_order_id.clone(),
                    });
                }
            }
        }
        if is_terminal(record.order_status) {
            order.terminal = true;
        }
    }

    fn on_trade(&mut self, record: TradeRecord, outputs: &mut Vec<TdOutput>) {
        if !self.trades.insert(&record.trade_id) {
            return;
        }
        let tracked = self.orders.get(&record.order_ref).is_some_and(|order| {
            order
                .venue_order_id
                .as_deref()
                .is_none_or(|venue| venue == record.order_sys_id)
        });
        if !tracked {
            outputs.push(TdOutput::ExternalTrade(record));
            return;
        }

        let order = self
            .orders
            .get_mut(&record.order_ref)
            .expect("order checked above");
        let venue_order_id = if record.order_sys_id.is_empty() {
            order.venue_order_id.clone().unwrap_or_default()
        } else {
            record.order_sys_id.clone()
        };
        if !order.accepted {
            order.accepted = true;
            outputs.push(TdOutput::Accepted {
                client_order_id: order.client_order_id,
                venue_order_id: venue_order_id.clone(),
            });
        }
        outputs.push(TdOutput::Filled {
            client_order_id: order.client_order_id,
            venue_order_id,
            trade: record,
        });
    }

    fn on_query_orders(&mut self, response: QueryResponse<OrderRecord>) {
        let request_id = response.request_id;
        match response.result {
            Err(e) => self.fail_query(request_id, &e),
            Ok(row) => {
                let Some(PendingQuery::Orders { rows, .. }) = self.queries.get_mut(&request_id)
                else {
                    return;
                };
                rows.extend(row);
                if response.is_last
                    && let Some(PendingQuery::Orders { rows, reply }) =
                        self.queries.remove(&request_id)
                {
                    let _ = reply.send(Ok(rows));
                }
            }
        }
    }

    fn on_query_trades(&mut self, response: QueryResponse<TradeRecord>) {
        let request_id = response.request_id;
        match response.result {
            Err(e) => self.fail_query(request_id, &e),
            Ok(row) => {
                let Some(PendingQuery::Trades { rows, .. }) = self.queries.get_mut(&request_id)
                else {
                    return;
                };
                rows.extend(row);
                if response.is_last
                    && let Some(PendingQuery::Trades { rows, reply }) =
                        self.queries.remove(&request_id)
                {
                    let _ = reply.send(Ok(rows));
                }
            }
        }
    }

    fn on_query_positions(&mut self, response: QueryResponse<PositionRecord>) {
        let request_id = response.request_id;
        match response.result {
            Err(e) => self.fail_query(request_id, &e),
            Ok(row) => {
                let Some(PendingQuery::Positions { rows, .. }) = self.queries.get_mut(&request_id)
                else {
                    return;
                };
                rows.extend(row);
                if response.is_last
                    && let Some(PendingQuery::Positions { rows, reply }) =
                        self.queries.remove(&request_id)
                {
                    let _ = reply.send(Ok(rows));
                }
            }
        }
    }

    fn on_query_account(&mut self, response: QueryResponse<AccountRecord>) {
        let request_id = response.request_id;
        match response.result {
            Err(e) => self.fail_query(request_id, &e),
            Ok(row) => {
                let Some(PendingQuery::Account { row: slot, .. }) =
                    self.queries.get_mut(&request_id)
                else {
                    return;
                };
                if row.is_some() {
                    *slot = row;
                }
                if response.is_last
                    && let Some(PendingQuery::Account { row, reply }) =
                        self.queries.remove(&request_id)
                {
                    let _ = reply.send(Ok(row));
                }
            }
        }
    }

    fn fail_query(&mut self, request_id: i32, error: &RspError) {
        if let Some(query) = self.queries.remove(&request_id) {
            query.fail(format_error(error));
        }
    }

    fn fail_queries(&mut self, reason: &str) {
        for query in std::mem::take(&mut self.queries).into_values() {
            query.fail(reason.to_string());
        }
    }

    fn next_request_id(&mut self) -> i32 {
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.next_request_id
    }

    fn set_status(&self, status: SessionStatus) {
        self.status.send_replace(status);
    }
}

fn format_error(error: &RspError) -> String {
    format!("[{}] {}", error.error_id, error.message)
}

fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}
