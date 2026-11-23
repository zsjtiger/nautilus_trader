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

//! Live execution client for CTP.
//!
//! The client places limit and market orders through the trader protocol session. Lifecycle
//! events are built from an [`OrderIdentity`] captured at submission, so the event loop never
//! touches the cache. Order commands run on the engine thread and can read the cache.

use std::{
    collections::{BTreeMap, HashMap},
    fmt::Debug,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use async_trait::async_trait;
use nautilus_common::{
    clients::ExecutionClient,
    live::{dst::time, runner::get_exec_event_sender, sender::EventSender},
    messages::{
        ExecutionEvent,
        execution::{
            CancelAllOrders, CancelOrder, GenerateFillReports, GenerateOrderStatusReports,
            GeneratePositionStatusReports, QueryAccount, SubmitOrder,
        },
    },
};
use nautilus_core::{
    UUID4, UnixNanos,
    time::{AtomicTime, get_atomic_clock_realtime},
};
use nautilus_execution::client::core::ExecutionClientCore;
use nautilus_live::{execution::emitter::ExecutionEventEmitter, task::TaskGroup};
use nautilus_model::{
    accounts::AccountAny,
    enums::{
        AccountType, LiquiditySide, OmsType, OrderSide, OrderStatus, OrderType, PositionSide,
        TimeInForce,
    },
    events::{OrderAccepted, OrderCanceled, OrderEventAny, OrderFilled},
    identifiers::{
        AccountId, ClientId, ClientOrderId, InstrumentId, StrategyId, TradeId, TraderId, Venue,
        VenueOrderId,
    },
    orders::{Order, OrderAny},
    reports::{FillReport, OrderStatusReport, PositionStatusReport},
    types::{AccountBalance, Currency, MarginBalance, Money, Price, Quantity},
};
use rust_decimal::Decimal;
use tokio_util::sync::CancellationToken;

use crate::{
    config::CtpExecClientConfig,
    instruments::instrument_id,
    md::parse::exchange_timestamp_ns,
    native::NativeTdApi,
    td::{
        AccountRecord, OrderInsertRequest, OrderRecord, PositionRecord, TdBackend, TdEvent,
        TradeRecord, direction, offset, position_direction,
        session::{SessionStatus, TdCredentials, TdOutput, TdSession},
        status,
    },
};

/// The venue name used for the client's own identity. Order routing uses each instrument's
/// exchange, which is covered by [`ExecutionClient::handles_order_venue`].
pub const CTP_VENUE: &str = "CTP";

const TASK_GRACE: Duration = Duration::from_secs(2);
const TASK_FORCE: Duration = Duration::from_secs(1);

/// Creates a trading backend for a session, given the validated config and event channel.
pub type TdBackendFactory<B> = Box<
    dyn Fn(&CtpExecClientConfig, tokio::sync::mpsc::UnboundedSender<TdEvent>) -> anyhow::Result<B>,
>;

type SharedSession<B> = Arc<Mutex<TdSession<B>>>;

/// Order fields the event loop needs to build lifecycle events without the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderIdentity {
    pub strategy_id: StrategyId,
    pub instrument_id: InstrumentId,
    pub order_side: OrderSide,
    pub order_type: OrderType,
}

/// Instrument facts the client needs to convert protocol values to Nautilus values.
#[derive(Debug, Clone)]
struct InstrumentSpec {
    instrument_id: InstrumentId,
    price_precision: u8,
    currency: Currency,
    multiplier: Decimal,
}

/// Everything the event loop and the report paths need. It is `Send` and holds no cache handle.
#[derive(Debug, Clone)]
struct Dispatch {
    trader_id: TraderId,
    account_id: AccountId,
    emitter: ExecutionEventEmitter,
    identities: Arc<Mutex<BTreeMap<ClientOrderId, OrderIdentity>>>,
    specs: Arc<HashMap<String, InstrumentSpec>>,
    clock: &'static AtomicTime,
}

impl Dispatch {
    fn now(&self) -> UnixNanos {
        self.clock.get_time_ns()
    }

    fn identity(&self, client_order_id: ClientOrderId) -> Option<OrderIdentity> {
        lock(&self.identities).get(&client_order_id).copied()
    }

    fn forget(&self, client_order_id: ClientOrderId) {
        lock(&self.identities).remove(&client_order_id);
    }

    fn handle(&self, output: TdOutput) {
        match output {
            TdOutput::Accepted {
                client_order_id,
                venue_order_id,
            } => {
                let Some(identity) = self.identity(client_order_id) else {
                    tracing::warn!(%client_order_id, "accepted order has no identity");
                    return;
                };
                let Ok(venue_order_id) = VenueOrderId::new_checked(&venue_order_id) else {
                    tracing::error!(%client_order_id, "invalid venue order ID from CTP");
                    return;
                };
                let now = self.now();
                let event = OrderAccepted::new(
                    self.trader_id,
                    identity.strategy_id,
                    identity.instrument_id,
                    client_order_id,
                    venue_order_id,
                    self.account_id,
                    UUID4::new(),
                    now,
                    now,
                    false,
                );
                self.emitter
                    .send_order_event(OrderEventAny::Accepted(event));
            }
            TdOutput::Rejected {
                client_order_id,
                reason,
            } => {
                if let Some(identity) = self.identity(client_order_id) {
                    self.emitter.emit_order_rejected_event(
                        identity.strategy_id,
                        identity.instrument_id,
                        client_order_id,
                        &reason,
                        self.now(),
                        false,
                    );
                }
                self.forget(client_order_id);
            }
            TdOutput::Canceled {
                client_order_id,
                venue_order_id,
            } => {
                if let Some(identity) = self.identity(client_order_id) {
                    let now = self.now();
                    let event = OrderCanceled::new(
                        self.trader_id,
                        identity.strategy_id,
                        identity.instrument_id,
                        client_order_id,
                        UUID4::new(),
                        now,
                        now,
                        false,
                        venue_order_id.and_then(|id| VenueOrderId::new_checked(&id).ok()),
                        Some(self.account_id),
                        None,
                    );
                    self.emitter
                        .send_order_event(OrderEventAny::Canceled(event));
                }
                self.forget(client_order_id);
            }
            TdOutput::CancelRejected {
                client_order_id,
                venue_order_id,
                reason,
            } => {
                if let Some(identity) = self.identity(client_order_id) {
                    self.emitter.emit_order_cancel_rejected_event(
                        identity.strategy_id,
                        identity.instrument_id,
                        client_order_id,
                        venue_order_id.and_then(|id| VenueOrderId::new_checked(&id).ok()),
                        &reason,
                        self.now(),
                        None,
                    );
                }
            }
            TdOutput::Filled {
                client_order_id,
                venue_order_id,
                trade,
            } => self.handle_fill(client_order_id, &venue_order_id, &trade),
            TdOutput::ExternalOrder(record) => {
                if let Some(report) = self.order_report(&record, None) {
                    self.emitter.send_order_status_report(report);
                }
            }
            TdOutput::ExternalTrade(record) => {
                if let Some(report) = self.fill_report(&record, None) {
                    self.emitter.send_fill_report(report);
                }
            }
        }
    }

    fn handle_fill(
        &self,
        client_order_id: ClientOrderId,
        venue_order_id: &str,
        trade: &TradeRecord,
    ) {
        let Some(identity) = self.identity(client_order_id) else {
            tracing::warn!(%client_order_id, "fill has no identity");
            return;
        };
        let Some(spec) = self.specs.get(&trade.instrument_id) else {
            tracing::error!(instrument = %trade.instrument_id, "fill for unconfigured instrument");
            return;
        };
        let (Ok(venue_order_id), Ok(trade_id), Ok(last_px), Ok(last_qty)) = (
            VenueOrderId::new_checked(venue_order_id),
            TradeId::new_checked(&trade.trade_id),
            Price::new_checked(trade.price, spec.price_precision),
            Quantity::new_checked(f64::from(trade.volume), 0),
        ) else {
            tracing::error!(%client_order_id, trade_id = %trade.trade_id, "invalid fill values from CTP");
            return;
        };
        let ts_event = trade_timestamp(trade).unwrap_or_else(|_| self.now());
        let now = self.now();
        let event = OrderFilled::new(
            self.trader_id,
            identity.strategy_id,
            identity.instrument_id,
            client_order_id,
            venue_order_id,
            self.account_id,
            trade_id,
            identity.order_side,
            identity.order_type,
            last_qty,
            last_px,
            spec.currency,
            LiquiditySide::NoLiquiditySide,
            UUID4::new(),
            ts_event,
            now,
            false,
            None,
            Some(zero_commission(spec.currency)),
            None,
        );
        self.emitter.send_order_event(OrderEventAny::Filled(event));
    }

    /// Builds an order status report from a protocol row. The client order ID is resolved by the
    /// caller when it is known.
    fn order_report(
        &self,
        record: &OrderRecord,
        client_order_id: Option<ClientOrderId>,
    ) -> Option<OrderStatusReport> {
        let spec = self.spec_for_order(record)?;
        let order_side = if record.direction == direction::SELL {
            OrderSide::Sell
        } else {
            OrderSide::Buy
        };
        let (order_type, price) = if record.limit_price > 0.0 {
            (
                OrderType::Limit,
                Price::new_checked(record.limit_price, spec.price_precision).ok(),
            )
        } else {
            (OrderType::Market, None)
        };
        let quantity = Quantity::new_checked(f64::from(record.volume_original), 0).ok()?;
        let filled_qty = Quantity::new_checked(f64::from(record.volume_traded), 0).ok()?;
        let venue_order_id = VenueOrderId::new_checked(venue_id_for(record)).ok()?;
        let ts_accepted = record_timestamp(&record.insert_date, &record.insert_time)
            .unwrap_or_else(|_| self.now());
        let ts_last =
            record_timestamp(&record.insert_date, &record.update_time).unwrap_or(ts_accepted);

        let mut report = OrderStatusReport::new(
            self.account_id,
            spec.instrument_id,
            client_order_id,
            venue_order_id,
            Some(order_side),
            order_type,
            TimeInForce::Day,
            order_status(record),
            quantity,
            filled_qty,
            ts_accepted,
            ts_last,
            self.now(),
            Some(UUID4::new()),
        );
        if let Some(price) = price {
            report = report.with_price(price);
        }
        Some(report)
    }

    fn fill_report(
        &self,
        record: &TradeRecord,
        client_order_id: Option<ClientOrderId>,
    ) -> Option<FillReport> {
        let spec = self.specs.get(&record.instrument_id)?;
        let order_side = if record.direction == direction::SELL {
            OrderSide::Sell
        } else {
            OrderSide::Buy
        };
        Some(FillReport::new(
            self.account_id,
            spec.instrument_id,
            VenueOrderId::new_checked(&record.order_sys_id).ok()?,
            TradeId::new_checked(&record.trade_id).ok()?,
            order_side,
            Quantity::new_checked(f64::from(record.volume), 0).ok()?,
            Price::new_checked(record.price, spec.price_precision).ok()?,
            zero_commission(spec.currency),
            LiquiditySide::NoLiquiditySide,
            client_order_id,
            None,
            trade_timestamp(record).unwrap_or_else(|_| self.now()),
            self.now(),
            Some(UUID4::new()),
        ))
    }

    fn spec_for_order(&self, record: &OrderRecord) -> Option<&InstrumentSpec> {
        self.specs.get(&record.instrument_id)
    }
}

/// Live CTP execution client.
pub struct CtpExecutionClient<B: TdBackend> {
    core: ExecutionClientCore,
    clock: &'static AtomicTime,
    config: CtpExecClientConfig,
    specs: Arc<HashMap<String, InstrumentSpec>>,
    emitter: ExecutionEventEmitter,
    identities: Arc<Mutex<BTreeMap<ClientOrderId, OrderIdentity>>>,
    session: Option<SharedSession<B>>,
    backend_factory: TdBackendFactory<B>,
    tasks: TaskGroup,
    account_id: AccountId,
}

impl<B: TdBackend + Send> Debug for CtpExecutionClient<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(CtpExecutionClient))
            .field("client_id", &self.core.client_id)
            .field("account_id", &self.account_id)
            .field("instruments", &self.specs.len())
            .field(
                "connected",
                &self
                    .session
                    .as_ref()
                    .is_some_and(|session| lock(session).is_ready()),
            )
            .finish_non_exhaustive()
    }
}

impl CtpExecutionClient<NativeTdApi> {
    /// Creates a disconnected client backed by the linked CTP trader SDK.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration or an instrument definition is invalid.
    pub fn new(core: ExecutionClientCore, config: CtpExecClientConfig) -> anyhow::Result<Self> {
        Self::with_backend(
            core,
            config,
            Box::new(|config, events| {
                NativeTdApi::create(&config.flow_path, config.production_mode, events)
            }),
            get_atomic_clock_realtime(),
        )
    }
}

impl<B: TdBackend + 'static> CtpExecutionClient<B> {
    /// Creates a disconnected execution client with a custom backend factory.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration or an instrument definition is invalid.
    pub(crate) fn with_backend(
        core: ExecutionClientCore,
        config: CtpExecClientConfig,
        backend_factory: TdBackendFactory<B>,
        clock: &'static AtomicTime,
    ) -> anyhow::Result<Self> {
        config.validate()?;
        let account_id = core.account_id;
        let mut specs = HashMap::new();
        for definition in &config.instruments {
            let spec = InstrumentSpec {
                instrument_id: instrument_id(definition)?,
                price_precision: u8::try_from(definition.price_increment.scale())?,
                currency: Currency::try_from_str(&definition.currency)
                    .ok_or_else(|| anyhow::anyhow!("unknown currency `{}`", definition.currency))?,
                multiplier: definition.multiplier,
            };
            specs.insert(definition.ctp_symbol.clone(), spec);
        }
        let emitter = ExecutionEventEmitter::new(
            clock,
            core.trader_id,
            account_id,
            AccountType::Margin,
            Some(Currency::CNY()),
        );

        Ok(Self {
            core,
            clock,
            config,
            specs: Arc::new(specs),
            emitter,
            identities: Arc::new(Mutex::new(BTreeMap::new())),
            session: None,
            backend_factory,
            tasks: TaskGroup::new(),
            account_id,
        })
    }

    /// Installs the event sender used by the emitter.
    pub fn set_event_sender(&mut self, sender: impl Into<EventSender<ExecutionEvent>>) {
        self.emitter.set_sender(sender);
    }

    fn dispatch(&self) -> Dispatch {
        Dispatch {
            trader_id: self.core.trader_id,
            account_id: self.account_id,
            emitter: self.emitter.clone(),
            identities: Arc::clone(&self.identities),
            specs: Arc::clone(&self.specs),
            clock: self.clock,
        }
    }

    fn session(&self) -> anyhow::Result<&SharedSession<B>> {
        self.session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("CTP execution client is not connected"))
    }

    fn spec_by_symbol(&self, instrument_id: &InstrumentId) -> Option<&InstrumentSpec> {
        self.specs.get(instrument_id.symbol.as_str())
    }

    async fn tear_down(&mut self) -> anyhow::Result<()> {
        if let Some(session) = self.session.take()
            && let Err(e) = lock(&session).shutdown()
        {
            tracing::warn!(error = %e, "CTP trader logout failed");
        }
        self.tasks.begin_shutdown();
        self.tasks
            .finish_shutdown(TASK_GRACE, TASK_FORCE)
            .await
            .map_err(anyhow::Error::from)
    }

    /// Queries the trading account and emits its state.
    async fn refresh_account(&self) -> anyhow::Result<()> {
        let session = self.session()?.clone();
        let receiver = lock(&session).query_account()?;
        let record = time::timeout(self.query_timeout(), receiver)
            .await
            .map_err(|_| anyhow::anyhow!("timed out querying CTP account"))???
            .ok_or_else(|| anyhow::anyhow!("CTP returned no account record"))?;
        self.emit_account_state(&record);
        Ok(())
    }

    fn emit_account_state(&self, record: &AccountRecord) {
        let Some((balances, margins)) = account_balances(record) else {
            tracing::error!("invalid CTP account balance");
            return;
        };
        self.emitter
            .emit_account_state(balances, margins, true, self.clock.get_time_ns(), None);
    }

    fn query_timeout(&self) -> Duration {
        Duration::from_secs(self.config.query_timeout_secs)
    }
}

#[async_trait(?Send)]
impl<B: TdBackend + 'static> ExecutionClient for CtpExecutionClient<B> {
    fn is_connected(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| lock(session).is_ready())
    }

    fn client_id(&self) -> ClientId {
        self.core.client_id
    }

    fn account_id(&self) -> AccountId {
        self.account_id
    }

    fn venue(&self) -> Venue {
        Venue::from(CTP_VENUE)
    }

    fn oms_type(&self) -> OmsType {
        OmsType::Netting
    }

    fn get_account(&self) -> Option<AccountAny> {
        self.core.cache().account_owned(&self.account_id)
    }

    fn handles_order_venue(&self, venue: Venue) -> bool {
        venue == self.venue()
            || self
                .specs
                .values()
                .any(|spec| spec.instrument_id.venue == venue)
    }

    fn generate_account_state(
        &self,
        balances: Vec<AccountBalance>,
        margins: Vec<MarginBalance>,
        reported: bool,
        ts_event: UnixNanos,
        info: Option<nautilus_core::Params>,
    ) -> anyhow::Result<()> {
        self.emitter
            .emit_account_state(balances, margins, reported, ts_event, info);
        Ok(())
    }

    fn start(&mut self) -> anyhow::Result<()> {
        if self.core.is_started() {
            return Ok(());
        }
        self.emitter.set_sender(get_exec_event_sender());
        self.core.set_started();
        tracing::info!(client_id = %self.core.client_id, "started CTP execution client");
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.core.set_stopped();
        tracing::info!(client_id = %self.core.client_id, "stopped CTP execution client");
        Ok(())
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        if self.is_connected() {
            return Ok(());
        }
        self.config.validate()?;

        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let backend = (self.backend_factory)(&self.config, events_tx)?;
        let credentials = TdCredentials {
            broker_id: self.config.broker_id.clone(),
            investor_id: self.config.investor_id.clone(),
            password: self.config.password.clone(),
            app_id: self.config.app_id.clone(),
            auth_code: self.config.auth_code.clone(),
            product_info: self.config.product_info.clone(),
        };
        let session: SharedSession<B> = Arc::new(Mutex::new(TdSession::new(backend, credentials)));
        let mut status: tokio::sync::watch::Receiver<SessionStatus> = lock(&session).status();

        self.tasks.begin_shutdown();
        self.tasks
            .finish_shutdown(TASK_GRACE, TASK_FORCE)
            .await
            .map_err(anyhow::Error::from)?;
        self.tasks.start_generation()?;

        let cancel: CancellationToken = self.tasks.cancellation_token();
        let loop_session = Arc::clone(&session);
        let dispatch = self.dispatch();
        self.tasks.spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    event = events_rx.recv() => match event {
                        Some(event) => {
                            let outputs = lock(&loop_session).handle_event(event);
                            for output in outputs {
                                dispatch.handle(output);
                            }
                        }
                        None => break,
                    },
                }
            }
        })?;
        self.session = Some(Arc::clone(&session));

        lock(&session).start(&self.config.trade_front_address)?;

        let login_timeout = Duration::from_secs(self.config.login_timeout_secs);
        let outcome = time::timeout(
            login_timeout,
            status.wait_for(|s| matches!(s, SessionStatus::Ready | SessionStatus::Failed(_))),
        )
        .await;
        let failure = match outcome {
            Err(_) => Some(format!(
                "timed out after {}s waiting for CTP trading login",
                self.config.login_timeout_secs
            )),
            Ok(Err(_)) => Some("CTP trading session ended before login".to_string()),
            Ok(Ok(state)) => match &*state {
                SessionStatus::Failed(message) => Some(message.clone()),
                _ => None,
            },
        };
        if let Some(message) = failure {
            self.tear_down().await?;
            anyhow::bail!("CTP trading connect failed: {message}");
        }

        if let Err(e) = self.refresh_account().await {
            self.tear_down().await?;
            anyhow::bail!("CTP trading connect failed while loading the account: {e}");
        }

        self.core.set_connected();
        tracing::info!(client_id = %self.core.client_id, "CTP execution client connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.core.set_disconnected();
        self.tear_down().await
    }

    fn submit_order(&self, cmd: SubmitOrder) -> anyhow::Result<()> {
        let order = self.core.get_order(&cmd.client_order_id)?;

        let request = match self.spec_by_symbol(&order.instrument_id()) {
            Some(spec) => build_insert_request(&order, spec),
            None => Err("instrument is not configured for CTP".to_string()),
        };
        let request = match request {
            Ok(request) => request,
            Err(reason) => {
                self.emitter.emit_order_denied(&order, &reason);
                return Ok(());
            }
        };

        let Some(session) = self.session.as_ref() else {
            self.emitter.emit_order_rejected(
                &order,
                "CTP execution client is not connected",
                self.clock.get_time_ns(),
                false,
            );
            return Ok(());
        };

        self.emitter.emit_order_submitted(&order);
        lock(&self.identities).insert(
            order.client_order_id(),
            OrderIdentity {
                strategy_id: order.strategy_id(),
                instrument_id: order.instrument_id(),
                order_side: order.order_side(),
                order_type: order.order_type(),
            },
        );

        let result = lock(session).submit(order.client_order_id(), request);
        if let Err(e) = result {
            lock(&self.identities).remove(&order.client_order_id());
            self.emitter.emit_order_rejected(
                &order,
                &e.to_string(),
                self.clock.get_time_ns(),
                false,
            );
        }
        Ok(())
    }

    fn cancel_order(&self, cmd: CancelOrder) -> anyhow::Result<()> {
        let result = match self.session() {
            Ok(session) => lock(session).cancel(cmd.client_order_id),
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            self.emitter.emit_order_cancel_rejected_event(
                cmd.strategy_id,
                cmd.instrument_id,
                cmd.client_order_id,
                cmd.venue_order_id,
                &e.to_string(),
                self.clock.get_time_ns(),
                None,
            );
        }
        Ok(())
    }

    fn cancel_all_orders(&self, cmd: CancelAllOrders) -> anyhow::Result<()> {
        let session = self.session()?;
        let live = lock(session).live_client_order_ids();
        for client_order_id in live {
            let Some(identity) = lock(&self.identities).get(&client_order_id).copied() else {
                continue;
            };
            if identity.instrument_id != cmd.instrument_id {
                continue;
            }
            if cmd
                .order_side
                .is_some_and(|side| side != identity.order_side)
            {
                continue;
            }
            let result = lock(session).cancel(client_order_id);
            if let Err(e) = result {
                self.emitter.emit_order_cancel_rejected_event(
                    cmd.strategy_id,
                    cmd.instrument_id,
                    client_order_id,
                    None,
                    &e.to_string(),
                    self.clock.get_time_ns(),
                    None,
                );
            }
        }
        Ok(())
    }

    fn query_account(&self, _cmd: QueryAccount) -> anyhow::Result<()> {
        let session = self.session()?.clone();
        let receiver = lock(&session).query_account()?;
        let emitter = self.emitter.clone();
        let clock = self.clock;
        let timeout = self.query_timeout();
        self.tasks.spawn(async move {
            match time::timeout(timeout, receiver).await {
                Ok(Ok(Ok(Some(record)))) => match account_balances(&record) {
                    Some((balances, margins)) => {
                        emitter.emit_account_state(
                            balances,
                            margins,
                            true,
                            clock.get_time_ns(),
                            None,
                        );
                    }
                    None => tracing::error!("invalid CTP account balance"),
                },
                Ok(Ok(Ok(None))) => tracing::warn!("CTP returned no account record"),
                Ok(Ok(Err(e))) => tracing::error!(error = %e, "CTP account query failed"),
                Ok(Err(_)) => tracing::error!("CTP account query was dropped"),
                Err(_) => tracing::error!("timed out querying CTP account"),
            }
        })?;
        Ok(())
    }

    async fn generate_order_status_reports(
        &self,
        cmd: &GenerateOrderStatusReports,
    ) -> anyhow::Result<Vec<OrderStatusReport>> {
        let session = self.session()?.clone();
        let receiver = lock(&session).query_orders()?;
        let rows = time::timeout(self.query_timeout(), receiver)
            .await
            .map_err(|_| anyhow::anyhow!("timed out querying CTP orders"))???;

        let dispatch = self.dispatch();
        let guard = lock(&session);
        let mut reports = Vec::new();
        for row in rows {
            let client_order_id =
                guard.client_order_id_for(row.front_id, row.session_id, &row.order_ref);
            let Some(report) = dispatch.order_report(&row, client_order_id) else {
                tracing::warn!(order_ref = %row.order_ref, "skipping CTP order with unknown instrument");
                continue;
            };
            if cmd.open_only && is_closed(report.order_status) {
                continue;
            }
            if cmd
                .instrument_id
                .is_some_and(|id| id != report.instrument_id)
            {
                continue;
            }
            reports.push(report);
        }
        Ok(reports)
    }

    async fn generate_fill_reports(
        &self,
        cmd: GenerateFillReports,
    ) -> anyhow::Result<Vec<FillReport>> {
        let session = self.session()?.clone();
        let receiver = lock(&session).query_trades()?;
        let rows = time::timeout(self.query_timeout(), receiver)
            .await
            .map_err(|_| anyhow::anyhow!("timed out querying CTP trades"))???;

        let dispatch = self.dispatch();
        let guard = lock(&session);
        let mut reports = Vec::new();
        for row in rows {
            let client_order_id =
                guard.client_order_id_for_trade(&row.order_ref, &row.order_sys_id);
            let Some(report) = dispatch.fill_report(&row, client_order_id) else {
                continue;
            };
            if cmd
                .instrument_id
                .is_some_and(|id| id != report.instrument_id)
            {
                continue;
            }
            reports.push(report);
        }
        Ok(reports)
    }

    async fn generate_position_status_reports(
        &self,
        cmd: &GeneratePositionStatusReports,
    ) -> anyhow::Result<Vec<PositionStatusReport>> {
        let session = self.session()?.clone();
        let receiver = lock(&session).query_positions()?;
        let rows = time::timeout(self.query_timeout(), receiver)
            .await
            .map_err(|_| anyhow::anyhow!("timed out querying CTP positions"))???;

        let netted = net_positions(&rows);
        let mut reports = Vec::new();
        for (symbol, net) in netted {
            let Some(spec) = self.specs.get(&symbol) else {
                tracing::warn!(instrument = %symbol, "skipping CTP position for unconfigured instrument");
                continue;
            };
            if cmd.instrument_id.is_some_and(|id| id != spec.instrument_id) {
                continue;
            }
            if net.quantity == 0 {
                continue;
            }
            let side = if net.quantity > 0 {
                PositionSide::Long
            } else {
                PositionSide::Short
            };
            let Ok(quantity) = Quantity::new_checked(f64::from(net.quantity.abs()), 0) else {
                continue;
            };
            let avg_px_open = net.average_price(spec.multiplier);
            reports.push(PositionStatusReport::new(
                self.account_id,
                spec.instrument_id,
                side,
                quantity,
                self.clock.get_time_ns(),
                self.clock.get_time_ns(),
                Some(UUID4::new()),
                None,
                avg_px_open,
            ));
        }
        Ok(reports)
    }
}

/// Net position for one instrument, from long and short records.
#[derive(Debug, Default)]
struct NetPosition {
    quantity: i32,
    long_qty: i32,
    long_cost: f64,
    short_qty: i32,
    short_cost: f64,
}

impl NetPosition {
    /// Returns the average open price of the dominant side, or `None` when the side is flat.
    fn average_price(&self, multiplier: Decimal) -> Option<Decimal> {
        let (qty, cost) = if self.quantity > 0 {
            (self.long_qty, self.long_cost)
        } else {
            (self.short_qty, self.short_cost)
        };
        if qty <= 0 {
            return None;
        }
        let multiplier = f64::try_from(multiplier).ok()?;
        let price = cost / (f64::from(qty) * multiplier);
        Decimal::try_from(price).ok()
    }
}

fn net_positions(rows: &[PositionRecord]) -> BTreeMap<String, NetPosition> {
    let mut out: BTreeMap<String, NetPosition> = BTreeMap::new();
    for row in rows {
        let entry = out.entry(row.instrument_id.clone()).or_default();
        match row.direction {
            position_direction::LONG => {
                entry.long_qty += row.position;
                entry.long_cost += row.position_cost;
            }
            position_direction::SHORT => {
                entry.short_qty += row.position;
                entry.short_cost += row.position_cost;
            }
            _ => {}
        }
    }
    for entry in out.values_mut() {
        entry.quantity = entry.long_qty - entry.short_qty;
    }
    out
}

/// Builds the insert request for an order, or the reason it cannot be sent to CTP.
fn build_insert_request(
    order: &OrderAny,
    spec: &InstrumentSpec,
) -> Result<OrderInsertRequest, String> {
    if order.is_post_only() {
        return Err("post-only orders are not supported by CTP".to_string());
    }
    if order.is_quote_quantity() {
        return Err("quote-quantity orders are not supported by CTP".to_string());
    }
    if order.trigger_price().is_some() {
        return Err("trigger orders are not supported by CTP".to_string());
    }

    let direction = match order.order_side() {
        OrderSide::Buy => direction::BUY,
        OrderSide::Sell => direction::SELL,
    };

    let quantity = order.quantity().as_f64();
    if quantity.fract() != 0.0 || quantity <= 0.0 {
        return Err(format!(
            "CTP orders must be a positive whole number of lots, was {quantity}"
        ));
    }
    let volume = i32::try_from(quantity as i64)
        .map_err(|_| format!("order quantity {quantity} exceeds CTP limits"))?;

    let (price_type, limit_price) = match order.order_type() {
        OrderType::Limit => {
            let price = order
                .price()
                .ok_or_else(|| "limit order has no price".to_string())?;
            ('2', price.as_f64())
        }
        OrderType::Market => ('1', 0.0),
        other => return Err(format!("unsupported order type {other:?}")),
    };

    let (time_condition, volume_condition) = match (order.order_type(), order.time_in_force()) {
        (OrderType::Limit, TimeInForce::Day) => ('3', '1'),
        (OrderType::Limit, TimeInForce::Ioc) => ('1', '1'),
        (OrderType::Limit, TimeInForce::Fok) => ('1', '3'),
        (OrderType::Market, TimeInForce::Ioc) => ('1', '1'),
        (OrderType::Market, TimeInForce::Fok) => ('1', '3'),
        (_, TimeInForce::Gtc) => {
            return Err("CTP does not support GTC; use Day, IOC, or FOK".to_string());
        }
        (order_type, tif) => {
            return Err(format!(
                "CTP does not support {tif:?} time in force for {order_type:?} orders"
            ));
        }
    };

    let offset_flag = if order.is_reduce_only() {
        offset::CLOSE
    } else {
        offset::OPEN
    };

    Ok(OrderInsertRequest {
        broker_id: String::new(),
        investor_id: String::new(),
        instrument_id: spec.instrument_id.symbol.as_str().to_string(),
        exchange_id: spec.instrument_id.venue.as_str().to_string(),
        order_ref: String::new(),
        direction,
        price_type,
        limit_price,
        volume,
        time_condition,
        volume_condition,
        offset_flag,
    })
}

fn is_closed(status: OrderStatus) -> bool {
    matches!(
        status,
        OrderStatus::Filled
            | OrderStatus::Canceled
            | OrderStatus::Expired
            | OrderStatus::Rejected
            | OrderStatus::Denied
    )
}

fn order_status(record: &OrderRecord) -> OrderStatus {
    if record.order_submit_status == crate::td::submit_status::INSERT_REJECTED {
        return OrderStatus::Rejected;
    }
    match record.order_status {
        status::ALL_TRADED => OrderStatus::Filled,
        status::PART_TRADED_QUEUEING => OrderStatus::PartiallyFilled,
        status::NO_TRADE_QUEUEING => OrderStatus::Accepted,
        status::PART_TRADED_NOT_QUEUEING | status::NO_TRADE_NOT_QUEUEING | status::CANCELED => {
            OrderStatus::Canceled
        }
        _ => OrderStatus::Submitted,
    }
}

fn venue_id_for(record: &OrderRecord) -> String {
    if record.order_sys_id.is_empty() {
        format!(
            "{}:{}:{}",
            record.front_id, record.session_id, record.order_ref
        )
    } else {
        record.order_sys_id.clone()
    }
}

/// Converts an account record to balances and margins, or `None` if the values are inconsistent.
fn account_balances(record: &AccountRecord) -> Option<(Vec<AccountBalance>, Vec<MarginBalance>)> {
    let currency = Currency::try_from_str(&record.currency_id).unwrap_or(Currency::CNY());
    let total = Money::new(record.balance, currency);
    let free = Money::new(record.available, currency);
    let locked = Money::new((record.balance - record.available).max(0.0), currency);
    let balance = AccountBalance::new_checked(total, locked, free).ok()?;
    let margin_amount = Money::new(record.curr_margin.max(0.0), currency);
    let margin = MarginBalance::new(margin_amount, margin_amount, None);
    Some((vec![balance], vec![margin]))
}

fn zero_commission(currency: Currency) -> Money {
    Money::new(0.0, currency)
}

fn record_timestamp(date: &str, time: &str) -> anyhow::Result<UnixNanos> {
    exchange_timestamp_ns(date, time, 0)
}

fn trade_timestamp(record: &TradeRecord) -> anyhow::Result<UnixNanos> {
    exchange_timestamp_ns(&record.trade_date, &record.trade_time, 0)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, time::Duration};

    use nautilus_common::{
        cache::{Cache, CacheView},
        messages::execution::CancelOrder,
    };
    use nautilus_core::UUID4;
    use nautilus_model::{
        enums::{OrderSide, OrderType, TimeInForce},
        identifiers::{ClientId, StrategyId, TraderId},
        orders::{OrderAny, builder::OrderTestBuilder},
        types::{Price, Quantity},
    };
    use rstest::rstest;

    use super::*;
    use crate::{
        config::CtpInstrumentDefinition,
        md::RspError,
        td::{
            AccountRecord, LoginInfo, OrderCancelRequest, OrderInsertRequest, OrderRecord,
            QueryResponse, ResumeType, TdBackend, TdEvent, TradeRecord, submit_status,
        },
    };

    fn spec() -> InstrumentSpec {
        InstrumentSpec {
            instrument_id: InstrumentId::from("rb2510.SHFE"),
            price_precision: 1,
            currency: Currency::CNY(),
            multiplier: Decimal::from(10),
        }
    }

    fn limit_order(side: OrderSide, tif: TimeInForce) -> OrderAny {
        OrderTestBuilder::new(OrderType::Limit)
            .instrument_id(InstrumentId::from("rb2510.SHFE"))
            .side(side)
            .quantity(Quantity::from(10))
            .price(Price::from("3100.0"))
            .time_in_force(tif)
            .build()
    }

    fn limit_order_with(
        side: OrderSide,
        tif: TimeInForce,
        quantity: &str,
        post_only: bool,
    ) -> OrderAny {
        OrderTestBuilder::new(OrderType::Limit)
            .instrument_id(InstrumentId::from("rb2510.SHFE"))
            .side(side)
            .quantity(Quantity::from(quantity))
            .price(Price::from("3100.0"))
            .time_in_force(tif)
            .post_only(post_only)
            .build()
    }

    fn market_order(side: OrderSide, tif: TimeInForce) -> OrderAny {
        OrderTestBuilder::new(OrderType::Market)
            .instrument_id(InstrumentId::from("rb2510.SHFE"))
            .side(side)
            .quantity(Quantity::from(3))
            .time_in_force(tif)
            .build()
    }

    #[rstest]
    fn test_limit_day_buy_maps_to_open_gfd_request() {
        let order = limit_order(OrderSide::Buy, TimeInForce::Day);
        let request = build_insert_request(&order, &spec()).unwrap();
        assert_eq!(request.direction, direction::BUY);
        assert_eq!(request.price_type, '2');
        assert_eq!(request.limit_price, 3100.0);
        assert_eq!(request.volume, 10);
        assert_eq!(request.time_condition, '3');
        assert_eq!(request.volume_condition, '1');
        assert_eq!(request.offset_flag, offset::OPEN);
        assert_eq!(request.exchange_id, "SHFE");
        assert_eq!(request.instrument_id, "rb2510");
    }

    #[rstest]
    fn test_market_ioc_sell_maps_to_any_price_request() {
        let order = market_order(OrderSide::Sell, TimeInForce::Ioc);
        let request = build_insert_request(&order, &spec()).unwrap();
        assert_eq!(request.direction, direction::SELL);
        assert_eq!(request.price_type, '1');
        assert_eq!(request.limit_price, 0.0);
        assert_eq!(request.time_condition, '1');
    }

    #[rstest]
    fn test_limit_fok_requires_complete_volume() {
        let order = limit_order(OrderSide::Buy, TimeInForce::Fok);
        let request = build_insert_request(&order, &spec()).unwrap();
        assert_eq!(request.time_condition, '1');
        assert_eq!(request.volume_condition, '3');
    }

    #[rstest]
    fn test_reduce_only_maps_to_close_offset() {
        let order = OrderTestBuilder::new(OrderType::Limit)
            .instrument_id(InstrumentId::from("rb2510.SHFE"))
            .side(OrderSide::Sell)
            .quantity(Quantity::from(10))
            .price(Price::from("3100.0"))
            .time_in_force(TimeInForce::Day)
            .reduce_only(true)
            .build();
        let request = build_insert_request(&order, &spec()).unwrap();
        assert_eq!(request.offset_flag, offset::CLOSE);
    }

    #[rstest]
    #[case(limit_order(OrderSide::Buy, TimeInForce::Gtc), "GTC")]
    #[case(
        limit_order_with(OrderSide::Buy, TimeInForce::Day, "10", true),
        "post-only"
    )]
    #[case(
        limit_order_with(OrderSide::Buy, TimeInForce::Day, "1.5", false),
        "whole number"
    )]
    #[case(market_order(OrderSide::Buy, TimeInForce::Day), "does not support")]
    fn test_unsupported_orders_are_denied(#[case] order: OrderAny, #[case] reason: &str) {
        let err = build_insert_request(&order, &spec()).unwrap_err();
        assert!(err.contains(reason), "{err}");
    }

    #[rstest]
    fn test_order_status_maps_protocol_states() {
        let mut record = OrderRecord {
            front_id: 1,
            session_id: 2,
            order_ref: "1".to_string(),
            exchange_id: "SHFE".to_string(),
            order_sys_id: "9".to_string(),
            instrument_id: "rb2510".to_string(),
            direction: direction::BUY,
            order_status: status::NO_TRADE_QUEUEING,
            order_submit_status: submit_status::ACCEPTED,
            volume_original: 10,
            volume_traded: 0,
            volume_remaining: 10,
            limit_price: 3100.0,
            status_msg: String::new(),
            insert_date: "20251122".to_string(),
            insert_time: "09:30:00".to_string(),
            update_time: "09:30:00".to_string(),
            trading_day: "20251122".to_string(),
        };
        assert_eq!(order_status(&record), OrderStatus::Accepted);
        record.order_status = status::PART_TRADED_QUEUEING;
        assert_eq!(order_status(&record), OrderStatus::PartiallyFilled);
        record.order_status = status::ALL_TRADED;
        assert_eq!(order_status(&record), OrderStatus::Filled);
        record.order_status = status::CANCELED;
        assert_eq!(order_status(&record), OrderStatus::Canceled);
        record.order_status = status::UNKNOWN;
        assert_eq!(order_status(&record), OrderStatus::Submitted);
        record.order_submit_status = submit_status::INSERT_REJECTED;
        assert_eq!(order_status(&record), OrderStatus::Rejected);
    }

    #[rstest]
    fn test_account_balances_split_locked_and_free() {
        let record = AccountRecord {
            account_id: "123456".to_string(),
            currency_id: "CNY".to_string(),
            trading_day: "20251122".to_string(),
            balance: 100_000.0,
            available: 90_000.0,
            curr_margin: 5_000.0,
            commission: 0.0,
            position_profit: 0.0,
            close_profit: 0.0,
            deposit: 100_000.0,
            withdraw: 0.0,
        };
        let (balances, margins) = account_balances(&record).unwrap();
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].total.as_f64(), 100_000.0);
        assert_eq!(balances[0].free.as_f64(), 90_000.0);
        assert_eq!(balances[0].locked.as_f64(), 10_000.0);
        assert_eq!(margins[0].initial.as_f64(), 5_000.0);
    }

    // -----------------------------------------------------------------------------------------
    // End-to-end flow through a scripted trading backend
    // -----------------------------------------------------------------------------------------

    /// Answers every command with the protocol response a real front would send.
    #[derive(Debug, Clone)]
    struct ScriptedBackend {
        events: tokio::sync::mpsc::UnboundedSender<TdEvent>,
        /// Volume to fill immediately when an order is inserted, if any.
        fill_on_insert: Option<i32>,
        /// Every command the session sent, in order.
        transcript: Arc<Mutex<Vec<String>>>,
    }

    impl ScriptedBackend {
        fn record(&self, line: String) {
            lock(&self.transcript).push(line);
        }
    }

    impl TdBackend for ScriptedBackend {
        fn register_front(&mut self, _address: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn subscribe_topics(
            &mut self,
            _private: ResumeType,
            _public: ResumeType,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn init(&mut self) -> anyhow::Result<()> {
            self.events.send(TdEvent::FrontConnected)?;
            Ok(())
        }

        fn authenticate(
            &mut self,
            _broker_id: &str,
            _user_id: &str,
            _app_id: &str,
            _auth_code: &str,
            _product_info: &str,
            _request_id: i32,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn login(
            &mut self,
            _broker_id: &str,
            _user_id: &str,
            _password: &str,
            _product_info: &str,
            request_id: i32,
        ) -> anyhow::Result<()> {
            self.events.send(TdEvent::Login {
                request_id,
                result: Ok(LoginInfo {
                    trading_day: "20251122".to_string(),
                    front_id: 7,
                    session_id: 42,
                    max_order_ref: "0".to_string(),
                }),
            })?;
            Ok(())
        }

        fn logout(
            &mut self,
            _broker_id: &str,
            _user_id: &str,
            _request_id: i32,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn confirm_settlement(
            &mut self,
            _broker_id: &str,
            _investor_id: &str,
            request_id: i32,
        ) -> anyhow::Result<()> {
            self.events.send(TdEvent::SettlementConfirm {
                request_id,
                result: Ok(()),
            })?;
            Ok(())
        }

        fn insert_order(
            &mut self,
            order: &OrderInsertRequest,
            request_id: i32,
        ) -> anyhow::Result<()> {
            self.record(format!("insert {request_id} {order:?}"));
            self.events.send(TdEvent::Order(Box::new(order_row(
                &order.order_ref,
                status::NO_TRADE_QUEUEING,
                0,
            ))))?;
            if let Some(volume) = self.fill_on_insert {
                self.events.send(TdEvent::Trade(Box::new(TradeRecord {
                    trade_id: "T1".to_string(),
                    order_ref: order.order_ref.clone(),
                    order_sys_id: "5001".to_string(),
                    exchange_id: "SHFE".to_string(),
                    instrument_id: "rb2510".to_string(),
                    direction: order.direction,
                    offset_flag: order.offset_flag,
                    price: 3100.0,
                    volume,
                    trade_date: "20251122".to_string(),
                    trade_time: "09:30:02".to_string(),
                    trading_day: "20251122".to_string(),
                })))?;
            }
            Ok(())
        }

        fn cancel_order(
            &mut self,
            cancel: &OrderCancelRequest,
            request_id: i32,
        ) -> anyhow::Result<()> {
            self.record(format!("cancel {request_id} {cancel:?}"));
            self.events.send(TdEvent::Order(Box::new(order_row(
                &cancel.order_ref,
                status::CANCELED,
                0,
            ))))?;
            Ok(())
        }

        fn query_orders(
            &mut self,
            _broker_id: &str,
            _investor_id: &str,
            request_id: i32,
        ) -> anyhow::Result<()> {
            self.events.send(TdEvent::QryOrder(QueryResponse {
                request_id,
                is_last: true,
                result: Ok(None),
            }))?;
            Ok(())
        }

        fn query_trades(
            &mut self,
            _broker_id: &str,
            _investor_id: &str,
            request_id: i32,
        ) -> anyhow::Result<()> {
            self.events.send(TdEvent::QryTrade(QueryResponse {
                request_id,
                is_last: true,
                result: Ok(None),
            }))?;
            Ok(())
        }

        fn query_positions(
            &mut self,
            _broker_id: &str,
            _investor_id: &str,
            request_id: i32,
        ) -> anyhow::Result<()> {
            self.events.send(TdEvent::QryPosition(QueryResponse {
                request_id,
                is_last: true,
                result: Ok(None),
            }))?;
            Ok(())
        }

        fn query_account(
            &mut self,
            _broker_id: &str,
            _investor_id: &str,
            request_id: i32,
        ) -> anyhow::Result<()> {
            self.events.send(TdEvent::QryAccount(QueryResponse {
                request_id,
                is_last: true,
                result: Ok(Some(AccountRecord {
                    account_id: "123456".to_string(),
                    currency_id: "CNY".to_string(),
                    trading_day: "20251122".to_string(),
                    balance: 100_000.0,
                    available: 90_000.0,
                    curr_margin: 5_000.0,
                    commission: 0.0,
                    position_profit: 0.0,
                    close_profit: 0.0,
                    deposit: 100_000.0,
                    withdraw: 0.0,
                })),
            }))?;
            Ok(())
        }
    }

    fn order_row(order_ref: &str, order_status: char, traded: i32) -> OrderRecord {
        OrderRecord {
            front_id: 7,
            session_id: 42,
            order_ref: order_ref.to_string(),
            exchange_id: "SHFE".to_string(),
            order_sys_id: "5001".to_string(),
            instrument_id: "rb2510".to_string(),
            direction: direction::BUY,
            order_status,
            order_submit_status: submit_status::ACCEPTED,
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

    fn config() -> CtpExecClientConfig {
        CtpExecClientConfig {
            broker_id: "9999".to_string(),
            investor_id: "123456".to_string(),
            password: nautilus_core::string::secret::SecretString::from("secret"),
            trade_front_address: "tcp://127.0.0.1:10130".to_string(),
            app_id: None,
            auth_code: None,
            product_info: String::new(),
            flow_path: String::new(),
            production_mode: false,
            login_timeout_secs: 2,
            query_timeout_secs: 2,
            instruments: vec![CtpInstrumentDefinition {
                ctp_symbol: "rb2510".to_string(),
                exchange: "SHFE".to_string(),
                underlying: "rb".to_string(),
                asset_class: nautilus_model::enums::AssetClass::Commodity,
                currency: "CNY".to_string(),
                expiry_date: "20251015".to_string(),
                price_increment: Decimal::new(1, 1),
                multiplier: Decimal::new(10, 0),
            }],
        }
    }

    fn client(
        fill_on_insert: Option<i32>,
        cache: CacheView,
    ) -> (
        CtpExecutionClient<ScriptedBackend>,
        tokio::sync::mpsc::UnboundedReceiver<ExecutionEvent>,
    ) {
        let (client, rx, _transcript) = client_with_transcript(fill_on_insert, cache);
        (client, rx)
    }

    fn client_with_transcript(
        fill_on_insert: Option<i32>,
        cache: CacheView,
    ) -> (
        CtpExecutionClient<ScriptedBackend>,
        tokio::sync::mpsc::UnboundedReceiver<ExecutionEvent>,
        Arc<Mutex<Vec<String>>>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let transcript = Arc::new(Mutex::new(Vec::new()));
        let transcript_for_backend = Arc::clone(&transcript);
        let core = ExecutionClientCore::new(
            TraderId::from("TRADER-001"),
            ClientId::from("CTP"),
            Venue::from(CTP_VENUE),
            OmsType::Netting,
            AccountId::from("CTP-123456"),
            AccountType::Margin,
            Some(Currency::CNY()),
            cache,
        );
        let factory: TdBackendFactory<ScriptedBackend> = Box::new(move |_, events| {
            Ok(ScriptedBackend {
                events,
                fill_on_insert,
                transcript: Arc::clone(&transcript_for_backend),
            })
        });
        let mut client =
            CtpExecutionClient::with_backend(core, config(), factory, fixed_clock()).unwrap();
        client.set_event_sender(tx);
        (client, rx, transcript)
    }

    fn cache_with_order(order: &OrderAny) -> CacheView {
        let cache = Rc::new(RefCell::new(Cache::default()));
        cache
            .borrow_mut()
            .add_order(order.clone(), None, None, false)
            .unwrap();
        cache.into()
    }

    async fn next_event(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<ExecutionEvent>,
    ) -> ExecutionEvent {
        time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for execution event")
            .expect("event channel closed")
    }

    fn order_event(event: ExecutionEvent) -> OrderEventAny {
        match event {
            ExecutionEvent::Order(order_event) => order_event,
            other => panic!("expected an order event, was {other:?}"),
        }
    }

    fn submit_command(order: &OrderAny) -> SubmitOrder {
        SubmitOrder::new(
            order.trader_id(),
            Some(ClientId::from("CTP")),
            order.strategy_id(),
            order.instrument_id(),
            order.client_order_id(),
            order.init_event().clone(),
            None,
            None,
            None,
            UUID4::new(),
            nautilus_core::UnixNanos::default(),
            None,
        )
    }

    fn sample_order() -> OrderAny {
        OrderTestBuilder::new(OrderType::Limit)
            .trader_id(TraderId::from("TRADER-001"))
            .strategy_id(StrategyId::from("S-001"))
            .instrument_id(InstrumentId::from("rb2510.SHFE"))
            .client_order_id(ClientOrderId::from("O-1"))
            .side(OrderSide::Buy)
            .quantity(Quantity::from(10))
            .price(Price::from("3100.0"))
            .time_in_force(TimeInForce::Day)
            .build()
    }

    #[tokio::test]
    async fn test_connect_emits_account_state_and_trades_an_order_lifecycle() {
        let order = sample_order();
        let cache = cache_with_order(&order);
        let (mut client, mut rx) = client(None, cache);

        client.connect().await.unwrap();
        assert!(client.is_connected());
        assert!(matches!(
            next_event(&mut rx).await,
            ExecutionEvent::Account(_)
        ));

        client.submit_order(submit_command(&order)).unwrap();
        assert!(matches!(
            order_event(next_event(&mut rx).await),
            OrderEventAny::Submitted(_)
        ));
        assert!(matches!(
            order_event(next_event(&mut rx).await),
            OrderEventAny::Accepted(_)
        ));

        client
            .cancel_order(CancelOrder::new(
                order.trader_id(),
                Some(ClientId::from("CTP")),
                order.strategy_id(),
                order.instrument_id(),
                order.client_order_id(),
                None,
                UUID4::new(),
                nautilus_core::UnixNanos::default(),
                None,
                None,
            ))
            .unwrap();
        assert!(matches!(
            order_event(next_event(&mut rx).await),
            OrderEventAny::Canceled(_)
        ));

        client.disconnect().await.unwrap();
        assert!(!client.is_connected());
    }

    #[tokio::test]
    async fn test_full_fill_emits_filled_event_with_zero_commission() {
        let order = sample_order();
        let cache = cache_with_order(&order);
        let (mut client, mut rx) = client(Some(10), cache);
        client.connect().await.unwrap();
        let _account = next_event(&mut rx).await;

        client.submit_order(submit_command(&order)).unwrap();
        let _submitted = next_event(&mut rx).await;
        assert!(matches!(
            order_event(next_event(&mut rx).await),
            OrderEventAny::Accepted(_)
        ));

        let OrderEventAny::Filled(fill) = order_event(next_event(&mut rx).await) else {
            panic!("expected a fill");
        };
        assert_eq!(fill.last_qty.as_f64(), 10.0);
        assert_eq!(fill.last_px.as_f64(), 3100.0);
        assert_eq!(fill.trade_id.to_string(), "T1");
        assert_eq!(fill.commission.map(|c| c.as_f64()), Some(0.0));
        client.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn test_unsupported_order_is_denied_without_reaching_the_backend() {
        let order = OrderTestBuilder::new(OrderType::Limit)
            .trader_id(TraderId::from("TRADER-001"))
            .strategy_id(StrategyId::from("S-001"))
            .instrument_id(InstrumentId::from("rb2510.SHFE"))
            .client_order_id(ClientOrderId::from("O-GTC"))
            .side(OrderSide::Buy)
            .quantity(Quantity::from(10))
            .price(Price::from("3100.0"))
            .time_in_force(TimeInForce::Gtc)
            .build();
        let cache = cache_with_order(&order);
        let (mut client, mut rx) = client(None, cache);
        client.connect().await.unwrap();
        let _account = next_event(&mut rx).await;

        client.submit_order(submit_command(&order)).unwrap();
        let OrderEventAny::Denied(denied) = order_event(next_event(&mut rx).await) else {
            panic!("expected a denial");
        };
        assert!(denied.reason.as_str().contains("GTC"));
        client.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn test_connect_fails_when_login_is_rejected() {
        #[derive(Debug)]
        struct RejectingBackend(tokio::sync::mpsc::UnboundedSender<TdEvent>);
        impl TdBackend for RejectingBackend {
            fn register_front(&mut self, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            fn subscribe_topics(&mut self, _: ResumeType, _: ResumeType) -> anyhow::Result<()> {
                Ok(())
            }
            fn init(&mut self) -> anyhow::Result<()> {
                self.0.send(TdEvent::FrontConnected)?;
                Ok(())
            }
            fn authenticate(
                &mut self,
                _: &str,
                _: &str,
                _: &str,
                _: &str,
                _: &str,
                _: i32,
            ) -> anyhow::Result<()> {
                Ok(())
            }
            fn login(
                &mut self,
                _: &str,
                _: &str,
                _: &str,
                _: &str,
                request_id: i32,
            ) -> anyhow::Result<()> {
                self.0.send(TdEvent::Login {
                    request_id,
                    result: Err(RspError {
                        error_id: 3,
                        message: "invalid password".to_string(),
                    }),
                })?;
                Ok(())
            }
            fn logout(&mut self, _: &str, _: &str, _: i32) -> anyhow::Result<()> {
                Ok(())
            }
            fn confirm_settlement(&mut self, _: &str, _: &str, _: i32) -> anyhow::Result<()> {
                Ok(())
            }
            fn insert_order(&mut self, _: &OrderInsertRequest, _: i32) -> anyhow::Result<()> {
                Ok(())
            }
            fn cancel_order(&mut self, _: &OrderCancelRequest, _: i32) -> anyhow::Result<()> {
                Ok(())
            }
            fn query_orders(&mut self, _: &str, _: &str, _: i32) -> anyhow::Result<()> {
                Ok(())
            }
            fn query_trades(&mut self, _: &str, _: &str, _: i32) -> anyhow::Result<()> {
                Ok(())
            }
            fn query_positions(&mut self, _: &str, _: &str, _: i32) -> anyhow::Result<()> {
                Ok(())
            }
            fn query_account(&mut self, _: &str, _: &str, _: i32) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let core = ExecutionClientCore::new(
            TraderId::from("TRADER-001"),
            ClientId::from("CTP"),
            Venue::from(CTP_VENUE),
            OmsType::Netting,
            AccountId::from("CTP-123456"),
            AccountType::Margin,
            Some(Currency::CNY()),
            Rc::new(RefCell::new(Cache::default())),
        );
        let factory: TdBackendFactory<RejectingBackend> =
            Box::new(|_, events| Ok(RejectingBackend(events)));
        let mut client =
            CtpExecutionClient::with_backend(core, config(), factory, get_atomic_clock_realtime())
                .unwrap();
        let err = client.connect().await.unwrap_err();
        assert!(err.to_string().contains("invalid password"), "{err}");
        assert!(!client.is_connected());
    }

    /// A clock frozen at a fixed instant, so event timestamps do not depend on the host.
    fn fixed_clock() -> &'static AtomicTime {
        Box::leak(Box::new(AtomicTime::new(
            false,
            UnixNanos::from(1_763_000_000_000_000_000_u64),
        )))
    }

    /// Renders an event by its protocol-determined fields only. Event IDs are random by design
    /// and are excluded, so two runs with the same inputs compare equal.
    fn canonical(event: &ExecutionEvent) -> String {
        match event {
            ExecutionEvent::Account(state) => {
                format!("account total={}", state.balances[0].total.as_f64())
            }
            ExecutionEvent::Order(OrderEventAny::Submitted(e)) => {
                format!("submitted {} ts={}", e.client_order_id, e.ts_event.as_u64())
            }
            ExecutionEvent::Order(OrderEventAny::Accepted(e)) => format!(
                "accepted {} venue={} ts={}",
                e.client_order_id,
                e.venue_order_id,
                e.ts_event.as_u64()
            ),
            ExecutionEvent::Order(OrderEventAny::Filled(e)) => format!(
                "filled {} trade={} qty={} px={} ts={}",
                e.client_order_id,
                e.trade_id,
                e.last_qty.as_f64(),
                e.last_px.as_f64(),
                e.ts_event.as_u64()
            ),
            ExecutionEvent::Order(OrderEventAny::Canceled(e)) => {
                format!("canceled {} ts={}", e.client_order_id, e.ts_event.as_u64())
            }
            _ => "other".to_string(),
        }
    }

    /// The wire transcript and the domain output of one run.
    #[derive(Debug, PartialEq)]
    struct Run {
        transcript: Vec<String>,
        events: Vec<String>,
    }

    async fn run_lifecycle(fill_volume: i32) -> Run {
        let order = sample_order();
        let cache = cache_with_order(&order);
        let (mut client, mut rx, transcript) = client_with_transcript(Some(fill_volume), cache);

        client.connect().await.unwrap();
        let mut events = vec![canonical(&next_event(&mut rx).await)];
        client.submit_order(submit_command(&order)).unwrap();
        for _ in 0..3 {
            events.push(canonical(&next_event(&mut rx).await));
        }
        client.disconnect().await.unwrap();

        Run {
            transcript: lock(&transcript).clone(),
            events,
        }
    }

    #[tokio::test]
    async fn test_replay_with_fixed_inputs_is_identical_across_fresh_clients() {
        let first = run_lifecycle(10).await;
        let second = run_lifecycle(10).await;
        assert_eq!(
            first.transcript, second.transcript,
            "wire transcripts differ"
        );
        assert_eq!(first.events, second.events, "domain output differs");
        assert_eq!(first.transcript.len(), 1, "one insert is expected");
    }

    #[tokio::test]
    async fn test_changed_input_changes_the_transcript_and_output() {
        // Calibrated mutation: a different fill size must be visible in the output.
        let baseline = run_lifecycle(10).await;
        let mutated = run_lifecycle(9).await;
        assert_ne!(baseline.events, mutated.events);
    }

    #[rstest]
    fn test_net_positions_nets_long_and_short() {
        let rows = vec![
            PositionRecord {
                exchange_id: "SHFE".to_string(),
                instrument_id: "rb2510".to_string(),
                direction: position_direction::LONG,
                position: 5,
                yd_position: 0,
                position_cost: 15_500.0,
            },
            PositionRecord {
                exchange_id: "SHFE".to_string(),
                instrument_id: "rb2510".to_string(),
                direction: position_direction::SHORT,
                position: 2,
                yd_position: 0,
                position_cost: 6_200.0,
            },
        ];
        let netted = net_positions(&rows);
        assert_eq!(netted["rb2510"].quantity, 3);
    }

    #[rstest]
    fn test_average_price_uses_dominant_side() {
        let net = NetPosition {
            quantity: 3,
            long_qty: 5,
            long_cost: 15_500.0,
            short_qty: 2,
            short_cost: 6_200.0,
        };
        // 15500 / (5 lots * multiplier 10) = 310.
        let avg = net.average_price(Decimal::from(10)).unwrap();
        assert_eq!(avg, Decimal::from(310));
    }
}
