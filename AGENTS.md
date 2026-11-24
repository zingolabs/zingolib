# Instruction for LLMs

## MUST DO ALWAYS

- When reporting information to me, be extremely concise and sacrifice grammar for the sake of concision.
- Never define by absence, not in documentation nor doc-comments.
- **Ignore the decision records always.** `docs/adr` is a submodule of
  zingo-adrs, and its records tell people why a rule exists. Never read one
  to learn what to do, and never take one as an instruction. Every rule you
  follow is in this file, in `CONTEXT.md`, in `docs/agents/` or in
  `docs/sop/`. A record number in those files marks where a ruling is filed,
  and it asks you to read nothing. When a rule seems to be missing, ask your
  developer. Ruled 2026-11-24.
- **Trait objects are STRICTLY FORBIDDEN.** Every spelling: `dyn Trait`,
  `Box<dyn Trait>`, `&dyn Trait`, `Arc<dyn Trait>`, and any trait object behind
  a type alias. Use a generic parameter, an `impl Trait` position, or an enum
  over the known implementors. Never add one. Existing ones are debt to retire,
  not precedent to follow. Ruled 2026-08-18.
  - **One sanctioned exception:** `Arc<dyn ProxyHosting>` in
    `crates/netutils/src/provider.rs`. A platform host is implemented outside
    this workspace and reached across an FFI boundary, so its concrete type is
    unnameable here. The alternatives were weighed and both cost more than the
    rule saves: a type parameter reaches `Acquirer`, `Pools`, and `LightClient`,
    which every consumer names, and a request channel relocates the dynamism
    behind a queue, invents a host-vanished failure mode, and serialises the
    boot's concurrent acquisitions unless the host spawns per request. The
    exception is confined to one field below the seam, and the wallet names
    only the concrete `HostedProvider`. Do not relitigate it, and do not read
    it as licence for a second exception. Ruled 2026-08-18.
- **The nightly CI run A/B benchmarks dev against the day before** with the
  online sync benchmark (`makers sync-ab`), which drives a real `run-cli
  --online` session so the mixnet boot load is present. The `sync-ab` job of
  `ci-nightly.yaml` compares the dev head with the newest first-parent commit
  older than 24 hours, uploads the report as an artifact, and skips a day
  that added no commit. A commit carries no benchmark numbers of its own. A
  regression in the nightly report is bisected over that day's commits with
  the same tool, and the fix carries the two numbers. Ruled 2026-08-18 as a
  per-commit rule, moved to the nightly run 2026-10-07.
- **The communication model is moving to events published from zingolib.**
  The wallet crate takes a callback interface from the consumer at startup
  and calls it with typed events (sync status, balance, new transactions,
  errors), so the consumer's timed polls become a subscription. Issue #2824
  tracks the arc and issue #2820 specifies the events. Every review and every
  implementation MUST weigh its change against that model: do not add a poll,
  a side channel around the lightclient lock, or a string-shaped status that
  an event will replace, and shape new state so an event can carry it. Ruled
  2026-10-02.
- **Code duplication is FORBIDDEN.** The convention is maximally DRY code
  with pure functions first: extract the shared logic into one pure function
  and call it from every site, and keep effects at the edges. Never add
  duplication. Duplication that was there before your change is deferred
  under "Pull request scope" below, unless your task rewrites its lines. A
  second copy of a helper, a branch, or a constant is a defect, in review
  and in implementation alike. Ruled 2026-10-02 and amended 2026-11-24. CI
  enforces it through the
  workbench binary `dupes-gate` (tools/workbench/src/dupes_gate.rs), which
  runs cargo-dupes with the settings that module pins. The gate rejects an
  exact group outside `.dupes-ignore.toml`, a near group count other than
  the ceiling `MAX_NEAR` in that module, and a baseline entry whose group
  is gone. Never add an entry for a new group, with two exceptions: a
  trait-delegation pair, where one type implements two traits whose
  methods delegate identically and cannot share an impl, takes an entry
  whose reason names the pair; and an FFI export pair, where the same
  call runs under the write lock storing its result and under the read
  lock previewing it (`send`/`preview_send`), takes one entry for the
  functions and one for their lock closures, each naming the pair. The
  gate erases identifiers, so it cannot tell the two apart (ruled
  2026-10-28, zingolib#3104). Otherwise remove the duplication, then run
  `dupes-gate cleanup` and commit the result. When a near group
  disappears, lower `MAX_NEAR` to the new count; the ceiling only moves
  down. No config file holds the settings, so run cargo-dupes through the
  binary: `cargo run --manifest-path tools/workbench/Cargo.toml --bin
  dupes-gate -- <cargo-dupes arguments>`.
- **A pull request merges into `dev` only when `dev` could be released the
  moment it lands.** Releases are cut from `dev` with no `stable` branch
  (zingo-adrs zingolib/0064), so a candidate can be taken on any day. `dev`
  is releasable when every public item works completely and no item exists
  in two forms: a dead code path, a type that carries an old field beside
  its replacement, a disabled test and an unfinished flag all fail. A larger
  feature or refactor lands as increments that each hold that bar, as "Pull
  request scope" below specifies, and only a person opens an integration
  branch `integration/<name>`. Every change writes its changelog lines in
  the same pull request, a dependency moves only to a version on crates.io
  or a git pin the team accepts shipping, and the `[X.Y.Z]` section of a
  cut changelog is never edited after the cut. The procedure is
  `docs/sop/SOP001-release.md`. Ruled 2026-11-02 in zingo-adrs zingolib/0064.

## Writing & Code Style

Goal: produce prose and code that reads as if written by a specific, competent human, not by a model. The point is naturalness and fit, not looking exhaustive or safe. When in doubt, commit to a choice and keep it short.

### Prose

#### Punctuation
- No em dashes. Use commas, parentheses, or separate sentences.
- No semicolons. Split into two sentences.
- Don't over-clarify with parentheticals. Cut the aside or fold it into the sentence.

#### Constructions to avoid
- The antithesis flip: "not X, but Y", "isn't just X, it's Y", "not only X but also Y". State the claim directly.
- Defaulting to groups of three (adjectives, clauses, list items). Vary the count.
- "From X to Y" fake-comprehensive sweeps.
- "Whether you're X or Y" catch-all wrap-ups.
- Forced analogies ("think of it like a...").

#### Openers and closers
- No throat-clearing: "It's important to note", "It's worth noting", restating the question before answering.
- No grandiose closers or zoom-outs: "In conclusion", "Ultimately", "At the end of the day", "in an ever-evolving world". Stop when the point is made.
- Don't chain connectives: "Moreover", "Furthermore", "Additionally", "That said".

#### Tone
- No sycophancy: "Great question", "You're absolutely right".
- Commit to a position. No false balance or manufactured symmetry between unequal options.
- Assert plainly. Cut reflexive hedging and over-qualification.

#### Vocabulary to avoid
- delve, tapestry, realm, landscape, navigate/navigating, leverage, robust, seamless, crucial, vital, pivotal, testament, boasts, nestled, foster, harness, unlock, elevate, embark, showcase, underscore, spearhead, treasure trove, game-changer, cheap, liveness, gap, shape, correctness, alive, honest, simple, probe, contact, stay.

#### Formatting
- Don't bold the lead phrase of every bullet.
- Don't bullet what should be prose.
- No headers on two-sentence sections.
- No emoji as section markers.
- Vary sentence length deliberately.

#### Articles and determiners

- Don't drop the definite or indefinite article before a noun to sound terse. Write "the spec was written", not "spec is written"; "the parser reads the manifest", not "parser reads manifest". This zero-article, telegraphic register is a strong machine-generated tell and often reads like translated copy.
- Watch the related tense slip: the clipped present where the past belongs ("spec is written" for "the spec was written", "add handler" for "we added a handler"). That is commit-message and changelog phrasing leaking into prose. Use natural past tense for things that happened.
- The exception is genuine fragment formats (bullet labels, short table cells) where an article would just be noise. Everywhere else, use full grammatical sentences with their articles intact.

#### Cadence and register

- Describe, don't sell. Use a neutral, declarative register. Avoid the promotional cadence of landing-page and ad copy.
- Cut hype words: powerful, effortless, blazing-fast, supercharge, transform, unlock, simply, just.
- Don't pitch benefits at the reader ("you'll love how fast it is", "say goodbye to X", "no more Y"). State what the thing does and let it stand.
- Avoid the staccato rhythm of short fragments stacked for impact, exclamatory energy, and calls to action. That cadence is built to persuade, not to inform.

### Code (all languages)

- Comment why, not what. No line-by-line narration of obvious operations.
- No tutorial narration ("Now we...", "Step 1:", "First, let's...") and no banner comments (`// ===== HELPERS =====`).
- No docstrings that just restate the signature.
- Names: concise and domain-specific. Avoid generic placeholders (`data`, `result`, `output`, `item`, `value`, `temp`, `handleData`, a helper named `helper`) and avoid over-long descriptive names where a short one is idiomatic.
- No completeness theater: no unrequested demo/usage blocks, no logs narrating execution ("Starting...", "Done!"), no emoji in output, no unprompted complexity analysis in comments.
- Never use magic numbers, anywhere. Every bare literal gets a name: prefer the most private binding the context allows, and a named constant where privacy can't confine it. Derive new constants from existing named constants rather than repeating a number.
- Don't add guards for conditions that can't occur. Don't wrap non-throwing code in try/catch. Don't swallow-and-log errors; let them propagate.
- Match the surrounding codebase's idioms and conventions over textbook-uniform formatting.

### Documentation in code

A doc-comment holds only a succinct bulleted list of the item's side effects.
The rule covers every doc-comment form: Rust `///` and `//!`, and file- or
module-level blocks.

- Each bullet names one side effect: a write to disk, a network call, a lock
  taken, a mutation of shared state, a spawned task, a panic.
- A doc-comment holds no summary sentence, no parameter or return
  description, and no restatement of the signature.
- A doc-comment references no ADR, issue, or other document.
- An item with no side effect carries no doc-comment.

### Rust

- Don't reach for `.clone()` to satisfy the borrow checker. Borrow or restructure first.
- Use `?` for propagation. `.unwrap()` is disallowed: never write one, and replace every one a change touches. Avoid `.expect()` outside tests and throwaway code.
- Every `.unwrap()` or `.expect()` a change touches is considered, never carried along. Touching means adding, moving, rewriting, or keeping one on a line the diff alters, in test code as much as in shipped code. For each one, present the options for explicit `Err` handling and their trade-offs: propagate with `?` into a typed error, restructure so the failure is impossible by construction, match the failure and define the fallback, or keep an `.expect()` whose message states the invariant that makes it unreachable. Every `.expect()` MUST state its invariant as its argument, the fact that makes the failure unreachable, never a description of the failure. An `.expect()` with an explicit invariant is acceptable in test code. A touched 'expect' that stays does so by the reviewer's choice, with its invariant written where the reviewer sees it. This applies in implementation and in review alike.
- Use tail expressions. No explicit `return` on the final line.
- Don't annotate types the compiler infers (`let x: i32 = 5;`).
- Prefer `if let` and combinators (`map`, `and_then`, `ok_or`, `unwrap_or_else`) over verbose `match` when clearer.
- Prefer iterator chains over manual `for` + `push` where idiomatic.
- Use `&str` where a borrow suffices instead of `String`.
- Run `makers feature-sweep` before you push. An ordinary `cargo check` compiles one feature combination, so a rename or a signature change can leave code behind a `#[cfg(feature = ...)]` gate broken and still look green; the sweep checks the touched crates in every combination, as CI does.

### TypeScript / React

- No `any`. Type precisely. Don't annotate what TS already infers. Don't use `as` to silence the checker.
- Prefer union/literal types over enums where idiomatic. Prefer named exports.
- Don't use `React.FC`. Type props directly.
- Don't wrap everything in `useMemo`/`useCallback`. Use them only for a real identity or perf need.
- Don't reach for `useEffect` to compute derived state. Derive it during render.
- No `console.log` narrating execution.
- Don't over-componentize trivial markup, and don't prop-drill where composition or context fits.

### HTML / CSS

- Use semantic elements. Avoid div soup.
- Keep class lists purposeful and legible. Don't pad with utilities that don't do anything.

### For agents

- Before finishing a task, scan what you wrote against this file. Focus on the high-signal tells, not a full re-audit: antithesis flips and narrating comments in prose, `.clone()`/`.unwrap()` spam and explicit trailing `return` in Rust, `useEffect` for derived state and `any` in TS.
- Verify your *new* output fits these rules and the surrounding code's style. The question is "does what I added fit", not "does this whole file now obey CLAUDE.md".
- Don't reformat, re-comment, or otherwise "correct" existing code you were only asked to touch lightly. Match what's there. Keep diffs scoped to the task, as "Pull request scope" below specifies.

### Tooling

- No heredocs (<< EOF, << 'EOF') in bash or other shell scripts. They're hard to read, break on escaping, and bury content that should be its own file. Use a real file, a templating step, or printf with explicit lines instead.

## Pull request scope

A pull request makes one semantic change. Coherence is the rule, and size is
only a tripwire. Ruled 2026-11-24, tracked in issue #3222.

Four terms carry the rule.

- An **arc** is a change too large for one pull request.
- An **increment** is one pull request of an arc. It can add code or remove
  code, and it leaves `dev` releasable.
- A **follow-up** is an issue for work that a pull request found and left
  out.
- The **tripwire** is 450 added lines. Above it the author states why the
  pull request cannot be split.

### One semantic change

Apply three tests before you open a pull request.

1. State the change in behaviour as one sentence. If the sentence needs
   "and" to join two behaviours, split the pull request.
2. A pull request typed `refactor`, `test`, `docs`, `ci`, `build` or `chore`
   changes no shipped behaviour, and no existing test assertion changes in
   it. A public API break marked `!` is allowed. A change a running program
   can observe is a change of behaviour, and error text and log text are
   observable. When a refactor needs one, the pure refactor goes first as
   its own pull request.
3. A `fix` or a `feat` holds one change of behaviour and only the
   refactoring that change cannot compile without.

### Deferral

Deferral is the default. Fix in the pull request only what a CI gate rejects
and what sits on a line the task must rewrite. Every other defect or rule
violation you find is a follow-up. A follow-up records something you can
point at, such as two unwraps in a named function, and never a preference
about how a module is organised.

A follow-up describes `dev`. Name the item as `dev` holds it, so the issue
is true for a reader who never saw the pull request and stays true after the
pull request merges or closes. A defect that exists only on the branch of an
open pull request is never a follow-up.

Draft each follow-up in the form `docs/agents/issue-tracker.md` gives and
show the list to your developer. File one only after your developer grants
it. The grant can cover a session, and it is never assumed.

### Arcs and increments

Each increment lands on `dev` as soon as it is ready, so the work of
independent contributors does not diverge. A commit is releasable when every
public item works completely and no item exists in two forms. A type that
carries an old field beside its replacement fails. So does a function kept
beside its successor with no caller, a method that is declared and
unimplemented, a disabled test, and a flag that gates unfinished code.
Sibling items in different styles are allowed while the arc is in progress:
one method of a trait can return a domain type while the others still
return generated types.

Before the first increment, open one tracking issue for the arc, labelled
`tracking`, under the same grant as a follow-up. It states the end state and
lists the increments as a checklist. It names the increment that makes an
uneven public surface uniform, which is what lets `dev` hold that surface
meanwhile. Every increment names it on a `Part of:` line. Rebase the
remaining increments onto `dev` after each one lands.

Never open an integration branch on your own judgement. If no increment can
hold the guarantee, stop and tell your developer which increments you tried
and why each fails. A person opens the branch.

### When the scope is decided

1. Before you edit a file, write the one sentence and estimate the crates
   and the public API the task touches. If the task fails the first test or
   the estimate passes the tripwire, answer with a list of increments, one
   sentence each in landing order, and wait for your developer to approve
   it.
2. While you work, stop when the diff crosses the tripwire or when a
   refactor turns out to need a change of behaviour. Report what you have
   and propose the split. Do not finish first and split afterwards.
3. When you open the pull request, fill in every field of the template.

If your developer tells you to keep one large pull request, comply, and
write the developer's reason under "Why this cannot be split".

### The tripwire

The count is the lines added between the merge base with the target branch
and the head, with rename detection on. `Cargo.lock`,
`bindings/CHANGELOG.md`, `.dupes-ignore.toml` and every Markdown file are
left out. A mechanical rename is left out when the template declares it on
a `Renames:` line as `old -> new`. Perform a symbol rename with
rust-analyzer, and declare only a new name that the base tree does not
already hold.

### Review

A reviewing agent applies the three tests before it reads for defects. A
failed test is its first finding, and the finding names each increment and
the order they land in. For an increment, read its tracking issue first to
learn where the arc goes, and judge the increment on whether it is whole. A
public surface in two styles is a finding only when the tracking issue names
no increment that makes it uniform.

A defect the pull request introduces is fixed in the pull request. So is any
other reason the pull request should not merge as it is. Write each as a
review comment on the pull request, and never file one as an issue. An issue
is for work that can wait until after the merge.

Every other finding is written as a proposed follow-up, never as a request
to change the pull request. The one sentence of the pull request decides the
boundary: the same bug in a neighbouring function is a follow-up, unless the
sentence claims to fix the bug everywhere.

When you answer a review, the stop rule above applies to your fixes.

## Changelogs

Each crate's `CHANGELOG.md` keeps a Keep a Changelog `[Unreleased]` section.
An entry there describes the change a consumer sees between the crate's last
release and the next one, never a step between two unreleased states. When a
later change on `dev` supersedes, reverts or reshapes an unreleased entry,
rewrite that entry to the net result rather than appending a second one; an
entry whose subject arrived and left again within the same unreleased span is
removed. The release itself then needs no editing beyond the version
heading. `bindings/CHANGELOG.md` is generated from the audited crates'
changelogs per publication and is never edited by hand.

Before adding an entry, do this, and a reviewer checks it:

1. Read the crate's whole `[Unreleased]` section. If the subject of the
   change already has an entry (the same command, flag, function, type,
   constant or dependency), edit that entry; a second entry about one
   subject is the fault this rule exists to prevent.
2. Name the last release: the heading under `[Unreleased]`, whose tag is
   `<crate>_v<version>` (`zingolib_v7.0.0`, `pepper-sync_v0.6.0`). State the
   change against that tag's code and behaviour, which `git grep <pattern>
   <tag> -- <crate>` answers; "where it used to" and "no longer" refer to
   the release, never to an earlier commit on `dev`.
3. A subject that did not exist at the release is an addition, however many
   times `dev` reshaped it since; describe its final shape under `Added`,
   not its history under `Changed`. A subject the release had and the
   change removes goes under `Removed`, once.
4. A pull request that changes behaviour an unreleased entry records
   rewrites that entry, in the same pull request, even when the entry
   belongs to another author's change.

## Agent skills

### Issue tracker

Issues live in **GitHub Issues** on `zingolabs/zingolib`, managed via the `gh`
CLI. Linear holds the Zingo Mobile team's issues only. Pull requests are
**not** a triage surface. See `docs/agents/issue-tracker.md`.

### Triage labels

Five canonical triage roles are GitHub **labels** (`needs-triage`, `needs-info`,
`ready-for-agent`, `ready-for-human`, `wontfix`), with `wontfix` also closing
the issue as not planned. See `docs/agents/triage-labels.md`.

### Domain docs

**Single-context**: one `CONTEXT.md` at the repo root. Decision records sit
under `docs/adr/zingolib/`, and an agent ignores them, as "MUST DO ALWAYS"
rules. See `docs/agents/domain.md`.

### Pending designs

Ready-for-implementation designs live beside this file's other agent
docs. Current: `docs/agents/net-diag-design.md`, the shared network
failure taxonomy for the mixnet-covered operations (target branch
`nym_mobile_adoption`, PR #2527).
