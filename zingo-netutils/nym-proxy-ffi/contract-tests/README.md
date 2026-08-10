# The three-language wire contract

The hex files under `../test-data/golden/` pin the exact uniffi wire encoding
of every value the nym FFI functions carry: the `Socks5Endpoint` record that
`socks5_endpoint()` returns (including both port extremes), the
`ProxyDeathReason` that `ProxyDeathObserver::on_death` receives, and every
`ProxyFfiError` variant that `start()` can raise. Three test suites assert
both directions — lowering produces exactly the pinned bytes, and lifting the
pinned bytes produces exactly the canonical value — against the same files:

- **Rust**: `../tests/golden_wire.rs`, run by `cargo nextest run` in this
  workspace. This suite also owns the canonical values; the Kotlin and Swift
  suites repeat them verbatim.
- **Kotlin**: `kotlin/GoldenWireContractTest.kt`, compiled against the
  generated bindings and JUnit 4. zingo-mobile's test suite wires it in
  during the step-3/step-4 packaging (#2513/#2505); point `zingo.golden.dir`
  at `../test-data/golden` if the working directory differs.
- **Swift**: `swift/GoldenWireContractTests.swift`, an XCTest that joins the
  Mac-gated step-7 packaging (#2504). The golden directory resolves relative
  to the file, or set `ZINGO_GOLDEN_DIR`.

A single-language round trip proves only that one side's codec is
self-inverse. Sharing the pins is what makes the guarantee two-sided at test
time: if any side's generated codec drifts, that side's own suite fails in
its own language, with no shared process.

The pins are never-regenerate contract artifacts. A mismatch means the wire
encoding changed — a breaking change to every shipped binding. Do not bless
it away: decide the revision deliberately, delete the affected pin in the
same change that justifies it, and mint the replacement with the ignored
`bless_missing_goldens` test, which only ever creates absent files.
