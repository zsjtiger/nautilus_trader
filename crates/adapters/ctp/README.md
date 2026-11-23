# nautilus-ctp

[NautilusTrader](https://nautilustrader.io) adapter for [CTP](https://www.sfit.com.cn/), the
Comprehensive Transaction Platform used by Chinese futures exchanges.

The crate provides live market data (quotes, and trades inferred from cumulative volume), order
entry and cancellation, order, fill, position, and account reports, and Python bindings for the
configuration types and client factories. It reaches the market through the
[openctp](https://github.com/Jedore/openctp-ctp-python) CTP-compatible SDK, library version `v6.7.11`.

The integration guide, including the capability matrix, order mapping, and deterministic simulation
slice, is at [docs/integrations/ctp.md](../../../docs/integrations/ctp.md).

## Feature flags

- `ctp-native`: Links the CTP SDK for live sessions. Requires `CTP_SDK_DIR` and `CTP_LIB_DIR` at
  build time. Without it, the clients build and fail on `connect` with an error that names this
  feature.
- `extension-module`: Builds as a Python extension module.
- `high-precision` (default): Uses 128-bit value types for prices and quantities.
- `python`: Enables the PyO3 bindings.

## Building with the native SDK

Provide the CTP C++ headers for `v6.7.11` in `CTP_SDK_DIR`, and a directory containing
`libthostmduserapi_se.so` and `libthosttraderapi_se.so` in `CTP_LIB_DIR`. The openctp wheel does
not ship headers, and it installs the libraries under hashed names, so link the standard names to
them.

```bash
CTP_SDK_DIR=/path/to/ctp/headers CTP_LIB_DIR=/path/to/ctp-lib \
  cargo build --locked -p nautilus-ctp --features ctp-native
```

At run time the library must be on the loader path, for example through `LD_LIBRARY_PATH`. The CTP
library checks the process locale when it loads. Run with a locale that exists on the host.

## Limitations

- Live trading against a front has not been verified. Use a simulation front first.
- Trades are inferred from cumulative volume, so they are not authoritative.
- Instruments are declared in configuration, not loaded from the front.
- Order modification, GTC, post-only, and stop or trigger orders are denied before submission.
- Commission is reported as zero.

## Documentation

- [Integration guide](../../../docs/integrations/ctp.md)
- [Adapter developer guide](../../../docs/developer_guide/adapters.md)
- [API reference](https://docs.rs/nautilus-ctp/latest/nautilus_ctp/)

## License

Licensed under the GNU Lesser General Public License Version 3.0. See [LICENSE](LICENSE).
