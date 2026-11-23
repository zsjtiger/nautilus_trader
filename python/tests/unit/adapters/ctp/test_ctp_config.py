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
Test CTP configuration types and factory construction.
"""

import pytest

from nautilus_trader.adapters.ctp import CTP
from nautilus_trader.adapters.ctp import CtpDataClientConfig
from nautilus_trader.adapters.ctp import CtpDataClientFactory
from nautilus_trader.adapters.ctp import CtpExecClientConfig
from nautilus_trader.adapters.ctp import CtpExecutionClientFactory
from nautilus_trader.adapters.ctp import CtpInstrumentDefinition
from nautilus_trader.common import Environment
from nautilus_trader.live import LiveNode
from nautilus_trader.model import AssetClass
from nautilus_trader.model import TraderId


PASSWORD = "unit-test-password"


def _rebar() -> CtpInstrumentDefinition:
    return CtpInstrumentDefinition(
        ctp_symbol="rb2510",
        exchange="SHFE",
        underlying="rb",
        asset_class=AssetClass.COMMODITY,
        currency="CNY",
        expiry_date="20251015",
        price_increment="1",
        multiplier="10",
    )


def test_ctp_constant_names_the_adapter() -> None:
    """
    Test the CTP constant names the adapter.
    """
    assert CTP == "CTP"


def test_instrument_definition_stores_fields_and_repr_is_available() -> None:
    """
    Test an instrument definition exposes its symbol in its representation.
    """
    definition = _rebar()
    assert "rb2510" in repr(definition)


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("expiry_date", "2025-10-15"),
        ("price_increment", "0"),
        ("multiplier", "-1"),
        ("price_increment", "not-a-number"),
    ],
)
def test_instrument_definition_rejects_invalid_values(field: str, value: str) -> None:
    """
    Test an instrument definition rejects invalid field values.
    """
    kwargs = {
        "ctp_symbol": "rb2510",
        "exchange": "SHFE",
        "underlying": "rb",
        "asset_class": AssetClass.COMMODITY,
        "currency": "CNY",
        "expiry_date": "20251015",
        "price_increment": "1",
        "multiplier": "10",
    }
    kwargs[field] = value
    with pytest.raises(ValueError, match=r"YYYYMMDD|must be positive|invalid"):
        CtpInstrumentDefinition(**kwargs)


def test_data_config_builds_and_keeps_password_out_of_repr() -> None:
    """
    Test the data config builds and keeps the password out of its representation.
    """
    config = CtpDataClientConfig(
        broker_id="9999",
        user_id="123456",
        password=PASSWORD,
        front_address="tcp://127.0.0.1:10131",
        instruments=[_rebar()],
    )

    assert PASSWORD not in repr(config)
    assert PASSWORD not in str(config)


def test_data_config_requires_instruments() -> None:
    """
    Test the data config requires at least one instrument.
    """
    with pytest.raises(ValueError, match="instruments"):
        CtpDataClientConfig(
            broker_id="9999",
            user_id="123456",
            password=PASSWORD,
            front_address="tcp://127.0.0.1:10131",
            instruments=[],
        )


def test_exec_config_rejects_partial_authentication() -> None:
    """
    Test the execution config rejects an app ID without an authentication code.
    """
    with pytest.raises(ValueError, match="app_id"):
        CtpExecClientConfig(
            broker_id="9999",
            investor_id="123456",
            password=PASSWORD,
            trade_front_address="tcp://127.0.0.1:10130",
            instruments=[_rebar()],
            app_id="client",
            auth_code=None,
        )


def test_exec_config_accepts_full_authentication() -> None:
    """
    Test the execution config accepts complete authentication and hides the password.
    """
    config = CtpExecClientConfig(
        broker_id="9999",
        investor_id="123456",
        password=PASSWORD,
        trade_front_address="tcp://127.0.0.1:10130",
        instruments=[_rebar()],
        app_id="client",
        auth_code="code",
    )

    assert PASSWORD not in repr(config)


def test_factories_report_the_adapter_name() -> None:
    """
    Test both CTP factories report the adapter name.
    """
    assert CtpDataClientFactory().name() == CTP
    assert CtpExecutionClientFactory().name() == CTP


def test_live_node_builder_constructs_ctp_clients() -> None:
    """
    Test a Python node can construct both CTP clients without connecting.
    """
    trader_id = TraderId.from_str("TESTER-001")
    node = (
        LiveNode.builder("CTP-PYTEST-001", trader_id, Environment.SANDBOX)
        .add_data_client(
            None,
            CtpDataClientFactory(),
            CtpDataClientConfig(
                broker_id="9999",
                user_id="123456",
                password=PASSWORD,
                front_address="tcp://127.0.0.1:10131",
                instruments=[_rebar()],
            ),
        )
        .add_exec_client(
            None,
            CtpExecutionClientFactory(),
            CtpExecClientConfig(
                broker_id="9999",
                investor_id="123456",
                password=PASSWORD,
                trade_front_address="tcp://127.0.0.1:10130",
                instruments=[_rebar()],
            ),
        )
        .build()
    )

    assert node is not None
