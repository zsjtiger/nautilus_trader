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
Test the China A-share commission model.
"""

from decimal import Decimal
from types import SimpleNamespace

import pytest

from nautilus_trader.execution import FeeModel
from nautilus_trader.execution.ashare import AShareFeeModel
from nautilus_trader.model import InstrumentId
from nautilus_trader.model import Money
from nautilus_trader.model import OrderSide
from nautilus_trader.model import Price
from nautilus_trader.model import Quantity


SSE_ID = InstrumentId.from_str("600519.SSE")
SZSE_ID = InstrumentId.from_str("000001.SZSE")


def _order(side: OrderSide) -> SimpleNamespace:
    return SimpleNamespace(side=side)


def _instrument(instrument_id: InstrumentId) -> SimpleNamespace:
    return SimpleNamespace(id=instrument_id)


def _commission(
    model: AShareFeeModel,
    side: OrderSide,
    quantity: int,
    price: str,
    instrument_id: InstrumentId,
) -> Decimal:
    commission = model.get_commission(
        _order(side),
        Quantity.from_int(quantity),
        Price.from_str(price),
        _instrument(instrument_id),
    )
    assert isinstance(commission, Money)
    assert str(commission.currency) == "CNY"
    return commission.as_decimal()


def test_is_fee_model() -> None:
    """
    Test the model plugs into the engine as a FeeModel.
    """
    model = AShareFeeModel()

    assert isinstance(model, FeeModel)


def test_buy_above_minimum_charges_commission_and_transfer_fee() -> None:
    """
    Test a SSE buy above the minimum: 100 * 1000.00 = 100000 notional.

    Commission 25.00 (0.025%), transfer fee 1.00 (0.001%), no stamp duty.
    """
    model = AShareFeeModel()

    total = _commission(model, OrderSide.BUY, 100, "1000.00", SSE_ID)

    assert total == Decimal("26.00")


def test_buy_below_minimum_charges_minimum_commission() -> None:
    """
    Test a small SSE buy where 0.025% is below the 5.00 CNY minimum.

    Notional 1000; commission floored to 5.00, transfer fee 0.01.
    """
    model = AShareFeeModel()

    total = _commission(model, OrderSide.BUY, 100, "10.00", SSE_ID)

    assert total == Decimal("5.01")


def test_sell_on_sse_adds_stamp_duty_and_transfer_fee() -> None:
    """
    Test a SSE sell: commission 25.00, stamp duty 50.00, transfer fee 1.00.
    """
    model = AShareFeeModel()

    total = _commission(model, OrderSide.SELL, 100, "1000.00", SSE_ID)

    assert total == Decimal("76.00")


def test_sell_on_szse_has_no_transfer_fee() -> None:
    """
    Test a SZSE sell: commission floored to 5.00, stamp duty 0.50, no transfer fee.
    """
    model = AShareFeeModel()

    total = _commission(model, OrderSide.SELL, 100, "10.00", SZSE_ID)

    assert total == Decimal("5.50")


def test_buy_on_szse_has_no_stamp_duty_or_transfer_fee() -> None:
    """
    Test a SZSE buy above the minimum: commission only (100000 * 0.025% = 25.00).
    """
    model = AShareFeeModel()

    total = _commission(model, OrderSide.BUY, 100, "1000.00", SZSE_ID)

    assert total == Decimal("25.00")


def test_components_round_half_up_to_fen() -> None:
    """
    Test a component that lands exactly on half a fen rounds up, not to even.

    Notional 1.00 * 0.005 = 0.005 rounds to 0.01 under half-up rounding.
    """
    model = AShareFeeModel(
        commission_rate=Decimal("0.005"),
        min_commission=Decimal(0),
        transfer_fee_rate=Decimal(0),
    )

    total = _commission(model, OrderSide.BUY, 1, "1.00", SZSE_ID)

    assert total == Decimal("0.01")


def test_zero_rates_charge_nothing_beyond_minimum() -> None:
    """
    Test zero rates with a zero minimum charge nothing.
    """
    model = AShareFeeModel(
        commission_rate=Decimal(0),
        min_commission=Decimal(0),
        stamp_duty_rate=Decimal(0),
        transfer_fee_rate=Decimal(0),
    )

    assert _commission(model, OrderSide.SELL, 100, "1000.00", SSE_ID) == Decimal(0)


def test_transfer_fee_venue_is_configurable() -> None:
    """
    Test the transfer fee applies to the configured venue instead of SSE.
    """
    model = AShareFeeModel(transfer_fee_venue="SZSE")

    assert _commission(model, OrderSide.BUY, 100, "1000.00", SZSE_ID) == Decimal("26.00")
    assert _commission(model, OrderSide.BUY, 100, "1000.00", SSE_ID) == Decimal("25.00")


def test_get_commission_with_context_uses_get_commission() -> None:
    """
    Test the default context hook delegates to get_commission.
    """
    model = AShareFeeModel()

    total = model.get_commission_with_context(
        _order(OrderSide.BUY),
        Quantity.from_int(100),
        Price.from_str("1000.00"),
        _instrument(SSE_ID),
    )

    assert total.as_decimal() == Decimal("26.00")


@pytest.mark.parametrize(
    "kwargs",
    [
        {"commission_rate": Decimal("-0.001")},
        {"min_commission": Decimal(-1)},
        {"stamp_duty_rate": Decimal("-0.0005")},
        {"transfer_fee_rate": Decimal("-0.00001")},
    ],
)
def test_negative_parameters_are_rejected(kwargs: dict[str, Decimal]) -> None:
    """
    Test every rate and the minimum must be non-negative.
    """
    with pytest.raises(ValueError, match="must be non-negative"):
        AShareFeeModel(**kwargs)
