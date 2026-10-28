# Equity

`Equity` represents a listed share, ETF, or similar cash-market security. Nautilus uses
this type for instruments that trade in whole units, quote in one currency, and have no
contract expiry.

Examples include `AAPL.XNAS`, `MSFT.XNAS`, and venue-specific ETF symbols.

## Fields

| Field             | Rust type          | Python type        | Required/default | Notes                                    |
| ----------------- | ------------------ | ------------------ | ---------------- | ---------------------------------------- |
| `instrument_id`   | `InstrumentId`     | `InstrumentId`     | Required         | Stored as `id` in Rust.                  |
| `raw_symbol`      | `Symbol`           | `Symbol`           | Required         | Native venue symbol.                     |
| `currency`        | `Currency`         | `Currency`         | Required         | Quote and settlement currency.           |
| `price_precision` | `u8`               | `int`              | Required         | Decimal places allowed for prices.       |
| `price_increment` | `Price`            | `Price`            | Required         | Smallest valid price step.               |
| `lot_size`        | `Option<Quantity>` | `Quantity \| None` | `None`           | Board lot or whole-share lot size.       |
| `ts_event`        | `UnixNanos`        | `int`              | Required         | Event timestamp in nanoseconds.          |
| `ts_init`         | `UnixNanos`        | `int`              | Required         | Initialization timestamp in nanoseconds. |
| `isin`            | `Option<Ustr>`     | `str \| None`      | `None`           | International Securities ID when known.  |
| `max_quantity`    | `Option<Quantity>` | `Quantity \| None` | `None`           | Maximum order quantity.                  |
| `min_quantity`    | `Option<Quantity>` | `Quantity \| None` | `None`           | Minimum order quantity.                  |
| `max_price`       | `Option<Price>`    | `Price \| None`    | `None`           | Maximum valid quote or order price.      |
| `min_price`       | `Option<Price>`    | `Price \| None`    | `None`           | Minimum valid quote or order price.      |
| `margin_init`     | `Option<Decimal>`  | `Decimal \| None`  | `0`              | Initial margin rate.                     |
| `margin_maint`    | `Option<Decimal>`  | `Decimal \| None`  | `0`              | Maintenance margin rate.                 |
| `tick_scheme`     | `Option<Ustr>`     | `str \| None`      | `None`           | Registered variable tick scheme name.    |
| `info`            | `Option<Params>`   | `dict \| None`     | `None`           | Adapter metadata.                        |

**Note**: Python constructors use `instrument_id`. Rust stores the same value as `id`.

## Behavior

- `Equity` has asset class `Equity` and instrument class `Spot`.
- Quantity precision is always zero, so orders use whole-share quantities.
- The multiplier and size increment are one.
- It has no base currency, expiry, strike, option kind, or inverse costing flag.
- Use price limits only when the venue publishes them.

:::warning
`Equity` fixes size precision at zero. The `RiskEngine` denies any order whose quantity
precision exceeds the instrument size precision, so fractional-share quantities are rejected.
:::

## Example

```rust tab="Rust"
use nautilus_core::UnixNanos;
use nautilus_model::{
    identifiers::{InstrumentId, Symbol},
    instruments::Equity,
    types::{Currency, Price, Quantity},
};
use ustr::Ustr;

let aapl = Equity::builder()
    .instrument_id(InstrumentId::from("AAPL.XNAS"))
    .raw_symbol(Symbol::from("AAPL"))
    .isin(Ustr::from("US0378331005"))
    .currency(Currency::from("USD"))
    .price_precision(2)
    .price_increment(Price::from("0.01"))
    .lot_size(Quantity::from("100"))
    .ts_event(UnixNanos::default())
    .ts_init(UnixNanos::default())
    .build()
    .unwrap();
```

```python tab="Python"
from nautilus_trader.model import Currency
from nautilus_trader.model import Equity
from nautilus_trader.model import InstrumentId
from nautilus_trader.model import Price
from nautilus_trader.model import Quantity
from nautilus_trader.model import Symbol

aapl = Equity(
    instrument_id=InstrumentId.from_str("AAPL.XNAS"),
    raw_symbol=Symbol("AAPL"),
    currency=Currency.from_str("USD"),
    price_precision=2,
    price_increment=Price.from_str("0.01"),
    ts_event=0,
    ts_init=0,
    isin="US0378331005",
    lot_size=Quantity.from_int(100),
)
```

## China A-share equities

`nautilus_trader.execution.ashare` provides helpers for China A-share stocks. The helpers build
`Equity` instruments and a commission model; they do not connect to a venue.

- `ashare_equity(code, venue=SSE)` creates a CNY equity with a price precision of 2, a 0.01 price
  increment, and a 100-share lot size. The code must be six digits. The venue must be `SSE` or `SZSE`.
- `AShareFeeModel` charges brokerage commission with a per-fill minimum, stamp duty on sells, and a
  transfer fee on `SSE` fills. Pass it as `fee_model` to `add_venue()`. The rates are parameters
  with defaults that change over time, so check them against the current schedules.

To reject orders whose quantity is not a whole multiple of the lot size, list the venue in
`RiskEngineConfig.lot_size_enforced_venues`. The check is off by default. Such orders are denied with
`QUANTITY_NOT_LOT_MULTIPLE`. Orders priced in quote currency and validated full-position exits are not
checked.

To enforce T+1 settlement, list the venue in `RiskEngineConfig.t_plus_one_venues`. A sell is denied with
`SELLABLE_QUANTITY_EXCEEDED` when it exceeds the net long position less the shares bought earlier on the
same trading day and less quantity committed to other open sell orders. A trading day runs from midnight
UTC+8. The check applies to submissions and modifications, except validated full-position exits.

To enforce daily price limits, list the venue in `RiskEngineConfig.price_limit_venues`. Limit orders and
limit modifications are denied with `PRICE_OUTSIDE_LIMIT` when the price falls outside the limits. The
limits are set from the previous trading day's close, using the most recent cached daily bar dated before
today: 20% for STAR and ChiNext codes (300, 301, 688) and 10% otherwise, rounded half away from zero to
the price precision. If no previous close is cached, the order is denied with
`PRICE_LIMIT_REFERENCE_MISSING`. Load daily bars for the instrument before trading on a price-limit venue.

:::note
The price limits do not yet cover ST stocks (5%) or the new-listing exemptions, and market orders are not
checked against the limits.
:::

## Adapters

**Adapters that create or consume `Equity` instruments include**:

- [Databento](../../integrations/databento.md) for listed US equities and ETFs.
- [Interactive Brokers](../../integrations/interactive_brokers.md) for listed equity contracts.

## Related guides

- [Data](../data/) explains market data that references instruments.
- [Value types](../value_types.md) explains `Price`, `Quantity`, and `Money`.
