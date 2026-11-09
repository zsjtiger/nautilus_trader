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
Live node end-to-end test for A-share orders through the sandbox venue.

A Python market data client publishes a daily bar and a steady quote stream for 600519.SSE. A
strategy submits one limit order. The sandbox execution client matches it, and the live risk engine
applies the A-share checks. The test asserts on the order events the strategy receives.
"""

import asyncio
import time
from typing import ClassVar

import pytest

from nautilus_trader.adapters.sandbox import SandboxExecutionClientConfig
from nautilus_trader.adapters.sandbox import SandboxExecutionClientFactory
from nautilus_trader.common import Environment
from nautilus_trader.config import DataClientConfig
from nautilus_trader.config import StrategyConfig
from nautilus_trader.execution.ashare import SSE
from nautilus_trader.execution.ashare import AShareFeeModel
from nautilus_trader.execution.ashare import ashare_equity
from nautilus_trader.live import LiveNode
from nautilus_trader.live import LiveRiskEngineConfig
from nautilus_trader.live.clients import DataClientFactory
from nautilus_trader.live.clients import MarketDataClient
from nautilus_trader.model import AccountType
from nautilus_trader.model import Bar
from nautilus_trader.model import BarType
from nautilus_trader.model import Currency
from nautilus_trader.model import InstrumentId
from nautilus_trader.model import Money
from nautilus_trader.model import OmsType
from nautilus_trader.model import OrderSide
from nautilus_trader.model import Price
from nautilus_trader.model import Quantity
from nautilus_trader.model import QuoteTick
from nautilus_trader.model import TraderId
from nautilus_trader.trading import Strategy


CNY = Currency.from_str("CNY")
INSTRUMENT_ID = InstrumentId.from_str("600519.SSE")
NANOS_PER_DAY = 86_400 * 1_000_000_000
NANOS_PER_HOUR = 3_600 * 1_000_000_000
CHINA_OFFSET_NS = 8 * NANOS_PER_HOUR
DAILY_BAR_TYPE = BarType.from_str("600519.SSE-1-DAY-LAST-EXTERNAL")


def _previous_trading_day_close_ts() -> int:
    """
    Return a timestamp at 15:00 China time on the previous calendar day.
    """
    now_ns = time.time_ns()
    today = (now_ns + CHINA_OFFSET_NS) // NANOS_PER_DAY
    return (today - 1) * NANOS_PER_DAY - CHINA_OFFSET_NS + 15 * NANOS_PER_HOUR


class FeedConfig(DataClientConfig):
    """
    Configure the feed with the previous close used for the daily bar.
    """

    def __init__(self, *, prev_close: str, quote_price: str, **_kwargs: object) -> None:
        """
        Initialize the configuration.
        """
        super().__init__()
        self.prev_close = prev_close
        self.quote_price = quote_price


class FeedClient(MarketDataClient):
    """
    Publish one instrument, a previous-day close bar, and a steady quote stream.
    """

    async def _connect(self) -> None:
        self._handle_instrument(ashare_equity("600519"))
        ts = _previous_trading_day_close_ts()
        close = Price.from_str(self.feed_config.prev_close)
        self._handle_data(
            Bar(
                bar_type=DAILY_BAR_TYPE,
                open=close,
                high=close,
                low=close,
                close=close,
                volume=Quantity.from_int(1_000),
                ts_event=ts,
                ts_init=ts,
            ),
        )

    async def _disconnect(self) -> None:
        if self._quote_task is not None:
            self._quote_task.cancel()
            self._quote_task = None

    async def _subscribe_quotes(self, _command) -> None:
        if self._quote_task is None:
            self._quote_task = asyncio.get_running_loop().create_task(self._publish_quotes())

    async def _publish_quotes(self) -> None:
        price = Price.from_str(self.feed_config.quote_price)
        while True:
            ts = time.time_ns()
            self._handle_data(
                QuoteTick(
                    instrument_id=INSTRUMENT_ID,
                    bid_price=price,
                    ask_price=price,
                    bid_size=Quantity.from_int(10_000),
                    ask_size=Quantity.from_int(10_000),
                    ts_event=ts,
                    ts_init=ts,
                ),
            )
            await asyncio.sleep(0.05)


class FeedFactory(DataClientFactory):
    """
    Construct the feed client for the node.
    """

    @staticmethod
    def create(*, name, config, cache, clock) -> object:
        """
        Construct the feed client for the node.
        """
        client = FeedClient(
            name=name,
            config=config,
            cache=cache,
            clock=clock,
            venue=SSE,
        )
        client.feed_config = config
        client._quote_task = None
        return client


class LimitOrderConfig(StrategyConfig):
    """
    Configure the single limit order to submit once a quote arrives.
    """

    def __init__(self, *, quantity: int, limit_price: str, **_kwargs: object) -> None:
        """
        Initialize the configuration.
        """
        super().__init__()
        self.quantity = quantity
        self.limit_price = limit_price


class LimitOrderStrategy(Strategy):
    """
    Submit one limit buy and record whether the order was denied or filled.
    """

    results: ClassVar[dict] = {}

    def __init__(self, config: LimitOrderConfig) -> None:
        """
        Initialize the configuration.
        """
        super().__init__(config)
        self._submitted = False

    def on_start(self) -> None:
        """
        Subscribe to the instrument quotes.
        """
        self.subscribe_quotes(INSTRUMENT_ID)

    def on_quote(self, _tick: QuoteTick) -> None:
        """
        Submit one limit order on the first quote.
        """
        if self._submitted:
            return
        self._submitted = True
        order = self.order_factory.limit(
            INSTRUMENT_ID,
            OrderSide.BUY,
            Quantity.from_int(self.config.quantity),
            Price.from_str(self.config.limit_price),
        )
        self.submit_order(order)

    def on_order_denied(self, event) -> None:
        """
        Record that the order was denied.
        """
        LimitOrderStrategy.results.update(status="denied", reason=str(event.reason))

    def on_order_filled(self, _event) -> None:
        """
        Record that the order was filled.
        """
        LimitOrderStrategy.results["status"] = "filled"


async def _run_node(
    *,
    quantity: int,
    limit_price: str,
    prev_close: str,
    quote_price: str,
    risk: LiveRiskEngineConfig,
) -> tuple[str, str]:
    LimitOrderStrategy.results = {}
    builder = (
        LiveNode.builder("ASHARE", TraderId("ASHARE-001"), Environment.SANDBOX)
        .with_reconciliation(False)
        .with_risk_engine_config(risk)
        .add_data_client(
            "FEED",
            FeedFactory,
            FeedConfig(prev_close=prev_close, quote_price=quote_price),
        )
        .add_simulated_exec_client(
            "SSE_SANDBOX",
            SandboxExecutionClientFactory(),
            SandboxExecutionClientConfig(
                venue=SSE,
                starting_balances=[Money(1_000_000, CNY)],
                base_currency=CNY,
                oms_type=OmsType.NETTING,
                account_type=AccountType.CASH,
                fee_model=AShareFeeModel(),
            ),
        )
    )
    node = builder.build()
    node.add_strategy(
        LimitOrderStrategy(LimitOrderConfig(quantity=quantity, limit_price=limit_price))
    )
    handle = node.handle()

    try:
        task = asyncio.create_task(node.run_async())
        async with asyncio.timeout(20.0):
            while "status" not in LimitOrderStrategy.results:
                if task.done():
                    await task
                    break
                await asyncio.sleep(0.05)

        handle.stop()
        async with asyncio.timeout(20.0):
            await task
    finally:
        node.dispose()

    results = LimitOrderStrategy.results
    return results.get("status", "none"), results.get("reason", "")


@pytest.mark.asyncio
async def test_limit_above_daily_upper_limit_is_denied_on_live_node() -> None:
    """
    Test a limit above the 10% upper limit (1100 on a 1000 close) is denied by the live risk engine.
    """
    status, reason = await _run_node(
        quantity=100,
        limit_price="1150.00",
        prev_close="1000.00",
        quote_price="1000.00",
        risk=LiveRiskEngineConfig(price_limit_venues=[SSE]),
    )

    assert status == "denied"
    assert reason.startswith("PRICE_OUTSIDE_LIMIT")


@pytest.mark.asyncio
async def test_limit_within_daily_limits_is_filled_on_live_node() -> None:
    """
    Test a limit inside the daily limits fills on the sandbox venue.
    """
    status, _reason = await _run_node(
        quantity=100,
        limit_price="1050.00",
        prev_close="1000.00",
        quote_price="1000.00",
        risk=LiveRiskEngineConfig(price_limit_venues=[SSE]),
    )

    assert status == "filled"


@pytest.mark.asyncio
async def test_odd_lot_is_denied_on_live_node_when_lot_size_enforced() -> None:
    """
    Test a 150-share order is denied on the live node when lots are enforced on SSE.
    """
    status, reason = await _run_node(
        quantity=150,
        limit_price="1000.00",
        prev_close="1000.00",
        quote_price="1000.00",
        risk=LiveRiskEngineConfig(lot_size_enforced_venues=[SSE]),
    )

    assert status == "denied"
    assert reason.startswith("QUANTITY_NOT_LOT_MULTIPLE")
