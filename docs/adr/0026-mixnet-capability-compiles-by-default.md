# 26. Mixnet capability compiles by default; activation stays a runtime consent

Date: 2026-07-29

Status: draft — ratified in session, pending review

## Context

Until this decision, zingo-cli's `nym` feature was off by default. A
contributor who ran the workspace's bare `cargo run` — a legitimate
launch, since zingo-cli is the workspace's only member binary — received
a build whose `nym on` answered with a compile-time refusal telling them
to rebuild. The 2026-07-29 session walked exactly that flow and asked
whether the proxy could instead sit compiled, adjacent, and inactive,
with `nym on` executing it at runtime.

The architecture already supports that shape. The heavy nym-sdk stack
lives solely in the `nym-proxy` binary, built from the excluded
`zingo-netutils` workspace (ADR 0011); zingolib's `nym` feature pulls
only the SOCKS5 client machinery, which resolves in the workspace
lockfile. Compiling the transport into the CLI therefore costs little
and links nothing that dials. The gate's remaining job was honesty about
provisioning: a build without a bundled proxy cannot promise a mixnet.
But that honesty already exists at runtime — provisioning failure is a
typed refusal at the go-online moment, never a silent clearnet (ADR
0024) — so the compile-time gate duplicated a guarantee the runtime
already owns, at the cost of a rebuild step in the most ordinary
contributor flow.

## Decision

`nym` is a default feature of zingo-cli: every ordinary build — bare
`cargo build`, `makers run-cli`, release packaging — carries the mixnet
transport, and enabling the mixnet never demands a rebuild. The opt-out
is explicit: `--no-default-features` on cargo, `--clearnet` on run-cli.

The REPL command renames from `nym` to `network`: there is exactly one
mixnet, so the transport's name is implicit, and the command's true
subject is the session's network posture. Its help and its output warn
about the next rule in capital letters, because it changes the session's
posture.

`network on` is itself a Connectivity Consent act, amending ADR 0025's
act list. An offline session that runs it switches to ONLINE MODE for
this session only — nothing is stored — resolving its indexer over the
same curated ranking `--online` uses at launch and only then, in the
launch order, bootstrapping the mixnet. The REPL's `change_server` pin
reads the live session rather than the launch snapshot, so the granted
consent also unlocks server changes. `network probe` grants nothing: it
emits traffic without expressing a go-online intent, so an offline
session refuses it with zingolib's single minted offline-refusal
string. The REPL-owned `servers` command still probes unguarded; that
gap remains open and tracked.

A build without the capability has no Online Mode at all: Offline Mode
is the only mode such a build can be in. The `network` command does not
exist there — no command may exist that could change the session's
posture — the online launch acts (`--online`, `--remember-online`, an
explicit `--server`) refuse loudly at startup, and a stored standing
consent is reported as inert. `--forget-online` still works, so an
opt-out build can retire a stored consent.

Runtime behavior otherwise remains governed by the existing rules. An
offline session executes no proxy and produces zero network activity —
compiling the capability starts nothing (ADR 0025). An Online session
provisions at its go-online moment, and a missing or failing proxy is a
typed refusal to the caller who expressed the go-online intent (ADR
0024).

## Consequences

A bare-cargo build whose user goes online will attempt mixnet
provisioning and refuse typed if no proxy is resolvable; the remedy is
`makers run-cli` (which bundles the proxy beside the binary) or an
explicit `--nym-proxy` path. This is the accepted trade: a runtime
refusal that names its remedy replaces a compile-time refusal that
demanded a rebuild.

The consent surface becomes asymmetric by design: the mixnet-capable
build offers four consent acts (`--online`, `--remember-online`,
`--server`, and the in-session `network on`), while the opt-out build
offers none. Clearnet-only operation is no longer a supported online
configuration — going online requires the mixnet capability compiled
in, and a session that wants clearnet transmits reaches them through
`network off` after a consented start.

CI's `--workspace` jobs now compile the CLI's gated code as a matter of
course; the `nym-feature` job keeps zingolib's gated tests (zingolib's
own default remains nym-off, so library consumers such as zingo-mobile
inherit nothing new) and pins the opt-out build's offline-only
refusals. The `--clearnet` path of run-cli
passes `--no-default-features` instead of omitting a feature flag.
