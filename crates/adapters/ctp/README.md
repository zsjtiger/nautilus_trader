# nautilus-ctp

[NautilusTrader](https://nautilustrader.io) adapter for [CTP](https://www.sfit.com.cn/), the
Comprehensive Transaction Platform used by Chinese futures exchanges.

This crate currently provides **live market data**: quotes from the depth snapshot, trades
inferred from cumulative volume, and instruments declared in configuration. Trading (order entry,
positions, and account state) is not implemented yet.

The crate reaches the market through the [openctp](https://github.com/Jedore/openctp-ctp-python)
CTP-compatible SDK. The library version is `v6.7.11`.

## Feature flags

- `ctp-native`: Links the CTP SDK for live sessions. Requires the build-time environment variables
  described below.
- `high-precision` (default): Uses 128-bit value types for prices and quantities.

## Building with the native SDK

The default build compiles the configuration, parsing, and session logic only. Creating a live
client fails at `connect` with an error that names the missing feature. To link the SDK, enable
`ctp-native` and provide two directories:

- `CTP_SDK_DIR`: the CTP C++ headers for the same version (`ThostFtdcMdApi.h`,
  `ThostFtdcUserApiStruct.h`, and the rest of the header set). The openctp wheel does not ship
  headers.
- `CTP_LIB_DIR`: a directory containing `libthostmduserapi_se.so`. The openctp wheel installs the
  library under a hashed name, so create a symlink with the standard name.

```bash
pip install openctp-ctp==6.7.11.0
mkdir -p "$HOME/ctp-lib"
ln -sf "$(python -c 'import openctp_ctp, glob, os; d=os.path.join(os.path.dirname(openctp_ctp.__file__), "..", "openctp_ctp.libs"); print(glob.glob(os.path.join(d, "libthostmduserapi_se-*.so"))[0])')" \
  "$HOME/ctp-lib/libthostmduserapi_se.so"
CTP_SDK_DIR=/path/to/ctp/headers CTP_LIB_DIR="$HOME/ctp-lib" \
  cargo build -p nautilus-ctp --features ctp-native
```

At run time the library must be on the loader path, for example through `LD_LIBRARY_PATH`.

### Locale

The CTP library checks the process locale when it loads. If the locale is missing or invalid, the
process aborts with `locale::facet::_S_create_c_locale name not valid`. Run with a locale that
exists on the host, such as `C.UTF-8` or a Chinese locale.

## Configuration

`CtpDataClientConfig` holds the broker ID, investor account, password (redacted in debug output),
front address, flow directory, production mode flag, login timeout, and instrument definitions.

Each `CtpInstrumentDefinition` describes one futures contract: the CTP symbol (for example
`rb2510`), the exchange (`SHFE`), the underlying, the asset class, the settlement currency, the last
trading day (`YYYYMMDD`), the price increment, and the contract multiplier. The price increment's
scale sets the price precision.

The feed carries no contract reference data, so instrument definitions must be complete and match
the exchange specification.

## Data semantics

- **Quotes** come from the top of the depth snapshot. A quote is emitted only when both the bid and
  the ask are set.
- **Trades** are inferred. CTP market data has no trade messages. A trade is emitted when the
  cumulative session volume increases, priced at the last traded price, with the size of the
  increase. Trades carry `NoAggressor`, and their IDs are `<action day>-<cumulative volume>`.
  Several trades between two snapshots collapse into one, and a feed gap can hide trades.
- **Timestamps** use the exchange clock (UTC+8) from the action day and update time, with
  millisecond precision.
- **Reconnects** clear the volume baseline and resubscribe every symbol that has demand, so no
  trade spans a gap.
- **Unset values** reported by the SDK as `DBL_MAX` are treated as absent.

Prices arrive as the SDK's IEEE-754 doubles. The conversion to `Price` happens at the parsing
boundary and rounds to the instrument's price precision.

## Known limitations

- No trading. Order entry, order status, positions, and account state are not implemented.
- No Python bindings yet, so no live node factory is exposed.
- Error messages from the SDK are GBK encoded. Invalid UTF-8 is replaced when decoded.
- The client is not yet deterministic-simulation conformant (see `docs/developer_guide/adapters.md`).
  It reads the wall clock for `ts_init` and runs on the real SDK thread.

## Documentation

- [Adapter developer guide](https://github.com/nautechsystems/nautilus_trader/blob/develop/docs/developer_guide/adapters.md)
- [API reference](https://docs.rs/nautilus-ctp/latest/nautilus_ctp/)

## License

Licensed under the GNU Lesser General Public License Version 3.0. See [LICENSE](LICENSE).
