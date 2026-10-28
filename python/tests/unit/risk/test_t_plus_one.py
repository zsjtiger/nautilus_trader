# -------------------------------------------------------------------------------------------------
#  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved.
#  https://nautechsystems.io
#
#  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
#  You may not use this file except in compliance with the License.
#  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
#
#  Unless required by applicable law or agreed to in writing, software
#  distributed under the License is distributed on an "AS IS" BASIS,
#  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
#  See the License for the specific language governing permissions and
#  limitations under the License.
# -------------------------------------------------------------------------------------------------
"""
Test T+1 settlement for shares bought earlier on the same trading day.
"""

import pytest

from nautilus_trader.backtest import BacktestEngine
from nautilus_trader.config import BacktestEngineConfig
from nautilus_trader.config import StrategyConfig
from nautilus_trader.execution.ashare import SSE
from nautilus_trader.execution.ashare import AShareFeeModel
from nautilus_trader.execution.ashare import ashare_equity
from nautilus_trader.model import AccountType
from nautilus_trader.model import Currency
from nautilus_trader.model import InstrumentId
from nautilus_trader.model import Money
from nautilus_trader.model import OmsType
from nautilus_trader.model import OrderSide
from nautilus_trader.model import OrderStatus
from nautilus_trader.model import Price
from nautilus_trader.model import Quantity
from nautilus_trader.model import QuoteTick
from nautilus_trader.model import TraderId
from nautilus_trader.risk import RiskEngineConfig
from nautilus_trader.trading import Strategy


CNY = Currency.from_str("CNY")
NANOS_PER_HOUR = 3_600 * 1_000_000_000
NANOS_PER_DAY = 24 * NANOS_PER_HOUR
# China Standard Time is UTC+8, so midnight of trading day `d` is `d * DAY - 8h` in UTC.
CHINA_OFFSET_NS = 8 * NANOS_PER_HOUR
DAY_0 = 20_000


def _china_time(day: int, hour: int) -> int:
    return day * NANOS_PER_DAY - CHINA_OFFSET_NS + hour * NANOS_PER_HOUR


class _BuyThenSellConfig(StrategyConfig):
    def __init__(
        self,
        *,
        instrument_id: InstrumentId,
        quantity: int,
        sell_at_ns: int,
        **_kwargs: object,
    ) -> None:
        super().__init__()
        self.instrument_id = instrument_id
        self.quantity = quantity
        self.sell_at_ns = sell_at_ns


class _BuyThenSell(Strategy):
    def __init__(self, config: _BuyThenSellConfig) -> None:
        super().__init__(config)
        self.buy = None
        self.sell = None

    def on_start(self) -> None:
        self.subscribe_quotes(self.config.instrument_id)

    def on_quote(self, tick: QuoteTick) -> None:
        if self.buy is None:
            self.buy = self.order_factory.market(
                self.config.instrument_id,
                OrderSide.BUY,
                Quantity.from_int(self.config.quantity),
            )
            self.submit_order(self.buy)
        elif self.sell is None and tick.ts_event >= self.config.sell_at_ns:
            self.sell = self.order_factory.market(
                self.config.instrument_id,
                OrderSide.SELL,
                Quantity.from_int(self.config.quantity),
            )
            self.submit_order(self.sell)


def _run(sell_day: int, t_plus_one_venues: list) -> OrderStatus:
    instrument = ashare_equity("600519")
    buy_at = _china_time(DAY_0, 10)
    sell_at = _china_time(sell_day, 11)

    engine = BacktestEngine(
        BacktestEngineConfig(
            trader_id=TraderId("TEST-001"),
            risk_engine=RiskEngineConfig(t_plus_one_venues=t_plus_one_venues),
        ),
    )
    engine.add_venue(
        venue=SSE,
        oms_type=OmsType.NETTING,
        account_type=AccountType.CASH,
        base_currency=CNY,
        starting_balances=[Money(1_000_000, CNY)],
        fee_model=AShareFeeModel(),
    )
    engine.add_instrument(instrument)

    quote_times = [buy_at, buy_at + NANOS_PER_HOUR, sell_at]
    engine.add_data(
        [
            QuoteTick(
                instrument_id=instrument.id,
                bid_price=Price.from_str("999.99"),
                ask_price=Price.from_str("1000.00"),
                bid_size=Quantity.from_int(10_000),
                ask_size=Quantity.from_int(10_000),
                ts_event=ts,
                ts_init=ts,
            )
            for ts in quote_times
        ],
    )

    strategy = _BuyThenSell(
        _BuyThenSellConfig(
            instrument_id=instrument.id,
            quantity=100,
            sell_at_ns=sell_at,
        ),
    )
    engine.add_strategy(strategy)
    engine.run()

    status = engine.cache.order(strategy.sell.client_order_id).status
    engine.dispose()
    return status


@pytest.mark.parametrize(
    ("sell_day_offset", "t_plus_one_venues", "expected"),
    [
        (0, [SSE], OrderStatus.DENIED),
        (1, [SSE], OrderStatus.FILLED),
        (0, [], OrderStatus.FILLED),
    ],
)
def test_sell_of_shares_bought_today_is_denied_only_on_t_plus_one_venues(
    sell_day_offset: int,
    t_plus_one_venues: list,
    expected: OrderStatus,
) -> None:
    """
    Test a same-day sell is denied under T+1 and allowed the next trading day.
    """
    assert _run(DAY_0 + sell_day_offset, t_plus_one_venues) == expected
