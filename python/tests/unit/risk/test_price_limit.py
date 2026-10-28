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
Test daily price limits for A-share limit orders.
"""

import pytest

from nautilus_trader.backtest import BacktestEngine
from nautilus_trader.config import BacktestEngineConfig
from nautilus_trader.config import StrategyConfig
from nautilus_trader.execution.ashare import SSE
from nautilus_trader.execution.ashare import AShareFeeModel
from nautilus_trader.execution.ashare import ashare_equity
from nautilus_trader.model import AccountType
from nautilus_trader.model import Bar
from nautilus_trader.model import BarType
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
CHINA_OFFSET_NS = 8 * NANOS_PER_HOUR
DAY_0 = 20_000
DAILY_BAR_TYPE = "600519.SSE-1-DAY-LAST-EXTERNAL"


def _china_time(day: int, hour: int) -> int:
    return day * NANOS_PER_DAY - CHINA_OFFSET_NS + hour * NANOS_PER_HOUR


class _LimitOnceConfig(StrategyConfig):
    def __init__(
        self,
        *,
        instrument_id: InstrumentId,
        price: str,
        **_kwargs: object,
    ) -> None:
        super().__init__()
        self.instrument_id = instrument_id
        self.price = price


class _LimitOnce(Strategy):
    def __init__(self, config: _LimitOnceConfig) -> None:
        super().__init__(config)
        self.order = None

    def on_start(self) -> None:
        self.subscribe_quotes(self.config.instrument_id)

    def on_quote(self, _tick: QuoteTick) -> None:
        if self.order is None:
            self.order = self.order_factory.limit(
                self.config.instrument_id,
                OrderSide.BUY,
                Quantity.from_int(100),
                Price.from_str(self.config.price),
            )
            self.submit_order(self.order)


def _run(price: str, with_reference: bool) -> OrderStatus:
    instrument = ashare_equity("600519")
    engine = BacktestEngine(
        BacktestEngineConfig(
            trader_id=TraderId("TEST-001"),
            risk_engine=RiskEngineConfig(price_limit_venues=[SSE]),
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

    if with_reference:
        # Previous trading day close of 1000.00, stamped during day 0.
        engine.add_data(
            [
                Bar(
                    bar_type=BarType.from_str(DAILY_BAR_TYPE),
                    open=Price.from_str("1000.00"),
                    high=Price.from_str("1000.00"),
                    low=Price.from_str("1000.00"),
                    close=Price.from_str("1000.00"),
                    volume=Quantity.from_int(1_000),
                    ts_event=_china_time(DAY_0, 15),
                    ts_init=_china_time(DAY_0, 15),
                ),
            ],
        )

    quote_ts = _china_time(DAY_0 + 1, 10)
    engine.add_data(
        [
            QuoteTick(
                instrument_id=instrument.id,
                bid_price=Price.from_str("999.99"),
                ask_price=Price.from_str("1000.00"),
                bid_size=Quantity.from_int(10_000),
                ask_size=Quantity.from_int(10_000),
                ts_event=quote_ts,
                ts_init=quote_ts,
            ),
        ],
    )

    strategy = _LimitOnce(_LimitOnceConfig(instrument_id=instrument.id, price=price))
    engine.add_strategy(strategy)
    engine.run()

    status = engine.cache.order(strategy.order.client_order_id).status
    engine.dispose()
    return status


@pytest.mark.parametrize(
    ("price", "with_reference", "expected"),
    [
        ("1150.00", True, OrderStatus.DENIED),
        ("1050.00", True, OrderStatus.FILLED),
        ("1050.00", False, OrderStatus.DENIED),
    ],
)
def test_limit_order_must_be_within_daily_price_limits(
    price: str,
    with_reference: bool,
    expected: OrderStatus,
) -> None:
    """
    Test a limit above the 10% upper limit is denied, and a missing reference close denies.
    """
    assert _run(price, with_reference) == expected
