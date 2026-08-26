# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- A dead sync recovers without the user when the error's typed
  recommendation allows it. A transient error relaunches sync against the
  bound indexer. An error that condemns the server redraws the sync
  indexer with a Server-Selection Sweep, in which the failed indexer is
  surveyed again only when it is the sole candidate; the redraw serves
  only an online session with no pinned server. Three automatic
  recoveries may run back to back before the session parks on the
  reported error, and a completed sync restores the budget.
- A seed phrase may arrive in the `ZINGO_SEED` environment variable when
  `--seed` is absent. A seed on the command line is visible in the host's
  process list for as long as the session runs, and in the shell history
  afterwards; the environment carries it to this process and its child
  alone. An explicit `--seed` still outranks the variable, and a blank
  phrase from either names no seed. The variable is read once, where the
  flag is read, and never logged.


### Deprecated

### Added
- `transparent_gap_limit` to `settings` command.
- `init_tracing` installs the process-global tracing subscriber. The
  binary entry point calls it once instead of deriving the mode of
  operation a second time to decide where log output goes.

### Changed
- `help` lists `info`, `change_server`, and `current_price` among the
  commands that need no wallet, where it had listed them as wallet
  commands. None of the three reads wallet state: the first two reach the
  indexer and the third reaches the mixnet. The sections split on whether a
  wallet is needed, not on whether the network is.
- An interactive session exits with the code it earned. A prompt the user
  closed — by `quit`, Ctrl-C, or Ctrl-D — still exits zero, but one the
  terminal ended now exits non-zero and reports on stderr, where before
  every interactive session claimed success whatever had happened. A
  failure to open the prompt at all is reported the same way instead of
  panicking, and a history entry that cannot be recorded no longer ends
  the session, since it costs the user recall and not the command.
- The interactive prompt never contends with a running sync for the
  wallet lock. Its chain height now rides the same lock-free progress
  channel the sync indicator reads, and the prompt's dedicated height
  query is gone, so Enter returns a prompt immediately while sync scans.
- Every dispatched CLI command narrates its progress every ten seconds
  while it runs, where the diagnostic interval was two.
- **Breaking.** `network probe` wraps the single `GetLatestBlock` RPC, the
  same tip call the Server-Selection Sweep surveys with, and reports the
  tip height alone: there is no chain name in the reply to print.
- A failed command renders its whole cause chain at the dispatch seam,
  one `caused by:` line per source link, over the sanctioned
  `zingo-net-diag` chain walk; the closing save's failure renders the
  same way.
- The commands whose failures are not yet typed now carry the failure
  itself across the dispatch seam instead of its outermost line, so the
  chain walk reaches their detail too. `quicksend` names the send
  refusal that stopped it, and `current_price` reports the whole price
  race, where both printed one bare summary line before.
- **Breaking.** `--server` has no default value. An online session without
  the flag configures no indexer at launch; the Server-Selection Sweep
  binds one at startup for every online session, whether or not it syncs,
  so an `--online --nosync` session still has an indexer for later
  interactive sync and send. `--server` remains the explicit pin the sweep
  surveys and never substitutes.
- **Breaking.** The send-path vocabulary of ADRs 0036 and 0037 reaches the
  CLI's output grammar. The mixnet route report's JSON key `witness` is now
  `destination`, and `migrate auto`'s success key `broadcast` is now
  `transmitted`. Progress narration says `destination <host>` and
  `mixnet escalation`, and help text says transmit where it said broadcast.
- **Breaking.** The `--no-mixnet` flag is retired. A connected session
  runs the mixnet unconditionally and fails closed; clearnet carries
  sync alone. The clearnet server-selection sweep now compiles only
  under the non-default `clearnet-test-mode` feature, and a default
  build resolves its indexer from `--server` without probing.
- Every dispatched command now narrates its latest progress line to stderr
  every eight seconds while it runs (`PROGRESS_HEARTBEAT_INTERVAL`), so no
  command is silent past one interval. The narration moved from individual
  command bodies to the dispatch seam, which reads every live progress
  side channel (transmit, migration batch, drain, split, and mixnet
  bootstrap). The transmit-family cadence therefore tightens from thirty
  seconds to eight. A command finishing before the first tick stays
  silent, as before.
- **Breaking.** A deliberate `--offline` session is unliftable (ADR 0032).
  The session offers no network-requiring command, the whole `network`
  family included. Suppressed commands leave `help` and refuse if typed,
  with exit code 1 in one-shot mode, and the refusal names the only exit:
  relaunch without `--offline`. A launch notice states the contract. The
  suppression is granular where a family splits: `migration` keeps its
  stored-state subcommands (`plan`, `status`, `windows`, `cadence`,
  `cancel`), and `sync`, `drain`, and `split` keep their non-emitting
  subcommands.
- **Breaking.** An unconsented session refuses network-requiring commands
  at the dispatch gate instead of deep in a command body, and the refusal
  names `network on` as the consent act. The `network` family stays
  offered there, since `network on` is how consent is granted. `help`
  reflects the live posture in every session.
- **Breaking.** `network off` is a zero-emission teardown, not a mixnet
  toggle (ADR 0032). It stops the nym proxy, drops the Indexer
  connection, clears the Migration Broadcast Endpoint, and aborts
  in-flight sync, returning only when teardown completes. The session
  drops to the unconsented posture, so `network on` re-consents, and the
  stored standing consent is untouched. The clearnet-transmit act is
  retired: no CLI command routes Transmission or price-fetch over
  clearnet.
- The `servers` report is a Last Known report: it renders the launch
  probe's ranking from session state and never probes.
- **Breaking.** A failing command now renders exactly once, as `Error: …` on
  stderr, and one-shot mode exits nonzero. Both the message and the failure
  itself previously went to stdout with an exit code of 0, so a failed send was
  indistinguishable from a successful one except by parsing prose (ADR 0031).
- **Breaking.** Stdout now carries exactly one thing, the command's result. The
  in-band `{"error": …}` JSON objects that used to appear inside otherwise
  successful output are gone, and their content travels as a typed command
  error on stderr. A consumer that parses `{"error"}` out of stdout must read
  stderr and the exit code instead (ADR 0031).
- **Breaking.** An unknown command is now an error rather than a line of help
  text on stdout, so a mistyped command fails loudly instead of appearing to
  succeed (ADR 0031).
- Progress narration moved from stdout to stderr: the transmit heartbeat, the
  indexer probing notices, the save and sync launch notices, and the trailer
  that `quit` prints while the wallet saves. Nothing may parse stderr
  (ADR 0031).
- Command dispatch now runs from a static command table of async command
  bodies, and the crate crosses from sync to async at a single audited seam
  that a Clippy `disallowed-methods` rule enforces. This is an internal change
  that alters no command's output (ADR 0030).
- **Breaking.** Command-line parsing is clap's now. Every command and
  sub-command is a clap derive grammar: help and usage errors are generated,
  so the hand-written parser messages and their byte-stability are gone.
  Arguments arrive typed, with txids, server URIs, output scopes, and
  performance levels validated at the parse. A malformed one-shot
  invocation fails with clap's usage error and exit code 2 before any wallet
  work begins, where it previously booted the wallet first.
- **Breaking.** Command names are case-sensitive: the grammar knows
  `balance`, never `BALANCE`, where the old dispatcher lowercased names.
- **Breaking.** Session flags precede the command name: `zingo-cli --nosync
  balance` binds the flag to the session, and a flag written after the
  command is a usage error with exit code 2, where the old parser accepted
  session flags in any position. A send-family memo beginning with a dash
  needs the standard escape: `send <address> <zatoshis> -- "-memo"`, and a
  `messages` filter beginning with a dash rides the same escape.
- `zingo-cli --help` now lists every wallet command, and `help <command>`
  appends clap's generated usage and options to the command's description.
- `exit` is a clean alias of `quit`, where it previously printed an
  unknown-command error before exiting, and a bare `save` refuses at the
  argument parse, since its sub-command is required.
- The interactive prompt and one-shot mode parse through one grammar,
  exactly once, at the process boundary. The command channel carries parsed
  values, so no string is re-parsed inside the process.

### Removed
- **Breaking.** The `nym-diary` feature and the `--indexer-diary` flag are
  gone, because the indexer diary no longer touches disk. `network history`
  needs neither: it now shows the attempts this session recorded, and nothing
  is written beside the wallet for a later session to read.
- **Breaking.** The string-command plumbing left the library surface: the
  `Command` and `ShortCircuitedCommand` traits, `HelpCommand`, and the
  `get_commands`, `get_standalone_commands`, `get_wallet_commands`,
  `do_user_command`, and `do_user_command_result` functions are gone.
  Dispatch parses one clap derive grammar and matches its enum
  exhaustively. No string entry point remains (ADR 0030).
- **Breaking.** `is_interactive` and `log_file_path` left the library
  surface. Both existed only so the binary could choose where tracing
  output goes, and `init_tracing` now makes that choice inside the
  library.

## [0.4.0] - 2026-06-10

### Removed
- `regtest` feature: can still use zingo-cli in regtest mode with no features enabled using the '--chain regtest' flag. 
- `tor` flag. tor is no longer supported but will be replaced by nym in the coming release.

## [0.3.0] - 2026-06-05

### Changed
`remove_transaction` command - now only allows transactions with the new `Failed` status to be removed.

### Removed
- `resend` command: see zingolib CHANGELOG.md on `LightClient::resend`
- `send_progress` command

## [0.2.0]

