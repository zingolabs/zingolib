# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
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
