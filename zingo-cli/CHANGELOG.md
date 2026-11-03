# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

### Changed

### Deprecated

### Removed

### Fixed

## [0.5.0] - 2026-10-28

Every entry below describes the change since 0.4.0, the last release: the
mixnet, the Connectivity Consent, the `network` family and the ZIP 318
migration commands are new since then, and the clap grammar, the error
contract and the server binding replaced what 0.4.0 shipped.

### Added
- Mixnet Mode (zingo-adrs zingolib/0011). A connected session runs the nym
  mixnet unconditionally and fails closed: Transmission and the price fetch
  travel the mixnet, nakednet carries sync alone, and no command routes a
  Transmission or a price fetch over nakednet. The `nym` feature carries it;
  the `--nym-proxy` flag, then `$ZINGO_NYM_PROXY`, then a `nym-proxy` bundled
  beside the binary, then the PATH lookup name the proxy binary, and the
  session resolves that path once, in zingolib's session driver, which the
  automatic sync recovery reuses. A `nym-proxy` that fails to start is
  reported with its path, the source that chose it, the cause, and a remedy
  fit to that source, and the `network on` success line carries the same
  path and source. The `nakednet-test-mode` feature suspends the go-online
  refusal of a build without the mixnet so a measurement can run, and
  announces itself at launch.
- Connectivity Consent (ADR 0032). A first boot is offline by design.
  `--online` consents for the session, `--remember-online` stores a standing
  consent beside the wallet so later sessions attach on their own,
  `--forget-online` removes it, and an explicit `--server` or a stored
  consent also takes a session online. A deliberate `--offline` session is
  unliftable: it offers no network-requiring command, the whole `network`
  family included; suppressed commands leave `help` and refuse if typed,
  with exit code 1 in one-shot mode, and the refusal names the only exit,
  relaunch without `--offline`. A launch notice states the contract. The
  suppression is granular where a family splits: `migration` keeps its
  stored-state subcommands (`plan`, `status`, `windows`, `cadence`,
  `cancel`), and `sync`, `drain`, and `split` keep their non-emitting
  subcommands. An unconsented session refuses network-requiring commands at
  the dispatch gate, and the refusal names `network on` as the consent act;
  the `network` family stays offered there, and `help` reflects the live
  posture in every session.
- The `network` family. `network status` reports the mixnet state, off,
  bootstrapping or ready. `network on` consents to go online, starts the
  `nym-proxy` child and routes transmissions through the mixnet. `network
  off` is a zero-emission teardown, not a mixnet toggle (ADR 0032): it stops
  the nym proxy, drops the Indexer connection, stops an in-flight sync
  between batches and waits for it, and returns only when teardown
  completes; the session drops to the unconsented posture, so `network on`
  re-consents, and the stored standing consent is untouched. `network probe`
  runs the `GetLightdInfo` liveness probe over the mixnet route and reports
  each leg's outcome: against the Session Indexer when no target is named,
  or a named `https` port 443 target; an Indexerless
  session is refused as offline, and a Session Indexer that is not `https`
  on port 443 is refused as an ineligible probe target. `network history`
  shows the per-indexer attempts this session recorded, naming a direct
  route `nakednet`; nothing is written beside the wallet for a later session
  to read.
- The ZIP 318 Orchard-to-Ironwood migration. `migrate` runs the scheduled
  migration in one interactive run. `migration` drives it step by step:
  `plan` computes the plan and prints its hash, sending nothing; `start`
  records consent to the plan with that hash and begins; `continue` drives
  one splitting step; `cadence` resets parts-per-window and redraws the
  schedule; `execute` sends every part owed right now in one spaced batch;
  `auto` transmits whatever the current window has due; `status` reports
  the balance, phase, part counts and coming windows; `windows` lists each
  window's block range, position and confirmations; `reconcile` checks the
  schedule against the chain and applies what is safe; `catchup` sends
  overdue parts now, spaced by the given seconds; `cancel` abandons the
  migration, keeping its confirmed parts. `drain` sends every Orchard fund
  into the Ironwood pool now, the immediate path, with `plan` previewing it
  and `now` sending; `split` splits Orchard notes into ZIP 318 part sizes,
  one round per call, with `plan` previewing the remaining rounds. The
  commands that sync first (`migrate`, `migration continue`, `migration
  execute`, `migration auto`) sync to the chain tip and return when the
  wallet is configured for continuous sync; a sync already running before
  the command is stopped for it and relaunched after it. `transmit`
  transmits calculated transactions to the Indexer.
- A dead sync recovers without the user when the error's typed
  recommendation allows it. A transient error relaunches sync against the
  Session Indexer. An error that condemns the server parks the session,
  because the Session Indexer is the only indexer and nothing redraws
  (zingo-adrs zingolib/0063). Three automatic recoveries may run back to
  back before the session parks on the reported error, and a completed
  sync restores the budget.
- `sync status` reports `total_blocks`, the block span from the wallet
  birthday to the last known chain height, beside `total_blocks_scanned`.
  Every other key, and the text `sync poll` prints, is unchanged; both are
  now rendered by `zingolib::perspective::sync_progress`.
- A seed phrase may arrive in the `ZINGO_SEED` environment variable when
  `--seed` is absent. A seed on the command line is visible in the host's
  process list for as long as the session runs, and in the shell history
  afterwards; the environment carries it to this process and its child
  alone. An explicit `--seed` still outranks the variable, and a blank
  phrase from either names no seed. The variable is read once, where the
  flag is read, and never logged.
- The prompt, `sync poll` and `sync await` report a sync task that panicked
  as "the sync task panicked" with the panic's message, where the process
  aborted before. `CommandError` gains `SyncPanicked(String)`.
- `transparent_gap_limit` to `settings` command.
- `init_tracing`, in the library, installs the process-global tracing
  subscriber and chooses where log output goes. The binary entry point
  calls it once instead of deriving the mode of operation a second time to
  decide that.
- `decline` drops the proposal that `send`, `send_all` or `shield` stored,
  the counterpart of `confirm`. A stored proposal holds sync paused until
  `confirm`, `calculate` or `decline` settles it (#3104); the help of the
  three proposing commands says so.
- The crate carries the package metadata crates.io requires (description,
  license, repository), so 0.5.0 is its first crates.io publication.

### Changed
- BREAKING: moves to the librustzcash NU7 release cohort zingolib pins, which
  sets the NU7 mainnet activation height, 3,543,405. `parse_viewkey` lists a
  P2SH viewing key item as `transparent` beside a P2PKH one, and a shielded
  unified address with a transparent receiver is rendered with that
  receiver.
- The `bucket_index` of a migration window (`migration` status's
  `upcoming_windows`, `window_timeline`) numbers the chain's ZIP 318 grid,
  whose boundaries sit three times as many blocks apart from NU7 activation,
  and the window times count 25 seconds a block from there (zingo-adrs
  zingolib/0056, amended). Below the activation nothing changes.
- BREAKING: `--server` names the Session Indexer, the one indexer a session
  holds, and has no default. An online session without it attaches to the
  chain's Default Indexer (`https://mainnet.zainod.zingolabs.dev:443` on
  mainnet), and a chain without a Default Indexer refuses the launch with a
  typed error naming the chain (zingo-adrs zingolib/0063). A value without
  a scheme completes to `https://`, and a value without a port to its
  scheme's default port (`:443` for `https`, `:80` for `http`), instead of
  `http://` and `:9067`; a plaintext regtest indexer needs an explicit
  `http://`. The launch-time selection among curated servers is gone (see
  Removed), so the indexer is bound at config time and the `--server` help
  says so.
- BREAKING: `change_server` takes `offline`, `default`, or a URI; the
  bare and empty forms, which bound an invalid URI, are refused at the
  parse (zingo-adrs zingolib/0063). Its example in the command help names
  `https://mainnet.zainod.zingolabs.dev:443` instead of `https://zec.rocks:443`.
- `servers`, an interactive-only command, prints the configured Session
  Indexer and probes nothing, where it printed the launch probe's ranking of
  the curated servers.
- Sync runs continuously: without `--waitsync`, the sync engine stays
  running after reaching the chain tip and scans newly mined blocks as they
  arrive. With `--waitsync`, sync shuts down once the wallet is up to date.
- New wallets use the default transparent address discovery (with gap
  limit) instead of the minimal discovery.
- `current_price` works while Mixnet Mode is switched off, fetching over
  nakednet as the toggle-off consents to (ADR 0011, amendment
  2026-08-26), and its output names the route the fetch traveled.
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
- Every dispatched command narrates its latest progress line to stderr
  every ten seconds while it runs (`PROGRESS_HEARTBEAT_INTERVAL`), and the
  transmit family every eight (`TRANSMIT_HEARTBEAT_INTERVAL`), so no
  command is silent past one interval. The narration lives at the dispatch
  seam, which reads every live progress side channel (transmit, migration
  batch, drain, split, and mixnet bootstrap). A command finishing before
  the first tick stays silent.
- A failed command renders its whole cause chain at the dispatch seam,
  one `caused by:` line per source link, over the sanctioned
  `zingo-net-diag` chain walk; the closing save's failure renders the
  same way.
- The commands whose failures are not yet typed now carry the failure
  itself across the dispatch seam instead of its outermost line, so the
  chain walk reaches their detail too. `quicksend` names the send
  refusal that stopped it, and `current_price` reports the whole price
  race, where both printed one bare summary line before.
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
  save and sync launch notices, and the trailer that `quit` prints while the
  wallet saves. Nothing may parse stderr (ADR 0031).
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
- A bare `save` refuses at the argument parse, since its sub-command is
  required; `exit` remains an alias of `quit`.
- The interactive prompt and one-shot mode parse through one grammar,
  exactly once, at the process boundary. The command channel carries parsed
  values, so no string is re-parsed inside the process.

### Removed
- The launch-time server selection: the `get_info()` probe of the curated
  indexer list, the ranked server list it produced, the failover list it
  kept, and the `servers` rendering of the ranking. A session binds one
  Session Indexer at config time, from `--server` or the chain's Default
  Indexer, in every build, and nothing probes a list (zingo-adrs
  zingolib/0063).
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

### Fixed
- `Failed to create lightclient.` renders the whole error chain, so a
  refused indexer connection names its transport cause (`bad uri`,
  connection refused, timeout) instead of the bare `gRPC client error.`
  layer.

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

