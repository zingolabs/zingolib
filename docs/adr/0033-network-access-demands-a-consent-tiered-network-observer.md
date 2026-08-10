---
status: accepted
date: 2026-08-05
---

# Network access demands a consent-tiered Network Observer

The Offline mode contract — zero traffic for the life of the session — is
today a promise kept by scattered runtime refusals: every network-reaching
`LightClient` method re-checks connectivity and returns
`LightClientError::Offline`, and the promise holds only where someone
remembered to check. The zingo-cli offline-contract tests document three
places nobody did: `nym on`, `nym probe`, and the REPL-owned `servers`
command can all emit traffic from an Offline session. We decided to replace
the promise with a constructibility argument: a network-reaching operation
demands a **Network Observer** as a parameter, the Observer is minted by
zingolib at the moment the corresponding grant actually happens, and a
session without the grant cannot construct one — so code that would emit
traffic without consent cannot be written. This is parse-don't-validate
applied to connectivity: the grant is parsed once, at the mint, and never
re-validated downstream.

The Observer is consent-tiered, because the domain's consent surfaces are:
a **Sync Observer**, minted where Connectivity Consent and a configured
Indexer meet, legitimizes only synchronization — the one surface clearnet
serves (the sync-only clearnet policy of ADR 0027, restated as a type); a
**Transmit Observer**, minted when Mixnet Mode reaches ready or when the
switched-off state records explicit clearnet consent, is demanded by every
send and broadcast; and the price fetch demands the Transmit Observer's
mixnet-ready sub-type, never its clearnet-consented form (ADR 0011). A
session holding only a Sync Observer therefore cannot transmit, whatever
code it runs, and an Offline session holds nothing.

zingolib owns the Observers and their minting sites, extending ADR 0024's
convergence doctrine to capabilities: every consumer — zingo-cli,
zingo-mobile, zingo-pc — inherits the theorem instead of hand-rolling its
own gate, which is how the ungated CLI paths arose. Two bounds govern the
implementation. It lands as its own pull request against dev, decoupled
from the CLI table refactor (PR #2626) and the planned clap conversion.
And it is minimally invasive: Observer parameters appear only on the
public network-reaching entry points (the sync starters, the send and
broadcast family, the probes, the price fetch); internal plumbing is not
re-threaded, and each entry point's runtime Offline refusal retires when
its signature converts, not before.

## Considered options

A single undifferentiated network capability was rejected because it would
let a body holding it transmit over clearnet, reducing ADR 0027 to runtime
checks inside the token's methods — the validate-style shape this decision
exists to eliminate. A CLI-side gateway wrapping `LightClient` was
rejected because a wrapper that can reach the whole client underneath
enforces nothing and leaves the other consumers exactly where they are.
Per-command capability markers in the CLI's spec table were rejected
because the capability boundary cuts at the subcommand level (`sync
status` is wallet-only while `sync run` is not), so row-level markers
would lie for the largest commands.

## Consequences

The scattered Offline refusal sites retire as their entry points convert,
and the offline-contract tests that pin refusal-by-runtime become
compile-time redundant, to be retired deliberately with the conversion
they cover. The CLI threads Observers at subcommand granularity, which
the clap conversion's typed subcommand variants make natural; that arc
therefore sequences first. The glossary's Network Observer entry
(zingolib/CONTEXT.md) is the vocabulary of record; "connection witness"
and "network witness" are retired working names.
