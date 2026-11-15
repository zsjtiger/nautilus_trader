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

//! Example: China A-share round trip with the T+1 and A-share commission rules.
//!
//! A strategy buys 100 shares of 600519.SSE on day one, then tries to sell them the same day.
//! The risk engine denies that sell under T+1 settlement. On day two the sell succeeds. The
//! sandbox venue charges A-share commission, stamp duty, and transfer fees.
//!
//! Run with: `cargo run -p nautilus-backtest --features examples --example engine-ashare`

#[cfg(feature = "mimalloc")]
mod allocator;

use ahash::AHashSet;
use nautilus_backtest::{
    config::{BacktestEngineConfig, SimulatedVenueConfig},
    engine::BacktestEngine,
};
use nautilus_common::actor::DataActor;
use nautilus_core::UnixNanos;
use nautilus_execution::models::fee::{AShareFeeModel, FeeModelAny};
use nautilus_model::{
    data::{Data, QuoteTick},
    enums::{AccountType, BookType, OmsType, OrderSide},
    events::{OrderDenied, OrderFilled},
    identifiers::{InstrumentId, StrategyId, Symbol, Venue},
    instruments::{Equity, InstrumentAny},
    types::{Currency, Money, Price, Quantity},
};
use nautilus_risk::engine::config::RiskEngineConfig;
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};

const VENUE: &str = "SSE";
const STARTING_BALANCE: &str = "1_000_000 CNY";
const TRADE_SIZE: &str = "100";

/// 2025-01-06 00:00 China Standard Time (UTC+8), as UNIX nanoseconds.
const DAY_ONE_START_NS: u64 = 1_736_092_800_000_000_000;
const NANOS_PER_MINUTE: u64 = 60_000_000_000;
const NANOS_PER_DAY: u64 = 86_400_000_000_000;

/// Buys on the first quote, attempts a same-day sell on the second, and sells on the fourth.
pub struct AShareRoundTrip {
    core: StrategyCore,
    instrument_id: InstrumentId,
    trade_size: Quantity,
    quotes_seen: usize,
}

impl AShareRoundTrip {
    pub fn new(instrument_id: InstrumentId) -> Self {
        let config = StrategyConfig {
            strategy_id: Some(StrategyId::from("ASHARE_ROUND_TRIP-001")),
            order_id_tag: Some("001".to_string()),
            ..Default::default()
        };
        Self {
            core: StrategyCore::new(config),
            instrument_id,
            trade_size: Quantity::from(TRADE_SIZE),
            quotes_seen: 0,
        }
    }
}

impl std::fmt::Debug for AShareRoundTrip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AShareRoundTrip")
            .field("instrument_id", &self.instrument_id)
            .field("quotes_seen", &self.quotes_seen)
            .finish()
    }
}

nautilus_strategy!(AShareRoundTrip, {
    fn on_order_denied(&mut self, event: OrderDenied) {
        println!("DENIED {}: {}", event.client_order_id, event.reason);
    }

    fn on_order_filled(&mut self, event: &OrderFilled) {
        println!(
            "FILLED {} {} {} @ {}",
            event.order_side, event.last_qty, event.instrument_id, event.last_px
        );
    }
});

impl DataActor for AShareRoundTrip {
    fn on_start(&mut self) -> anyhow::Result<()> {
        self.subscribe_quotes(self.instrument_id, None, None);
        Ok(())
    }

    fn on_quote(&mut self, _quote: &QuoteTick) -> anyhow::Result<()> {
        self.quotes_seen += 1;

        let side = match self.quotes_seen {
            1 => OrderSide::Buy,
            2 | 4 => OrderSide::Sell,
            _ => return Ok(()),
        };

        let order = self.order().market(
            self.instrument_id,
            side,
            self.trade_size,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        self.submit_order(order, None, None, None)?;
        Ok(())
    }
}

fn a_share_equity(instrument_id: InstrumentId) -> InstrumentAny {
    InstrumentAny::Equity(
        Equity::builder()
            .instrument_id(instrument_id)
            .raw_symbol(Symbol::from("600519"))
            .currency(Currency::from("CNY"))
            .price_precision(2)
            .price_increment(Price::from("0.01"))
            .lot_size(Quantity::from("100"))
            .ts_event(UnixNanos::default())
            .ts_init(UnixNanos::default())
            .build()
            .expect("valid A-share equity"),
    )
}

fn quote(instrument_id: InstrumentId, ask: &str, ts: u64) -> Data {
    Data::Quote(QuoteTick::new(
        instrument_id,
        Price::from("9.99"),
        Price::from(ask),
        Quantity::from("10000"),
        Quantity::from("10000"),
        ts.into(),
        ts.into(),
    ))
}

fn main() -> anyhow::Result<()> {
    #[cfg(feature = "mimalloc")]
    allocator::register();

    let venue = Venue::from(VENUE);
    let risk = RiskEngineConfig::builder()
        .lot_size_enforced_venues([venue].into_iter().collect::<AHashSet<_>>())
        .t_plus_one_venues([venue].into_iter().collect::<AHashSet<_>>())
        .build()?;

    let mut engine = BacktestEngine::new(BacktestEngineConfig {
        risk_engine: Some(risk),
        ..Default::default()
    })?;

    engine.add_venue(
        SimulatedVenueConfig::builder()
            .venue(venue)
            .oms_type(OmsType::Netting)
            .account_type(AccountType::Cash)
            .book_type(BookType::L1_MBP)
            .starting_balances(vec![Money::from(STARTING_BALANCE)])
            .base_currency(Currency::from("CNY"))
            .fee_model(FeeModelAny::AShare(AShareFeeModel::default()).into())
            .build()?,
    )?;

    let instrument_id = InstrumentId::from("600519.SSE");
    let instrument = a_share_equity(instrument_id);
    engine.add_instrument(&instrument)?;

    engine.add_strategy(AShareRoundTrip::new(instrument_id))?;

    // Day one: buy at 10:00, attempt a sell at 10:01 (denied by T+1), then hold.
    // Day two: sell at 10:00 (allowed).
    let day_two_start_ns = DAY_ONE_START_NS + NANOS_PER_DAY;
    let quotes = vec![
        quote(
            instrument_id,
            "10.00",
            DAY_ONE_START_NS + 10 * 60 * NANOS_PER_MINUTE,
        ),
        quote(
            instrument_id,
            "10.00",
            DAY_ONE_START_NS + 10 * 60 * NANOS_PER_MINUTE + NANOS_PER_MINUTE,
        ),
        quote(
            instrument_id,
            "10.00",
            DAY_ONE_START_NS + 10 * 60 * NANOS_PER_MINUTE + 2 * NANOS_PER_MINUTE,
        ),
        quote(
            instrument_id,
            "10.00",
            day_two_start_ns + 10 * 60 * NANOS_PER_MINUTE,
        ),
    ];
    engine.add_data(quotes, None, true, true)?;

    engine.run(None, None, None, false)?;
    Ok(())
}
