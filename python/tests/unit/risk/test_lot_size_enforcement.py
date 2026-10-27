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
Test whole-lot enforcement for venues listed in the risk engine configuration.
"""

import pytest

from nautilus_trader.backtest import BacktestEngine
from nautilus_trader.config import BacktestEngineConfig
from nautilus_trader.execution.ashare import SSE
from nautilus_trader.execution.ashare import ashare_equity
from nautilus_trader.model import AccountType
from nautilus_trader.model import Currency
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


class _BuyOnce(Strategy):
    def __init__(self, instrument_id, quantity: int) -> None:
        super().__init__()
        self._instrument_id = instrument_id
        self._quantity = quantity
        self.order = None

    def on_start(self) -> None:
        self.subscribe_quotes(self._instrument_id)

    def on_quote_tick(self, _tick: QuoteTick) -> None:
        if self.order is None:
            self.order = self.order_factory.market(
                self._instrument_id,
                OrderSide.BUY,
                Quantity.from_int(self._quantity),
            )
            self.submit_order(self.order)


def _run_buy(quantity: int, lot_size_enforced_venues: list) -> OrderStatus:
    instrument = ashare_equity("600519")
    engine = BacktestEngine(
        BacktestEngineConfig(
            trader_id=TraderId("TEST-001"),
            risk_engine=RiskEngineConfig(lot_size_enforced_venues=lot_size_enforced_venues),
        ),
    )
    engine.add_venue(
        venue=SSE,
        oms_type=OmsType.NETTING,
        account_type=AccountType.CASH,
        base_currency=CNY,
        starting_balances=[Money(1_000_000, CNY)],
    )
    engine.add_instrument(instrument)
    engine.add_data(
        [
            QuoteTick(
                instrument_id=instrument.id,
                bid_price=Price.from_str("999.99"),
                ask_price=Price.from_str("1000.00"),
                bid_size=Quantity.from_int(10_000),
                ask_size=Quantity.from_int(10_000),
                ts_event=i,
                ts_init=i,
            )
            for i in range(1, 4)
        ],
    )

    strategy = _BuyOnce(instrument.id, quantity)
    engine.add_strategy(strategy)
    engine.run()

    status = strategy.order.status
    engine.dispose()
    return status


@pytest.mark.parametrize(
    ("quantity", "lot_size_enforced_venues", "expected"),
    [
        (150, [SSE], OrderStatus.DENIED),
        (200, [SSE], OrderStatus.FILLED),
        (150, [], OrderStatus.FILLED),
    ],
)
def test_whole_lot_is_enforced_only_for_configured_venues(
    quantity: int,
    lot_size_enforced_venues: list,
    expected: OrderStatus,
) -> None:
    """
    Test a 150-share buy is denied only where the venue enforces lot sizes.
    """
    assert _run_buy(quantity, lot_size_enforced_venues) == expected
