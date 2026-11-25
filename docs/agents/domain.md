# Domain Docs

How the engineering skills should consume this repo's domain documentation when
exploring the codebase. **Layout: single-context.**

## Before exploring, read this

- **`CONTEXT.md`** at the repo root.

`docs/adr/` is a submodule of zingo-adrs. Ignore its records always
(`AGENTS.md`, "MUST DO ALWAYS"). The records tell people why a rule exists,
and every rule an agent follows is in `AGENTS.md`, `CONTEXT.md`,
`docs/agents/` or `docs/sop/`.

If `CONTEXT.md` doesn't exist, **proceed silently**. Don't flag its absence;
don't suggest creating it upfront. The `/domain-modeling` skill (reached via
`/grill-with-docs` and `/improve-codebase-architecture`) creates glossary terms
lazily when they actually get resolved.

## File structure

Single-context repo (this repo):

```
/
├── CONTEXT.md
├── docs/adr/            (submodule: zingolabs/zingo-adrs)
│   ├── 001-some-org-decision.md
│   └── zingolib/
│       ├── 0001-some-decision.md
│       └── 0002-another-decision.md
└── ...
```

zingolib is a Cargo workspace with multiple crates but a single Zcash
light-wallet domain, so one root `CONTEXT.md` plus the `zingolib/` scope of
zingo-adrs covers it. If the project later splits into genuinely separate
domains, switch to a multi-context layout (a root `CONTEXT-MAP.md` pointing at
per-crate `CONTEXT.md` files) and update this file.

## Proposing a record

Records are never proposed in this repository. The
[zingo-adrs README](https://github.com/zingolabs/zingo-adrs#proposing-a-record) explains how, and a zingolib record goes in
the `zingolib/` scope there.

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a
hypothesis, a test name), use the term as defined in `CONTEXT.md`. Don't drift to
synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal. Either you're
inventing language the project doesn't use (reconsider) or there's a real gap
(note it for `/domain-modeling`).
