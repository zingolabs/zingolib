# String-promotion census for the mixnet seam (ADR 0041 arc)

This census records every String in the nym/correspondent stack that the
2026-08-10 audit walked, with the structured type each should become and a
verdict on feasibility and benefit. It implements the ADR 0041 ruling that
the seam's vocabulary is typed, with strings surviving only at true wire
and FFI edges. Working type names below are unratified identifiers.

## Promote — feasible and beneficial

**Exit Node identity → `ExitNodeId` newtype.** *Implemented 2026-08-10.* Sites: `Reservation.node`,
`ExitPool.population`/`issued`, `clutch_nodes() -> Vec<String>`,
`HostedTransport.exit_node`, `ProxyState.exits`, `MixnetStatus.exits`,
`MixnetSlot::exits()`. One newtype ends the possibility of an exit
identity and a host string trading places, and gives the ledger a typed
key. Serde stays wire-compatible via a transparent representation.

**SOCKS5 endpoint → `std::net::SocketAddr`.** Sites:
`MixnetRoute::Mixnet(String)`, `HostedTransport.socks5_addr`,
`ProxyState.socks5_addr`, `MixnetStatus.socks5_addr`,
`MixnetSlot::Ready { socks5_addr }`, and the `&str` threaded through
`probe`/`select` survey calls. The attach path already parses to
`SocketAddr` to validate and then discards the type; promotion moves the
parse to the edge, and `Ready` becomes address-typed by construction.
Coordinate with PR #2665, which is amending `MixnetRoute`.

**Responsiveness class across the host seam → `ResponsivenessClass`.**
Site: `ProxyHost::start_transport(class: &str, …)` and the `class.wire()`
flattening in `HostedProxy::acquire`. The enum crosses the ADR 0041
request channel intact; `wire()` renders only inside the mobile FFI crate.

**Host refusals → typed `HostRefusal`.** Sites: `ProxyHost`'s
`Result<_, String>` returns, absorbed by
`TransportError::HostUnavailable(String)` and `HostRefused(String)`.
The ADR 0041 channel replies carry the typed refusal; prose survives only
as a display of the type, never as the type.

**Operator identity → `Operator` newtype.** *Implemented 2026-08-10.* Sites:
`sweep::operator_domain(&str) -> String` and its acknowledged mirror
`correspondent::operator_domain`; the census-level `Indexer::operator` is
the declared eventual owner. One newtype ends the duplicated derivation,
and the concurrent-Transmission correspondent ledger (open design) keys
by it.

**Indexer endpoint records → one host type.** Sites: `Health.standings`
keyed by `HashMap<String, _>`, `probe::ProbeSuccess.host`, and the
`server: String` diary fields. `http::uri::Authority` (or a thin `Host`
newtype over it) makes Health's key, the probe records, and the diary
agree by type rather than by convention.

## Keep as String — prose or foreign tokens, deliberately

**Bootstrap narration** (`bootstrap_detail`) and death/diary prose: these
are progress lines for humans; a type would add nothing.

**`UnknownMixnetModeToken(String)`**: a typed error whose payload is the
foreign token verbatim — already the right shape.

**`probe::ProbeSuccess.chain`**: the indexer's self-reported chain token,
compared but never interpreted; a `ChainToken` newtype is defensible but
low-benefit — promote only if it starts crossing module boundaries.
