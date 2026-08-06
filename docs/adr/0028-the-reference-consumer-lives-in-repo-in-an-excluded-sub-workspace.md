# 28. The reference consumer lives in-repo in an excluded sub-workspace

Date: 2026-08-03

Status: draft — ratified in session, pending review

## Context

ADR 0024 converged three consumers — zingo-cli, zingo-mobile, and
zingo-pc — on a zingolib-owned surface, and rejected a shared
consumer-facing crate "while zingo-viewmodel remains unmerged." The
2026-08-03 grilling session ratified a fourth consumer with a charter
unlike the shipping three: a Reference Consumer, a Tauri desktop app
that exists to prove the converged surface sufficient. It holds no user
funds, and its own code is confined to a typed one-to-one command
projection of zingolib's surface, a spawn-or-attach provisioning
adapter (the one axis of consumer difference ADR 0011 permits), and a
renderer whose TypeScript types are generated from the minted Rust
types and pinned by the shared golden fixtures.

A reference consumer earns its keep by *when* it breaks. If it builds
against workspace HEAD in the same CI that gates zingolib's merges, a
surface-breaking change fails the pull request that introduces it. If
it pins a rev in another repo — the zingo-pc arrangement — it
discovers the break weeks later, reproducing the divergence decay the
2026-07-28 three-tree audit documented.

Two standing rules bear on placement. The no-new-dependencies rule
(2026-07-13) admits exceptions only by explicit ratification, and Tauri
carries one of the largest dependency trees in the Rust ecosystem,
including system webkit libraries on Linux. The no-new-languages rule
requires explicit consent before TypeScript enters this pure-Rust
repository.

## Decision

The Reference Consumer lives at `zingo-tauri/` in this repository, as a
plain directory holding its own cargo workspace, listed under `exclude`
in the root workspace manifest. It takes zingolib as a path dependency
across the workspace boundary, so it always builds against workspace
HEAD, and a change to zingolib's consumer surface must mend the
Reference Consumer in the same pull request. A dedicated CI job builds
the sub-workspace on a webkit-capable image; the root workspace's
lockfile, dependency set, and `cargo check --workspace` loop are
untouched. The job is advisory during bring-up and becomes a blocking
merge gate at the app's first green run, flipped by a one-line CI
change: a canary that can be red for unrelated reasons trains
reviewers to ignore it, while one that never blocks is decoration.

Three scoped rulings accompany the placement. First, the Reference
Consumer is exempt from ADR 0024's rule 7 — "zingolib at a git rev,
never a branch" — because that rule disciplines external consumers,
while this consumer's function is to track HEAD; the exemption extends
to no other consumer. Second, the no-new-dependencies rule gains a
ratified exception scoped to the `zingo-tauri/` sub-workspace and its
own lockfile, mirroring the shape of the nym-stack exception; nothing
from the Tauri tree may enter the root workspace. Third, TypeScript is
consented into the repository confined to `zingo-tauri/`.

Subtree and submodule machinery are deliberately absent. A
`git subtree split` is retroactive, so the option to promote the app
into a standalone repository with its full history is preserved at zero
present cost — and a split copy would not build anyway, because the
path dependency resolves only inside this repository. Submodules are
ruled out: they would move the app's history outside the repository and
reintroduce the two-pull-request divergence dance the Reference
Consumer exists to prevent.

## Considered options

Membership in the root workspace was rejected because it would land the
Tauri tree in the shared lockfile and make every developer's
`cargo check --workspace` demand system webkit libraries. A separate
repository pinning zingolib by rev was rejected because the canary
would fire late. A git submodule was rejected for inverting history
ownership. Installing subtree sync machinery now was rejected because
retroactive splitting preserves the same option for free, and a
two-way-synced mirror would forfeit the atomic-pull-request property
that justifies in-repo placement. Replacing zingo-pc's binding layer
instead of adding a consumer was considered out of scope: ADR 0024
already settled that question in favor of keeping neon with typed serde
payloads.

## Consequences

The repository ceases to be single-workspace and single-language, with
both departures confined to one directory. CI grows a job with a
webkit-capable image. Surface-breaking zingolib changes acquire a new
cost — the same pull request must mend the Reference Consumer — which
is the gate working as designed. zingo-viewmodel gains its designated
first tenant, weakening ADR 0024's "while unmerged" objection to a
shared consumer-facing crate. If the app is ever promoted to a product,
its directory splits out retroactively with history intact, and the
promotion decision — not this record — settles rev-pinning, packaging,
and store pipelines.
