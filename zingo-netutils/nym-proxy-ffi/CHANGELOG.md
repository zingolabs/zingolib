# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- The Android library builds under the workspace's `mobile` profile, the one
  profile of the Binding Layer, and the `proxy` profile is gone. The nym stack
  beneath the crate now compiles with the same size optimization and overflow
  checks on both platforms, where the one-archive iOS build had left it checked
  on iOS and wrapping on Android. A plain-operator overflow in a nym crate is a
  panic on both platforms, which ends its tokio task and leaves the process
  running.
- The Binding Layer builder compiles the Android library and the wallet
  library into one target directory, `target/binding-layer/android`, where
  each had its own, so the crates the two share compile once per ABI and sit
  once in the CI cache. The library now lands at
  `target/binding-layer/android/<triple>/mobile/libzingo_nym_proxy_ffi.so`,
  where `android/proxy/` held it. The host plan merges the same way under
  `target/binding-layer/host`. The two cargo invocations stay separate, so
  each crate keeps its own feature set. A workbench test pins the Android
  builds to one directory.
- On iOS the crate ships inside `Zingolib.xcframework`, the one archive of the
  `ZingoBindings` package, which the wallet crate builds with its
  `zingo-nym-proxy-ffi` feature. `ZingoNymProxyFFI.xcframework` is gone. The
  app then links one Rust std and one `rust_eh_personality` beside its C++
  and Objective-C personality routines, the three that compact unwind
  encodes. Two archives brought two, and the linker of Xcode 26.3 refused
  the app's link. The `ZingoBindings` product, which the app imports, is
  unchanged.
- **Breaking:** the uniffi pin moves from 0.28 to 0.32.2, the root workspace's
  pin, and `zingo-uniffi-bindgen` generates the crate's bindings in library
  mode from the compiled library. A consumer regenerates its bindings; the
  generated files of an older uniffi do not load the new library.
- The crate joins the zingo-netutils workspace. Its own lockfile, `[patch]`
  stanza and nextest configuration move to that workspace, whose CI job tests
  the crate without `live-mixnet`.
