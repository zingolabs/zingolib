# String-promotion census for the mixnet seam (ADR 0041 arc)

This census records every String in the nym/correspondent stack that the
2026-08-10 audit walked, with the structured type each should become and a
verdict on feasibility and benefit. It implements the ADR 0041 ruling that
the seam's vocabulary is typed, with strings surviving only at true wire
and FFI edges. Working type names below are unratified identifiers,
except where an entry records its ratification: `ExitNodeId`,
`Operator`, and `HostRefusal` were ratified 2026-08-10.

## Promote — feasible and beneficial

**Exit Node identity → `ExitNodeId` newtype.** *Implemented 2026-08-10;
name ratified 2026-08-10.* Sites: `Reservation.node`,
`ExitPool.population`/`issued`, `clutch_nodes() -> Vec<String>`,
`HostedTransport.exit_node`, `ProxyState.exits`, `MixnetStatus.exits`,
`MixnetSlot::exits()`. One newtype ends the possibility of an exit
identity and a host string trading places, and gives the ledger a typed
key. Construction is checked — `ExitNodeId::parse` trims and refuses a
blank — so the invalid identity is unrepresentable. Serde stays
wire-compatible via a validated string representation.

**SOCKS5 endpoint → `std::net::SocketAddr`.** *Implemented 2026-08-10,
in two steps: first `SlotTunnel`, then every remaining wallet-held
site — `HostedTransport.socks5_addr`, `ProxyState.socks5_addr`,
`MixnetStatus.socks5_addr`, the test stand-in slot, and the
`probe`/`select` survey parameters.* The parse happens once where an
address enters (the child's announcement line, `attach_mixnet`'s
parameter, the typed host report), and the string renders only at the
zingo-netutils dial calls, whose `&str` parameters are a possible
future cross-workspace promotion outside this census's scope.

**Responsiveness class across the host seam → `ResponsivenessClass`.**
*Implemented 2026-08-10; deleted 2026-08-13.* The partition retired with
ADR 0044's single hedged acquisition policy, so no class token crosses
the seam any longer; `ProxyHost::start_transport` lost its `class`
parameter with the type.

**Host refusals → typed `HostRefusal`.** *Implemented 2026-08-10; the
name and its two-variant shape (`Failed` versus `Declined`) are ratified
2026-08-10 — see Host Refusal in `zingolib/CONTEXT.md`.* Sites:
`ProxyHost`'s `Result<_, String>` returns, absorbed by
`TransportError::HostUnavailable(String)` and `HostRefused(String)`.
The ADR 0041 channel replies carry the typed refusal; prose survives only
as a display of the type, never as the type. Endpoint defects are
deliberately not a variant: the wallet judges a host's reports itself.

**Operator identity → `Operator` newtype.** *Implemented 2026-08-10;
name ratified 2026-08-10.* Sites:
`sweep::operator_domain(&str) -> String` and its acknowledged mirror
`correspondent::operator_domain`; the census-level `Indexer::operator` is
the declared eventual owner. One newtype ends the duplicated derivation,
and the concurrent-Transmission correspondent ledger (open design) keys
by it.

**Indexer endpoint records → one host type.** *Implemented 2026-08-10
as the `Host` newtype (working name), chosen over `http::uri::Authority`
because an Authority carries a port while Health judges at host grain;
the indexer history's `exit` column rode along to `ExitNodeId`, which
required the `nym` module to be declared in every build so the identity
vocabulary reached it. That column was retired on 2026-08-17 with the
at-rest history it served, so nothing outside the `nym` feature names an
`ExitNodeId` and the module is gated again.* Sites: `Health.standings`
keyed by `HashMap<String, _>`, `probe::ProbeSuccess.host`, and the
`server: String` history fields. `http::uri::Authority` (or a thin `Host`
newtype over it) makes Health's key, the probe records, and the history
agree by type rather than by convention.

## Keep as String — prose or foreign tokens, deliberately

**Bootstrap narration** (`bootstrap_detail`) and death/history prose: these
are progress lines for humans; a type would add nothing.

**`UnknownMixnetModeToken(String)`**: a typed error whose payload is the
foreign token verbatim — already the right shape.

**`probe::ProbeSuccess.chain`**: the indexer's self-reported chain token,
compared but never interpreted; a `ChainToken` newtype is defensible but
low-benefit — promote only if it starts crossing module boundaries.
