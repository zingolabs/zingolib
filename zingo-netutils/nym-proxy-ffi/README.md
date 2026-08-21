# zingo-nym-proxy-ffi

The Nym mixnet proxy as a UniFFI library for the mobile app.

This directory is its own Cargo workspace with its own `Cargo.lock`. The
root workspace in `rust/` excludes it.

## Crates

- `zingo-nym-proxy-ffi` (`src/lib.rs`): wraps `NymProxy` from zingolib's
  `zingo-netutils`. `MixnetProxyHandle::start` joins the Nym network and
  opens a local SOCKS5 listener. `socks5_endpoint` and `exit_node` report
  what it bound. `stop` tears it down. A listener monitor calls
  `ProxyDeathObserver::on_death` once if the listener is lost. Builds as
  `cdylib` for Android, `staticlib` for iOS, and `lib` for host tests.
- `zingo-nym-tls-init` (`tls-init/`): Android only. One JNI function,
  `NymTlsInit.initPlatformVerifier`, that hands the app `Context` to
  `rustls-platform-verifier`.

## Consumers

- Android: `android/app/src/main/java/org/ZingoLabs/Zingo/NymTransportModule.kt`
  loads `libzingo_nym_proxy_ffi.so` through the generated Kotlin bindings
  (`scripts/generate_kotlin_bindings.mjs`, not checked in).
  `rust/workbench` has `bundle-android-shim` and `consume-android-shim`.
- iOS: `ios/NymTransportModule.swift` links `ZingoNymProxyFFI.xcframework`
  and the generated Swift bindings. `rust/ios/build_ios.mjs` builds both.
- The app hands the endpoint to the wallet FFI's `attachMixnet`.

## Tests

- `cargo nextest run --workspace`: unit tests, `tests/ffi_roundtrip.rs`,
  `tests/golden_wire.rs` against `test-data/golden/`.
- `cargo nextest run --features live-mixnet --test live_mixnet`: starts the
  real proxy against the Nym network.
- `contract-tests/`: the Kotlin and Swift golden tests against the same
  pins. See its README.
