---
status: accepted
date: 2026-08-05
---

# The CLI crosses sync to async exactly once, dispatching from a static command table

`zingo-cli` accumulated some sixty-four `RT.block_on` call sites because every
command privately chose where the sync world ended. Any call path that stacked
two of them panicked with "Cannot start a runtime from within a runtime," and
review 4852724030 of PR #2592 reached exactly that panic in testing: `network
on`, already inside `block_on`, called the synchronous `select_servers()`
wrapper. We decided that command bodies are ordinary `async` functions
returning typed results, that the sync-to-async crossing happens at exactly
one seam — `do_user_command` for the string frontends, plus the startup driver
in `lib.rs` — and that a Clippy `disallowed-methods` rule bans `block_on`
everywhere else in the crate, so the invariant is enforced rather than
remembered.

We further decided that the dispatch registry is a `static` slice of
`CommandSpec` entries — name, help text, short help, a wallet-requirement
marker, and an async function pointer — replacing the
`HashMap<&'static str, Box<dyn Command>>` that was rebuilt on every dispatch.
The map shape was zecwallet-light-cli lineage, not a decision: the commands
are stateless fieldless structs, nothing outside the crate implements or
consumes the `Command` trait, `help` had to re-impose an ordering by sorting
because the map randomized it, and the `servers` command lived outside the
registry entirely, patched into the help listing by hand. The table gives
dispatch without allocation, `help` in declaration order, a registry home for
`servers`, and a spec-resident command name from which usage hints and
heartbeat labels derive instead of being re-typed at every site. The twin
standalone/wallet maps and the single-implementor `ShortCircuitedCommand`
trait dissolve into the marker field.

We further decided that when one command body needs another's behavior, it
calls that body directly as an async Rust function and never re-enters string
dispatch, which exists only at the seam. The sole such edge is `quit`, which
today reaches `save shutdown` through `do_user_command` — the crate's one
path that stacks two `Command` executions. That path avoids the
nested-runtime panic only by accident: the outer `exec` happens to hold no
`block_on` of its own. Under the table, `quit`'s body awaits the save-shutdown
function by name, so the compiler enforces the dependency and the hazard
class cannot reappear there.

A further grilling round refined the row type. A `CommandBody` enum —
`Standalone(fn(&[&str]) -> Result<String, CommandError>)`,
`Wallet(WalletRunFn)`, `Repl` — replaces the wallet-requirement marker field
and the `Option` around the run pointer, so a command's capability and its
body are one typed fact: a standalone body is synchronous and never receives
the wallet, and the REPL-owned `servers` cannot claim a body, where the
previous shape let the marker and the pointer contradict each other silently.
The `wallet!` and `standalone!` table macros mint each command's name exactly
once, via `stringify!`, from the body function's identifier, so the
dispatched name and the body cannot diverge; `servers` carries the one
literal name, having no body function to mint from.

## Considered options

A boxed-future `exec` method on the existing trait (K1) would have removed the
crash class but kept the fifty unit structs and traded `RT.block_on` ceremony
for `Box::pin` ceremony at every command. A nesting-detection helper around a
sync `exec` (K2) would have detected the class at run time instead of
eliminating it by construction. Both were rejected in favor of the table.

## Consequences

The twenty-five `Ok(RT.block_on(async move { … }))` scaffolds dissolve, and
everything below the seam (server selection, the nym command body, the
transmit heartbeat) becomes async-native and composable, which is what the
`network on` consent flow of ADR 0026 needs. The failure contract for string
frontends — what a failing command prints, and on which stream — is unblocked
by this decision but is a separate ruling, recorded when ratified.
