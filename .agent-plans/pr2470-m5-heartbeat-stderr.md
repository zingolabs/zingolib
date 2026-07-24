# PR #2470 review M5: transmit heartbeats must not corrupt stdout

CLAIMED 2026-07-23 by the review-remediation session (M1 ff64da547,
M2 6b9d002de, M3 0936a66d2, M4 4b70ac582). Finding M5: the 30-second
transmit heartbeat wraps seven pre-existing commands and prints to
stdout in every build, so in one-shot command mode the progress lines
precede the JSON result on the same stream and break
`zingo-cli ... quicksend | jq` on any slow send or migration — a
default-build behavior change the PR body's preservation claim does
not carve out.

## File claims

- `zingo-cli/src/commands.rs` — the heartbeat emission target and its
  tests.

## Status

APPLIED (2026-07-23), uncommitted; commits are the user's to make. All
seven production emit closures (quicksend, quickshield, confirm,
transmit, migrate, migration start, migration catch-up) now print the
heartbeat to STDERR; stdout carries only command results, so one-shot
piping stays parseable however slow the transmission. The heartbeat
keeps firing in every build — the progress value is real for slow
clearnet migrations too — and interactive sessions see no difference
(both streams reach the terminal). `with_transmit_heartbeat`'s doc
states the stream contract; the injected-emit tests pin cadence and
content as before.

Verified: clippy clean on default and nym,nym-diary (all targets); 95
default-build zingo-cli tests green; an emit audit shows eprintln at
all seven sites and no stdout emitter remaining.
