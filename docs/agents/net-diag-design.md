# Design: the net-diag failure taxonomy

Status: ready for implementation. Target branch: `nym_mobile_adoption`
(PR #2527). Written 2026-07-26 during the zingo-mobile silent-alpha
verification sessions (zingo-mobile#1207). Related records: ADR 0011 and
its mobile amendment, ADR 0017, issue #2531.

## Problem

Field testing of the mobile alpha produced failures whose causes were
buried. A price fetch hung for five minutes with no bound and finally
died with `tls handshake eof`. The root causes of two earlier outages
(an uninitialized platform verifier, then an empty manual root store)
each cost a debugging session because every error crossed the system as
flattened prose. The wallet's own liveness verdict (`died`) says nothing
about why. Send fan-out failures join per-witness errors into one
string.

The information exists at the failure site and is destroyed on the way
out. This design keeps it.

## Goals

1. Every covered network operation (price fetch, broadcast fan-out,
   attach validation) reports failures as data: which stage failed,
   against what target, with the full cause chain.
2. One taxonomy reused across all of them, so a consumer that learns to
   read one operation's failures can read them all.
3. Classification is done by pure functions over error values. No
   effects, no clock, no logging inside them. Fully unit-testable with
   fabricated errors.
4. The price fetch gets a client-side timeout so a hang becomes a typed
   stage instead of an unbounded wait.

## Non-goals

Fielded UniFFI errors (structured stage and target crossing the mobile
FFI as typed fields) are explicitly out of scope. That change amends the
mobile error contract and gets its own reviewed PR. This design must be
forward-compatible with it: keep `NetOpStage` small, data-carrying, and
serializable, and nothing here may depend on the flat message format
beyond the stability contract below.

## The crate

New crate `zingo-net-diag` at the repository root, std-only, zero
dependencies. Both cargo workspaces path-depend on it (the parent
workspace from `zingo-price` and `zingolib`, the `zingo-netutils`
standalone workspace optionally, for the shim follow-up). Zero
dependencies is a hard requirement: it is what lets one crate serve two
lockfile-isolated workspaces without resolver coupling.

```rust
/// Where, along a covered network operation, the failure occurred.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetOpStage {
    /// Refused before any network touch: the mixnet route resolved to
    /// off, bootstrapping, or died.
    RouteResolution,
    /// The local SOCKS5 endpoint could not be reached.
    LocalProxyConnect,
    /// SOCKS5 negotiation with the local proxy failed.
    SocksHandshake,
    /// The tunnel was established and the data path then broke.
    TunnelTransport,
    /// TLS with the remote target failed through the tunnel.
    RemoteTls,
    /// The remote target answered with an HTTP-level failure.
    RemoteHttp,
    /// The target's response body was undecodable.
    PayloadDecode,
    /// The operation exceeded its client-side bound.
    TimedOut { after_ms: u64 },
}

/// The reusable failure record for one attempt against one target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetOpFailure {
    pub stage: NetOpStage,
    /// The remote target, as a host or URI string. For stages before the
    /// tunnel, the local SOCKS endpoint.
    pub target: String,
    /// The full cause chain, deepest cause last.
    pub detail: String,
}
```

`Display for NetOpFailure` renders `failed at {stage} to {target}:
{detail}` with stage names in kebab-case (`remote-tls`,
`timed-out(25000ms)`). This rendering is a stability contract: logs and
the mobile error messages carry it, and it must not change shape without
a changelog entry. Consumers must never parse it to make decisions. It
is for humans and logs. Machine dispatch waits for the fielded-error PR.

Display for each error type prints its own layer only. The cause chain
belongs in `std::error::Error::source`, never concatenated into the
message. The mobile FFI already walks `source()` and renders the chain.
Concatenating locally produces the duplicated nesting observed on device
(the same URL printed four times). One of the acceptance criteria below
pins this.

## Pure classifiers

The crate offers a generic chain inspector and the consumers offer
error-type-specific classifiers built on it.

```rust
/// Walks a cause chain and returns each layer's Display text, outermost
/// first. Pure.
pub fn chain_texts(error: &(dyn std::error::Error + 'static)) -> Vec<String>;
```

In `zingo-price` (which owns the reqwest dependency), a pure classifier
maps a `reqwest::Error` to a stage:

```rust
fn classify_reqwest(error: &reqwest::Error) -> NetOpStage
```

Classification rules, checked in order:

| Evidence | Stage |
| --- | --- |
| `error.is_timeout()` | `TimedOut` |
| `error.is_connect()` and the chain mentions the local SOCKS address | `LocalProxyConnect` |
| chain mentions SOCKS negotiation | `SocksHandshake` |
| `error.is_connect()` and the chain mentions a TLS handshake | `RemoteTls` when the handshake reached the target, `TunnelTransport` for a mid-handshake EOF (`tls handshake eof`) |
| `error.is_status()` | `RemoteHttp` |
| `error.is_decode()` or `is_body()` | `PayloadDecode` |
| anything else | `TunnelTransport` |

The boundary between `RemoteTls` and `TunnelTransport` is genuinely
fuzzy from reqwest's chain alone. Classify conservatively and document
the choice in the classifier. Do not let the fuzziness leak into more
stages: two adjacent stages with a documented boundary beat five precise
ones nobody can produce.

Every classifier is tested with fabricated error chains covering every
stage, and an exhaustiveness test asserts every `NetOpStage` variant is
reachable from at least one fabricated input.

## Integration points

### zingo-price

The reqwest client gains `.timeout(Duration::from_secs(20))` and
`.connect_timeout(Duration::from_secs(10))`. Twenty seconds keeps the
native bound under the mobile UI's 25-second watchdog so the native call
ends and releases the wallet lock before or shortly after the UI gives
up.

`PriceError`'s request arm becomes `NetOpFailure`, built by
`classify_reqwest`. The success payload keeps carrying `via_socks5` (the
route attestation, consumed by zingo-mobile). Do not disturb that field.

### Broadcast fan-out

`fanout_broadcast`'s per-witness failures become `NetOpFailure` values,
target set to the witness host. The fan-out report becomes a vector of
typed attempts. The joined-prose rendering may remain as a Display on
top of the vector, for the existing consumers.

### Attach validation

The wallet's endpoint round-trip validation reports its failure as a
`NetOpFailure` with the appropriate early stage, so a `died` verdict
carries why. The mode enum itself does not change.

### The shim (optional follow-up)

`zingo-nym-proxy-ffi` may adopt the crate for `ProxyFfiError::Connect`
detail. Not required for this PR. The shim workspace path-dep must not
pull any new transitive dependency, which the zero-dependency rule
guarantees.

## Constraints

- Every new file starts with `#![forbid(unsafe_code)]` at the crate
  root. No exceptions, per the workspace invariant.
- CI is cargo-checkmate. Run `cargo fmt --check`, `cargo check
  --workspace`, `cargo clippy --workspace`, and `RUSTDOCFLAGS="-D
  warnings" cargo doc --workspace --no-deps` before pushing. Two known
  traps from this branch's history: rustfmt drift in merge resolutions,
  and public doc comments intra-doc-linking private items.
- Conventional commits, prose bodies, one topic per paragraph.
- The mobile app pins this branch. After pushing, zingo-mobile bumps its
  lock and rebuilds its native libraries. Coordinate through the pin
  comment in zingo-mobile's `rust/Cargo.toml`.

## Acceptance criteria

1. `zingo-net-diag` exists, std-only, `#![forbid(unsafe_code)]`, with
   unit tests for `chain_texts` and Display rendering.
2. A price fetch through a dead tunnel fails within 20 seconds with a
   `NetOpFailure` whose stage names the failing layer.
3. The fabricated-chain classifier tests cover every `NetOpStage`
   variant.
4. Fan-out and attach failures carry `NetOpFailure` values.
5. No Display concatenates its cause chain. A test renders a nested
   error through the mobile-style `source()` walk and asserts no
   repeated text.
6. `cargo fmt --check`, check, clippy, and doc all pass on both
   workspaces.
7. The `via_socks5` attestation field is unchanged and covered by an
   unmodified passing test.
