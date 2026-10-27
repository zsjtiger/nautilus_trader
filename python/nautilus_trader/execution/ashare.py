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
Support for China A-share equities: exchange venues, instruments, and the commission model.
"""

from __future__ import annotations

from decimal import ROUND_HALF_UP
from decimal import Decimal
from typing import Any

from nautilus_trader.execution import FeeModel
from nautilus_trader.model import Currency
from nautilus_trader.model import Equity
from nautilus_trader.model import InstrumentId
from nautilus_trader.model import Money
from nautilus_trader.model import OrderSide
from nautilus_trader.model import Price
from nautilus_trader.model import Quantity
from nautilus_trader.model import Symbol
from nautilus_trader.model import Venue


SSE = Venue("SSE")
SZSE = Venue("SZSE")

A_SHARE_LOT_SIZE = Quantity.from_int(100)
A_SHARE_PRICE_INCREMENT = Price.from_str("0.01")

_CODE_LENGTH = 6
_FEN = Decimal("0.01")
_ZERO = Decimal(0)


class AShareFeeModel(FeeModel):
    """
    Commission model for China A-share equity fills, denominated in CNY.

    Each fill charges, rounded to the fen (0.01 CNY) per component:

    - **Commission**: ``fill notional * commission_rate``, with a per-fill floor of
      ``min_commission``.
    - **Stamp duty**: ``fill notional * stamp_duty_rate``, charged on sells only.
    - **Transfer fee**: ``fill notional * transfer_fee_rate``, charged only when the instrument
      venue equals ``transfer_fee_venue``.

    The defaults reflect the rules in force when this model was written. Rates and exemptions
    change, so verify them against the current broker and exchange schedules before relying on
    backtest results.

    Parameters
    ----------
    commission_rate : Decimal, default Decimal("0.00025")
        Brokerage commission rate applied to both sides.
    min_commission : Decimal, default Decimal("5")
        Minimum commission per fill, in CNY.
    stamp_duty_rate : Decimal, default Decimal("0.0005")
        Stamp duty rate applied to sell fills.
    transfer_fee_rate : Decimal, default Decimal("0.00001")
        Transfer fee rate applied to fills on ``transfer_fee_venue``.
    transfer_fee_venue : str, default "SSE"
        Venue code that incurs the transfer fee. Shenzhen (``SZSE``) does not charge it.

    Raises
    ------
    ValueError
        If any rate or the minimum commission is negative.

    """

    def __init__(
        self,
        commission_rate: Decimal = Decimal("0.00025"),
        min_commission: Decimal = Decimal(5),
        stamp_duty_rate: Decimal = Decimal("0.0005"),
        transfer_fee_rate: Decimal = Decimal("0.00001"),
        transfer_fee_venue: str = "SSE",
    ) -> None:
        """
        Create the A-share commission model with the given rates and minimum.
        """
        for name, value in (
            ("commission_rate", commission_rate),
            ("min_commission", min_commission),
            ("stamp_duty_rate", stamp_duty_rate),
            ("transfer_fee_rate", transfer_fee_rate),
        ):
            if value < _ZERO:
                raise ValueError(f"{name} must be non-negative, was {value}")

        self._commission_rate = commission_rate
        self._min_commission = min_commission
        self._stamp_duty_rate = stamp_duty_rate
        self._transfer_fee_rate = transfer_fee_rate
        self._transfer_fee_venue = transfer_fee_venue
        self._currency = Currency.from_str("CNY")

    def get_commission(
        self,
        order: Any,  # noqa: ANN401
        fill_quantity: Any,  # noqa: ANN401
        fill_px: Any,  # noqa: ANN401
        instrument: Any,  # noqa: ANN401
    ) -> Money:
        """
        Return the total A-share commission for one fill, in CNY.

        The arguments are untyped to match the ``FeeModel`` base class, which the engine calls
        with concrete order and instrument types.
        """
        notional = Decimal(str(fill_quantity)) * Decimal(str(fill_px))

        commission = max(notional * self._commission_rate, self._min_commission)
        total = _fen(commission)

        if order.side == OrderSide.SELL:
            total += _fen(notional * self._stamp_duty_rate)

        if str(instrument.id.venue) == self._transfer_fee_venue:
            total += _fen(notional * self._transfer_fee_rate)

        return Money(total, self._currency)


def ashare_equity(
    code: str,
    venue: Venue = SSE,
    ts_event: int = 0,
    ts_init: int = 0,
) -> Equity:
    """
    Create an A-share equity instrument in CNY.

    The instrument uses a price precision of 2 with a 0.01 increment and a 100-share lot size.

    Parameters
    ----------
    code : str
        The six-digit stock code, for example ``"600519"``.
    venue : Venue, default SSE
        The exchange, either ``SSE`` (Shanghai) or ``SZSE`` (Shenzhen).
    ts_event : int, default 0
        UNIX timestamp in nanoseconds of the instrument event.
    ts_init : int, default 0
        UNIX timestamp in nanoseconds when the instrument was initialized.

    Returns
    -------
    Equity

    Raises
    ------
    ValueError
        If the code is not six digits, or the venue is not SSE or SZSE.

    """
    if len(code) != _CODE_LENGTH or not code.isdigit():
        raise ValueError(f"A-share code must be six digits, was {code!r}")
    if str(venue) not in {"SSE", "SZSE"}:
        raise ValueError(f"A-share venue must be SSE or SZSE, was {venue}")

    return Equity(
        instrument_id=InstrumentId.from_str(f"{code}.{venue}"),
        raw_symbol=Symbol(code),
        currency=Currency.from_str("CNY"),
        price_precision=2,
        price_increment=A_SHARE_PRICE_INCREMENT,
        lot_size=A_SHARE_LOT_SIZE,
        ts_event=ts_event,
        ts_init=ts_init,
    )


def _fen(amount: Decimal) -> Decimal:
    """
    Round an amount to the fen, half up.
    """
    return amount.quantize(_FEN, rounding=ROUND_HALF_UP)
