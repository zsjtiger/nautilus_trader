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

//! Live market data client for CTP.

use std::{
    fmt::Debug,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use crate::{
    config::CtpDataClientConfig,
    instruments::{futures_contract, instrument_id},
    md::{
        MdBackend, MdEvent,
        parse::InstrumentState,
        session::{LoginCredentials, LoginStatus, MdSession},
    },
    native::NativeMdApi,
};
use async_trait::async_trait;
use nautilus_common::{
    clients::DataClient,
    live::sender::EventSender,
    messages::{
        DataEvent,
        data::{SubscribeQuotes, SubscribeTrades, UnsubscribeQuotes, UnsubscribeTrades},
    },
};
use nautilus_core::time::{AtomicTime, get_atomic_clock_realtime};
use nautilus_live::task::TaskGroup;
use nautilus_model::{
    data::Data,
    identifiers::{ClientId, Venue},
    instruments::InstrumentAny,
};

const TASK_GRACE: Duration = Duration::from_secs(2);
const TASK_FORCE: Duration = Duration::from_secs(1);

/// Creates a backend for a session, given the validated config and the event channel it must use.
type BackendFactory<B> = Box<
    dyn Fn(&CtpDataClientConfig, tokio::sync::mpsc::UnboundedSender<MdEvent>) -> anyhow::Result<B>,
>;

type SharedSession<B> = Arc<Mutex<MdSession<B>>>;

/// CTP market data client.
///
/// Quotes come from the CTP depth snapshot. Trades are inferred from the change in cumulative
/// volume, so they carry no aggressor side. Instruments come from the configuration, because the
/// market data feed carries no contract reference data.
pub struct CtpDataClient<B: MdBackend> {
    client_id: ClientId,
    config: CtpDataClientConfig,
    instruments: Vec<InstrumentAny>,
    sender: EventSender<DataEvent>,
    backend_factory: BackendFactory<B>,
    clock: &'static AtomicTime,
    session: Option<SharedSession<B>>,
    tasks: TaskGroup,
}

impl<B: MdBackend> Debug for CtpDataClient<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(CtpDataClient))
            .field("client_id", &self.client_id)
            .field("front_address", &self.config.front_address)
            .field("instruments", &self.config.instruments.len())
            .field(
                "connected",
                &self
                    .session
                    .as_ref()
                    .is_some_and(|session| lock(session).is_logged_in()),
            )
            .finish_non_exhaustive()
    }
}

impl<B: MdBackend + 'static> CtpDataClient<B> {
    /// Creates a disconnected client with a custom backend factory.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration or an instrument definition is invalid.
    fn with_backend(
        client_id: ClientId,
        config: CtpDataClientConfig,
        sender: EventSender<DataEvent>,
        backend_factory: BackendFactory<B>,
    ) -> anyhow::Result<Self> {
        config.validate()?;

        let clock = get_atomic_clock_realtime();
        let ts_init = clock.get_time_ns();
        let instruments = config
            .instruments
            .iter()
            .map(|definition| {
                futures_contract(definition, ts_init).map(InstrumentAny::FuturesContract)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Self {
            client_id,
            config,
            instruments,
            sender,
            backend_factory,
            clock,
            session: None,
            tasks: TaskGroup::new(),
        })
    }

    fn session(&self) -> anyhow::Result<&SharedSession<B>> {
        self.session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("CTP market data client is not connected"))
    }

    /// Stops the session and its event loop. Logout is best effort.
    async fn tear_down(&mut self) -> anyhow::Result<()> {
        if let Some(session) = self.session.take()
            && let Err(e) = lock(&session).shutdown()
        {
            tracing::warn!(error = %e, "CTP market data logout failed");
        }
        self.tasks.begin_shutdown();
        self.tasks
            .finish_shutdown(TASK_GRACE, TASK_FORCE)
            .await
            .map_err(anyhow::Error::from)
    }

    fn publish_instruments(&self) {
        for instrument in &self.instruments {
            let event = DataEvent::Data(Data::Instrument(Box::new(instrument.clone())));
            if let Err(e) = self.sender.send(event) {
                tracing::error!(error = %e, "failed to publish CTP instrument");
            }
        }
    }
}

#[async_trait(?Send)]
impl<B: MdBackend + 'static> DataClient for CtpDataClient<B> {
    fn client_id(&self) -> ClientId {
        self.client_id
    }

    fn venue(&self) -> Option<Venue> {
        None
    }

    fn start(&mut self) -> anyhow::Result<()> {
        tracing::info!(client_id = %self.client_id, "starting CTP market data client");
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        tracing::info!(client_id = %self.client_id, "stopping CTP market data client");
        Ok(())
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn dispose(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| lock(session).is_logged_in())
    }

    fn is_disconnected(&self) -> bool {
        !self.is_connected()
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        if self.is_connected() {
            return Ok(());
        }

        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let backend = (self.backend_factory)(&self.config, events_tx)?;

        let states = self
            .config
            .instruments
            .iter()
            .map(|definition| {
                Ok(InstrumentState {
                    instrument_id: instrument_id(definition)?,
                    price_precision: u8::try_from(definition.price_increment.scale())?,
                    size_precision: 0,
                    last_volume: None,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let credentials = LoginCredentials {
            broker_id: self.config.broker_id.clone(),
            user_id: self.config.user_id.clone(),
            password: self.config.password.clone(),
        };

        let session: SharedSession<B> = Arc::new(Mutex::new(MdSession::new(
            backend,
            credentials,
            states,
            self.sender.clone(),
            self.clock,
        )));

        let mut status: tokio::sync::watch::Receiver<LoginStatus> = lock(&session).status();

        // Drain any previous generation, then open a fresh one for this session's tasks.
        self.tasks.begin_shutdown();
        self.tasks
            .finish_shutdown(TASK_GRACE, TASK_FORCE)
            .await
            .map_err(anyhow::Error::from)?;
        self.tasks.start_generation()?;
        let cancel = self.tasks.cancellation_token();
        let loop_session = Arc::clone(&session);
        self.tasks.spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    event = events_rx.recv() => match event {
                        Some(event) => lock(&loop_session).handle_event(event),
                        None => break,
                    },
                }
            }
        })?;
        self.session = Some(Arc::clone(&session));

        lock(&session).start(&self.config.front_address)?;

        let timeout = Duration::from_secs(self.config.login_timeout_secs);
        let outcome = tokio::time::timeout(
            timeout,
            status.wait_for(|s| matches!(s, LoginStatus::LoggedIn | LoginStatus::Failed(_))),
        )
        .await;

        let failure = match outcome {
            Err(_) => Some(format!(
                "timed out after {}s waiting for CTP market data login",
                self.config.login_timeout_secs
            )),
            Ok(Err(_)) => Some("CTP market data session ended before login".to_string()),
            Ok(Ok(state)) => match &*state {
                LoginStatus::Failed(message) => Some(message.clone()),
                _ => None,
            },
        };

        if let Some(message) = failure {
            self.tear_down().await?;
            anyhow::bail!("CTP market data connect failed: {message}");
        }

        self.publish_instruments();
        tracing::info!(client_id = %self.client_id, "CTP market data client connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.tear_down().await
    }

    fn subscribe_quotes(&mut self, cmd: SubscribeQuotes) -> anyhow::Result<()> {
        let session = self.session()?.clone();
        for id in &cmd.instrument_ids {
            lock(&session).set_quotes_demand(id.symbol.as_str(), true)?;
        }
        Ok(())
    }

    fn unsubscribe_quotes(&mut self, cmd: &UnsubscribeQuotes) -> anyhow::Result<()> {
        let session = self.session()?.clone();
        for id in &cmd.instrument_ids {
            lock(&session).set_quotes_demand(id.symbol.as_str(), false)?;
        }
        Ok(())
    }

    fn subscribe_trades(&mut self, cmd: SubscribeTrades) -> anyhow::Result<()> {
        let session = self.session()?.clone();
        for id in &cmd.instrument_ids {
            lock(&session).set_trades_demand(id.symbol.as_str(), true)?;
        }
        Ok(())
    }

    fn unsubscribe_trades(&mut self, cmd: &UnsubscribeTrades) -> anyhow::Result<()> {
        let session = self.session()?.clone();
        for id in &cmd.instrument_ids {
            lock(&session).set_trades_demand(id.symbol.as_str(), false)?;
        }
        Ok(())
    }
}

impl CtpDataClient<NativeMdApi> {
    /// Creates a disconnected client backed by the linked CTP SDK.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration or an instrument definition is invalid.
    pub fn new(
        client_id: ClientId,
        config: CtpDataClientConfig,
        sender: EventSender<DataEvent>,
    ) -> anyhow::Result<Self> {
        Self::with_backend(
            client_id,
            config,
            sender,
            Box::new(|config, events| {
                NativeMdApi::create(&config.flow_path, config.production_mode, events)
            }),
        )
    }
}

/// Locks a session, recovering the guard if a previous holder panicked.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use nautilus_core::{UUID4, UnixNanos, string::secret::SecretString};
    use nautilus_model::{
        enums::AssetClass,
        identifiers::{ClientId, InstrumentId},
    };
    use rust_decimal::Decimal;

    use super::*;
    use crate::{
        config::CtpInstrumentDefinition,
        md::{DepthMarketData, RspError},
    };

    /// Emits scripted events in response to commands, so the client flow can run end to end.
    #[derive(Debug)]
    struct ScriptedBackend {
        events: tokio::sync::mpsc::UnboundedSender<MdEvent>,
        reject_login: bool,
    }

    impl ScriptedBackend {
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
    }

    impl MdBackend for ScriptedBackend {
        fn register_front(&mut self, _address: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn init(&mut self) -> anyhow::Result<()> {
            self.events.send(MdEvent::FrontConnected)?;
            Ok(())
        }

        fn login(
            &mut self,
            _broker_id: &str,
            _user_id: &str,
            _password: &str,
            _request_id: i32,
        ) -> anyhow::Result<()> {
            let result = if self.reject_login {
                Err(RspError {
                    error_id: 3,
                    message: "invalid password".to_string(),
                })
            } else {
                Ok("20251122".to_string())
            };
            self.events.send(MdEvent::Login(result))?;
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

        fn subscribe(&mut self, _instrument_ids: &[String]) -> anyhow::Result<()> {
            self.events.send(Self::depth(100, 3100.0))?;
            self.events.send(Self::depth(105, 3100.2))?;
            Ok(())
        }

        fn unsubscribe(&mut self, _instrument_ids: &[String]) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn config() -> CtpDataClientConfig {
        CtpDataClientConfig {
            broker_id: "9999".to_string(),
            user_id: "123456".to_string(),
            password: SecretString::from("secret"),
            front_address: "tcp://127.0.0.1:10131".to_string(),
            flow_path: String::new(),
            production_mode: false,
            login_timeout_secs: 2,
            instruments: vec![CtpInstrumentDefinition {
                ctp_symbol: "rb2510".to_string(),
                exchange: "SHFE".to_string(),
                underlying: "rb".to_string(),
                asset_class: AssetClass::Commodity,
                currency: "CNY".to_string(),
                expiry_date: "20251015".to_string(),
                price_increment: Decimal::new(1, 1),
                multiplier: Decimal::new(10, 0),
            }],
        }
    }

    fn client(
        reject_login: bool,
    ) -> (
        CtpDataClient<ScriptedBackend>,
        tokio::sync::mpsc::UnboundedReceiver<DataEvent>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let factory: BackendFactory<ScriptedBackend> = Box::new(move |_, events| {
            Ok(ScriptedBackend {
                events,
                reject_login,
            })
        });
        let client = CtpDataClient::with_backend(
            ClientId::from("CTP"),
            config(),
            EventSender::from(tx),
            factory,
        )
        .unwrap();
        (client, rx)
    }

    async fn next_data(rx: &mut tokio::sync::mpsc::UnboundedReceiver<DataEvent>) -> Data {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .expect("timed out waiting for data event")
                .expect("data channel closed");
            if let DataEvent::Data(data) = event {
                return data;
            }
        }
    }

    #[tokio::test]
    async fn test_connect_publishes_instruments_and_streams_quotes() {
        let (mut client, mut rx) = client(false);
        client.connect().await.unwrap();
        assert!(client.is_connected());

        assert!(matches!(next_data(&mut rx).await, Data::Instrument(_)));

        let id = InstrumentId::from("rb2510.SHFE");
        client
            .subscribe_quotes(SubscribeQuotes::new(
                vec![id],
                Some(ClientId::from("CTP")),
                None,
                UUID4::new(),
                UnixNanos::default(),
                None,
                None,
            ))
            .unwrap();

        let Data::Quote(quote) = next_data(&mut rx).await else {
            panic!("expected a quote tick");
        };
        assert_eq!(quote.bid_price.to_string(), "3100.0");
        assert_eq!(quote.ask_price.to_string(), "3100.2");

        client.disconnect().await.unwrap();
        assert!(!client.is_connected());
    }

    #[tokio::test]
    async fn test_login_rejection_fails_connect_and_leaves_client_disconnected() {
        let (mut client, _rx) = client(true);
        let err = client.connect().await.unwrap_err();

        assert!(err.to_string().contains("login rejected"), "{err}");
        assert!(!client.is_connected());
    }

    #[tokio::test]
    async fn test_subscribe_before_connect_is_rejected() {
        let (mut client, _rx) = client(false);
        let err = client
            .subscribe_quotes(SubscribeQuotes::new(
                vec![InstrumentId::from("rb2510.SHFE")],
                Some(ClientId::from("CTP")),
                None,
                UUID4::new(),
                UnixNanos::default(),
                None,
                None,
            ))
            .unwrap_err();

        assert!(err.to_string().contains("not connected"));
    }

    #[cfg(not(feature = "ctp-native"))]
    #[tokio::test]
    async fn test_default_build_reports_native_feature_on_connect() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut client =
            CtpDataClient::new(ClientId::from("CTP"), config(), EventSender::from(tx)).unwrap();
        let err = client.connect().await.unwrap_err();

        assert!(err.to_string().contains("ctp-native"), "{err}");
    }
}
