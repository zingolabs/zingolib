# Instruction for LLMs

## MUST DO ALWAYS

- When reporting information to me, be extremely concise and sacrifice grammar for the sake of concision.
- Never define by absence, not in documentation nor doc-comments.
- **Trait objects are STRICTLY FORBIDDEN.** Every spelling: `dyn Trait`,
  `Box<dyn Trait>`, `&dyn Trait`, `Arc<dyn Trait>`, and any trait object behind
  a type alias. Use a generic parameter, an `impl Trait` position, or an enum
  over the known implementors. Never add one. Existing ones are debt to retire,
  not precedent to follow. Ruled 2026-08-18.

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

### Rust

- Don't reach for `.clone()` to satisfy the borrow checker. Borrow or restructure first.
- Use `?` for propagation. Avoid `.unwrap()`/`.expect()` outside tests and throwaway code.
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
- Don't reformat, re-comment, or otherwise "correct" existing code you were only asked to touch lightly. Match what's there. Keep diffs scoped to the task.

### Tooling

- No heredocs (<< EOF, << 'EOF') in bash or other shell scripts. They're hard to read, break on escaping, and bury content that should be its own file. Use a real file, a templating step, or printf with explicit lines instead.

## Agent skills

### Issue tracker

Issues and PRDs live in **Linear** (Zingo Mobile team), managed via the
`linear-server` MCP tools. External GitHub PRs are **not** a triage surface. See
`docs/agents/issue-tracker.md`.

### Triage labels

Five canonical triage roles map onto Linear **statuses** (Backlog → `needs-triage`,
Todo → `ready-for-human`, Canceled → `wontfix`) plus two **labels**
(`needs-info`, `ready-for-agent`). See `docs/agents/triage-labels.md`.

### Domain docs

**Single-context**: one `CONTEXT.md` + `docs/adr/` at the repo root. See
`docs/agents/domain.md`.

### Pending designs

Ready-for-implementation designs live beside this file's other agent
docs. Current: `docs/agents/net-diag-design.md`, the shared network
failure taxonomy for the mixnet-covered operations (target branch
`nym_mobile_adoption`, PR #2527).
