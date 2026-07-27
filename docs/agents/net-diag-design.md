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

## Clearnet price fetch, test-gated

The production price fetch has no clearnet tier and must keep none (ADR
0011 amendment). Tests are exempt: exercising the Gemini payload
parsing, the insufficient-trades panic, and the classifier against the
live endpoint currently requires a live mixnet tunnel, which makes those
tests slow, flaky, and entangled with Nym weather.

Add a clearnet fetch path gated so it cannot ship: either
`#[cfg(any(test, feature = "clearnet-price-fetch"))]` on a separate
`get_current_price_clearnet()` in `zingo-price`, or the same gate on an
internal route parameter. The mobile workspaces must never enable the
feature. The function is for this repository's tests and diagnostics
probes only, and its doc comment says so. The always-on mobile flavors
additionally refuse app-side before the FFI, so even a misconfigured
build would not reach it there.

While in `zingo-price`: the current-price extraction indexes into the
sorted trades vector at a fixed position. A response with fewer trades
than expected panics. Replace the indexing with a typed
insufficient-data failure (a `PayloadDecode` stage fits), and cover it
with a fabricated short response in the tests the clearnet gate makes
cheap.

## The polling blackout

This section is a required part of the implementation, recorded here in
full because the field evidence is subtle and the fix is structural.

### What was observed

On 2026-07-26, on the zingo-mobile silent alpha, a price fetch through a
half-dead tunnel hung with no bound. The app's mixnet mode poll, which
had heartbeat every 30 seconds all session, went silent from 19:36:05 to
19:41:22. It resumed only when the hung fetch finally died with `tls
handshake eof` after roughly five minutes. A second fetch attempt froze
the polls again until the app was force-stopped. During the entire
blackout the app believed the transport was `ready`.

### Why it happens

The mobile FFI's `zec_price` holds the lightclient write lock across the
whole network fetch. The mode poll takes the read side of the same lock,
so it queues behind the hung writer. Every observer that could have
noticed the dead tunnel shares its choke point with the operation that
is stuck on the dead tunnel. The consequences chain: the liveness
question is never asked, `died` can never be reported, the wallet's
`ready` claim goes stale with no bound, and the app's auto-recovery
(which triggers on `died`) can never fire. The privacy property held
throughout. Fail-closed does not depend on the poll. Availability and
observability did not hold.

### Required remedies

1. The client timeout (acceptance criterion 2) bounds the blackout but
   does not remove it. Twenty seconds of frozen polling per hung fetch
   is tolerable. It is not the fix.
2. `update_current_price` must not hold the wallet write lock across the
   network fetch. Resolve the route under the lock, release it, perform
   the fetch, then re-acquire briefly to store the fetched price.
   Document the small race this admits (the route could die mid-fetch,
   which the fetch itself then reports as a typed failure).
3. Audit the other covered operations for the same coupling. The
   broadcast fan-out and attach validation also run under long-held
   locks. For each, either the lock is released across network waits or
   the operation's progress is observable through a side channel that
   shares no lock with it (zingo-mobile's DRAIN_PROGRESS idiom, an
   independent Arc the poller reads).
4. The mixnet mode query itself must never block behind a covered
   operation's network wait. If remedy 2 alone does not guarantee that,
   snapshot the mode into a lock-free or separately locked cell at
   transition points and serve queries from the snapshot.

### Additional acceptance criteria

8. A test drives an oracle fetch against a black-hole listener (accepts
   the TCP connection, never completes TLS) and asserts the mixnet mode
   query answers within one second throughout the fetch.
9. The same test asserts the fetch itself resolves within the client
   timeout as a typed `TimedOut` failure.

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
