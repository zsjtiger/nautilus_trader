# CTP

[CTP](https://www.sfit.com.cn/) is the trading platform that Chinese futures brokers use to reach
the Shanghai, Dalian, Zhengzhou, China Financial Futures, Shanghai International Energy, and
Guangzhou futures exchanges. The `nautilus-ctp` adapter connects to CTP market data and trading
fronts through the [openctp](https://github.com/Jedore/openctp-ctp-python) CTP-compatible SDK, library
version `v6.7.11`.

:::warning
The adapter is in beta. The protocol is implemented, but it has been exercised only against
fake fronts in the test suite. Validate it against your broker's simulation front before trading
live capital.
:::

## Capability matrix

| Area                  | Status        | Notes                                                                                 |
| :-------------------- | :------------ | :------------------------------------------------------------------------------------ |
| Market data: quotes   | Supported     | Top of book from the depth snapshot. Emitted when bid and ask are both set.          |
| Market data: trades   | Inferred      | Derived from the increase in cumulative volume. No aggressor side.                   |
| Instruments           | Configuration | Declared with `CtpInstrumentDefinition`. The feed carries no contract reference data. |
| Orders: limit         | Supported     | `Day`, `IOC`, and `FOK` time in force.                                               |
| Orders: market        | Supported     | `IOC` and `FOK` only.                                                                |
| Orders: GTC           | Not supported | Denied before submission. Use `Day`.                                                 |
| Orders: post-only, stop, trigger, quote quantity | Not supported | Denied before submission. |
| Order modification    | Not supported | Cancel and resubmit instead.                                                         |
| Cancellation          | Supported     | Single order and cancel-all for an instrument.                                       |
| Order and fill reports | Supported    | From the day's order and trade queries.                                              |
| Positions             | Supported     | Net of the long and short records CTP reports.                                       |
| Account state         | Supported     | Balance, available funds, and margin from the trading account query.                 |
| Commission            | Zero          | The trade return carries no commission. Fills report zero commission.                |
| Python bindings       | Supported     | Configs, instrument definitions, and factories. Requires a build with the adapter.   |

## Installation

The adapter links the SDK only when built with the `ctp-native` feature. The default build compiles
the protocol and session logic, and a live client fails at `connect` with an error that names the
missing feature.

Provide two directories at build time:

- `CTP_SDK_DIR`: the CTP C++ headers for `v6.7.11` (`ThostFtdcMdApi.h`,
  `ThostFtdcTraderApi.h`, `ThostFtdcUserApiStruct.h`, and the rest of the header set). The openctp
  wheel does not ship headers.
- `CTP_LIB_DIR`: a directory containing `libthostmduserapi_se.so` and
  `libthosttraderapi_se.so`. The openctp wheel installs both under hashed names, so link the
  standard names to them.

```bash
pip install openctp-ctp==6.7.11.0
mkdir -p "$HOME/ctp-lib"
SITE="$(python -c 'import openctp_ctp, os; print(os.path.dirname(os.path.dirname(openctp_ctp.__file__)))')"
ln -sf "$(ls "$SITE"/openctp_ctp.libs/libthostmduserapi_se-*.so)" "$HOME/ctp-lib/libthostmduserapi_se.so"
ln -sf "$(ls "$SITE"/openctp_ctp.libs/libthosttraderapi_se-*.so)" "$HOME/ctp-lib/libthosttraderapi_se.so"
```

Build the adapter, or the Python package, with the feature enabled:

```bash
CTP_SDK_DIR=/path/to/ctp/headers CTP_LIB_DIR="$HOME/ctp-lib" \
  cargo build --locked -p nautilus-ctp --features ctp-native
```

At run time the library must be on the loader path, for example through `LD_LIBRARY_PATH`.

### Locale

The CTP library checks the process locale when it loads. A missing or invalid locale aborts the
process with `locale::facet::_S_create_c_locale name not valid`. Run with a locale that exists on
the host, such as `C.UTF-8` or a Chinese locale.

## Symbology

Each instrument is identified as `<ctp_symbol>.<exchange>`, for example `rb2510.SHFE`. The CTP
symbol is the name used on the wire. The exchange code is the venue suffix.

## Configuration

Instruments are declared because the market data feed carries no contract reference data. The
definition must match the exchange specification. The price increment's scale sets the price
precision, and the multiplier is in currency units per price point.

```python
from nautilus_trader.adapters.ctp import CtpDataClientConfig
from nautilus_trader.adapters.ctp import CtpExecClientConfig
from nautilus_trader.adapters.ctp import CtpInstrumentDefinition
from nautilus_trader.model import AssetClass

rebar = CtpInstrumentDefinition(
    ctp_symbol="rb2510",
    exchange="SHFE",
    underlying="rb",
    asset_class=AssetClass.COMMODITY,
    currency="CNY",
    expiry_date="20251015",
    price_increment="1",
    multiplier="10",
)

data_config = CtpDataClientConfig(
    broker_id="9999",
    user_id="investor-id",
    password="investor-password",
    front_address="tcp://<market-data-front>",
    instruments=[rebar],
    production_mode=False,
)

exec_config = CtpExecClientConfig(
    broker_id="9999",
    investor_id="investor-id",
    password="investor-password",
    trade_front_address="tcp://<trading-front>",
    instruments=[rebar],
    app_id="<app-id>",       # Set with auth_code when the front requires terminal authentication.
    auth_code="<auth-code>",
    production_mode=False,
)
```

Register the factories with the node builder:

```python
from nautilus_trader.adapters.ctp import CtpDataClientFactory
from nautilus_trader.adapters.ctp import CtpExecutionClientFactory
from nautilus_trader.common import Environment
from nautilus_trader.live import LiveNode
from nautilus_trader.model import TraderId

node = (
    LiveNode.builder("CTP-001", TraderId.from_str("TRADER-001"), Environment.LIVE)
    .add_data_client(None, CtpDataClientFactory(), data_config)
    .add_exec_client(None, CtpExecutionClientFactory(), exec_config)
    .build()
)
```

Use `production_mode=False` against a simulation front. Set `flow_path` to a writable directory when
the working directory is not writable, because the SDK stores session files there.

## Market data

- **Quotes** are emitted for each depth snapshot where both the bid and the ask are set. An unset
  price is reported as `DBL_MAX` by the SDK and is treated as absent.
- **Trades** are inferred. When the cumulative session volume rises, a trade is emitted for the
  increase, priced at the last traded price, with aggressor side `NoAggressor`. The trade ID is
  `<action day>-<cumulative volume>`. Several trades between two snapshots collapse into one, and a
  feed gap can hide trades.
- **Timestamps** use the exchange clock (UTC+8) from the action day and update time, with
  millisecond precision.
- **Reconnects** clear the volume baseline so no trade spans a gap, and resubscribe every symbol
  that has subscription demand.

## Execution

### Order mapping

| Nautilus order                       | CTP order                                          |
| :----------------------------------- | :------------------------------------------------- |
| Limit, `Day`                         | Limit, good for the day (`GFD`)                    |
| Limit, `IOC`                         | Limit, immediate or cancel                         |
| Limit, `FOK`                         | Limit, immediate, complete volume                  |
| Market, `IOC` or `FOK`               | Any price, immediate                               |
| `reduce_only`                        | Close offset (`Close`)                             |
| Otherwise                            | Open offset (`Open`)                               |

Orders are denied before submission when they use `GTC`, post-only, stop or trigger types, quote
quantity, a fractional quantity, or a market order with a time in force other than `IOC` or `FOK`.

:::warning
On exchanges that distinguish close-today from close-yesterday, such as the Shanghai and Shanghai
International Energy exchanges, a plain close can be rejected. The adapter does not yet split a
close into close-today and close-yesterday legs, so verify closing orders on the simulation front
first.
:::

### Lifecycle

- **Accepted**: emitted on the first order return that is queued or traded. A fill that arrives first
  emits the accept event before the fill.
- **Rejected**: emitted for an insert rejected by the front, or by the exchange, with the exchange's
  status message as the reason.
- **Canceled**: emitted when the remaining volume is canceled, including immediate-or-cancel
  remainders.
- **Cancel rejected**: emitted when the front or exchange rejects a cancel.
- **Filled**: emitted per trade return, with the trade's price, volume, and exchange time. The
  commission is zero.

### Restart and recovery

The session recovers the day's orders and trades with the reports the engine requests at startup.
Orders placed by this process are tracked by an order reference that is only valid for this front
and session. Orders from an earlier process appear as external orders and fills, which the engine
reconciles from the reports. The adapter does not persist its order references across restarts.

## Deterministic simulation

The claimed slice is the market data and trading session cores, their pure parsing, the client event
loops and report paths, and instrument construction. Those files are gated by `check-dst-conventions`.
Their timers go through the `nautilus_common::live::dst::time` facade, their `select!` blocks are
`biased`, and their collections that reach the wire or the output are ordered. The clock is injected
at construction.

The native transport is not claimed. The CTP SDK owns its network sockets and worker threads, which
madsim cannot intercept, so no controlled local peer can carry its traffic. A fixed-input replay test
runs the trading flow twice in fresh clients and compares the wire transcript and the domain output.
It also checks that a changed input changes the output. A fresh-process comparison and a peer-driven
wire pin are not yet in place.

## Known limitations

- Trading against a live front has not been verified in this release. Use a simulation front first.
- Trades are inferred from volume, so they are not authoritative. Use the order and trade reports to
  reconcile fills.
- Instruments must be declared in configuration. The adapter does not load them from the front.
- Commission is reported as zero. Account it separately if fees matter for your strategy.
- Order references are not persisted across restarts.
- Closing orders on exchanges that split close-today and close-yesterday are not yet split.
