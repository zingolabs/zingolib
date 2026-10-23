# The three-language wire contract

The hex files under `../test-data/golden/` pin the exact uniffi wire encoding
of every value the nym FFI functions carry: the `Socks5Endpoint` record that
`socks5_endpoint()` returns (including both port extremes), every
`ProxyDeathReason` variant that `ProxyDeathObserver::on_death` receives, and
every `ProxyFfiError` variant that `start()` can raise. Three test suites assert
both directions — lowering produces exactly the pinned bytes, and lifting the
pinned bytes produces exactly the canonical value — against the same files:

- **Rust**: `../tests/golden_wire.rs`, run by `cargo nextest run` in this
  workspace. This suite also owns the canonical values; the Kotlin and Swift
  suites repeat them verbatim.
- **Kotlin**: `kotlin/GoldenWireContractTest.kt`, compiled against the
  Kotlin Binding Set and JUnit 4. The Gradle library under `bindings/android`
  adds this directory as a test source and sets `zingo.golden.dir` to the
  absolute path of `zingo-netutils/nym-proxy-ffi/test-data/golden`, and the
  package job of
  `.github/workflows/binding-layer-android.yaml` runs it with
  `testReleaseUnitTest` against the Binding Set the AAR ships.
- **Swift**: `swift/GoldenWireContractTests.swift`, an XCTest in the
  test-only package `Package.swift` beside it, which depends on the shipped
  package under `bindings/swift`. The iOS plan of `build-binding-layer` ends
  by running it on an iOS simulator, so
  `.github/workflows/binding-layer-ios.yaml` runs it on every build. The
  golden directory resolves relative to the file. The builder requires the
  executed test count that xcodebuild reports to equal the suite's size.
  The test binary also links `app-personalities/`, a C++ throw and an
  Objective-C throw, so it carries the `__gxx_personality_v0` and
  `__objc_personality_v0` routines an app brings beside the archive's
  `rust_eh_personality`. Compact unwind encodes three routines at most, so
  a bundle that brings two Rust archives, each with its own std, fails this
  link the way it fails the app's.

A single-language round trip proves only that one side's codec is
self-inverse. Sharing the pins is what makes the guarantee two-sided at test
time: if any side's generated codec drifts, that side's own suite fails in
its own language, with no shared process.

The pins are never-regenerate contract artifacts. A mismatch means the wire
encoding changed — a breaking change to every shipped binding. Do not bless
it away: decide the revision deliberately, delete the affected pin in the
same change that justifies it, and mint the replacement with the ignored
`bless_missing_goldens` test, which only ever creates absent files.
