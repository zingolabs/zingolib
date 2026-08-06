---
status: accepted
date: 2026-08-05
---

# CLI stdout carries only the result; failures and narration travel on stderr, and a failed one-shot exits nonzero

zingo-cli's string frontends printed everything to stdout — success JSON,
`Error:` prose, in-band `{"error" => …}` objects at twenty-two construction
sites, server-probing narration, and the quit/save trailer — and one-shot mode
returned success unconditionally, so a failed send exited 0 and was
indistinguishable from a successful one except by parsing prose. We decided
that stdout carries exactly one thing: the command's result. Every failure
travels as `Err(CommandError)` to the dispatch seam of ADR 0030, which renders
`Error: {e}` once, to stderr; one-shot mode exits nonzero on failure (0 for
success, 1 for any `CommandError`, finer codes deferred until a consumer
demonstrates the need). All Narration — the Transmit Heartbeat, server
probing, save and quit notices — goes to stderr, which carries no parse
contract. The in-band error objects are deleted, their content moved into
typed `CommandError` variants.

## Considered options

Blessing the status quo would have made the accidental shape the documented
one and left the machine channel unparseable. A `{"ok"}/{"error"}` JSON
envelope on stdout would have kept error data in the success channel by
design and forced an unwrapping layer onto every consumer; it was rejected as
re-blessing at the boundary exactly what the typed-error work removed inside.

## Consequences

This is a breaking change for any consumer that parses `{"error"}` from
stdout or asserts exit code 0: the known audit surface is the
cli-wallet-harness end-to-end work (PR #2453) and ad-hoc scripts. It ships
with a CHANGELOG entry marked breaking. It also settles, by construction,
that heartbeat and narration lines are presentation-only: nothing may parse
stderr.
