# Releasing from dev

Status: draft, 2026-10-29. Nothing below binds until the team has agreed it
and the decision to retire `stable` is recorded in zingo-adrs. Open
questions are collected at the end.

## What changed and why

Until zingolib 8.0.0, every crate release was cut on `stable`. That branch
let a hotfix for the last release ship without the half-finished work on
`dev`, at the cost of a back-port for every fix a consumer needed and a
release pull request per crate group. By the time zingolib#2985 merged the
two branches they had diverged by 410 commits, and zingolib/0062 found that
no product of the project depended on `stable` any more.

zingolib 8.0.0 was the first release cut from `dev`. From here on `dev` is
the one long-lived branch. A release is prepared on `dev`, tested as a
release candidate, and published from the commit the candidate names. When
a candidate needs a fix, a short-lived release line branches from the
candidate's tag so the fix ships without whatever landed on `dev` since.

The trade is that `dev` now carries the stability promise `stable` used to
carry. The last section states what that asks of every pull request.

## Terms

A release is one or more of these, each done from one release commit:

- a crate version published to crates.io, for the eight crates the
  workspace publishes: `zingolib`, `pepper-sync`, `zingo-netutils`,
  `zingo-net-diag`, `zingo-memo`, `zingo-status`, `zingo-price` and
  `zingo-cli`.
- a signed annotated tag per released crate, `<crate>_vX.Y.Z`, whose
  message is `<crate> release X.Y.Z`.
- a publication of the Binding Layer, which zingolib/0062 defines as the
  Prebuilt Bundles at a Published Commit in `ghcr.io`, named by the commit
  and never by a version.

The release set is the group of crates one release cycle bumps together.
A cycle has a lead crate, the one whose version names the candidate tags
and the release line. For a cycle that releases `zingolib`, the lead crate
is `zingolib`.

A release candidate is a commit tagged `<lead crate>_vX.Y.Z-rc.N`, counted
from 0. The candidate is of the whole set, so it carries one tag, on the
lead crate, while the release carries a tag per crate. The first candidate
of a cycle is always a commit of `dev`. A later candidate is a commit of
the cycle's release line.

A release line is a branch `release/<lead crate>_vX.Y.Z` created from the
first candidate's tag, and only when that candidate needs a fix. It holds
the fixes and the later candidates, and it merges back into `dev` after
each candidate is tagged. It is deleted once the release is published and
its last merge into `dev` has landed. Every release has a line of its own,
a patch release included, so the name always says which version is in
flight and the `release/` namespace holds nothing but lines.

## The procedure

### 1. Choose the release set and the versions

Read every crate's `[Unreleased]` changelog section and decide which crates
have something to release. A crate with an empty section is left alone.

Decide each bump by Semantic Versioning, with these house rules:

- A `BREAKING` marker in the changelog forces a major bump, or a minor
  bump for a crate still at 0.y.z.
- A bump of a public dependency's major version is breaking for every crate
  that exposes that dependency's types. The zcash cohort pinned in the root
  `Cargo.toml` is the usual case, and no tool sees it, so the changelog
  markers decide.
- `cargo-semver-checks` confirms a crate's own API against the previous
  release. Run it from a detached worktree of the candidate commit with
  `--baseline-rev <crate>_v<previous>`. `zingo-netutils` needs
  `--only-explicit-features`, because the baseline's `nym` feature does not
  build on its own.

A crate that depends on a bumped crate is bumped too, since its manifest
pins the dependency's version. Cargo orders the set by dependency at publish
time, so the order needs no thought here. The current order, read from a
dry run on 2026-10-29, is `zingo-memo`, `zingo-net-diag`, `zingo-status`,
`zingo-netutils`, `pepper-sync`, `zingo-price`, `zingolib`, `zingo-cli`.

### 2. Open the release pull request against dev

One pull request from a branch `prep/<lead crate>_vX.Y.Z`, outside the
`release/` namespace since it is not a line, with one commit titled
`release <crate> vX.Y.Z[, <crate> vX.Y.Z ...]`. It carries, for every crate
in the set:

- the `version` in the crate's `Cargo.toml`.
- the matching `version` in the root `[workspace.dependencies]` entry, which
  every workspace dependency declares beside its `path`.
- the changelog: the `[Unreleased]` section is renamed `[X.Y.Z] - <date>`
  with its empty headings dropped, and a fresh empty `[Unreleased]` skeleton
  is placed above it.

Then `cargo update --workspace --offline` refreshes `Cargo.lock`, and the
checks below run before the pull request is marked ready.

The pull request opens a tracking issue for the cycle, `release <lead
crate> vX.Y.Z`, which lists the evidence the cycle wants before its
candidate becomes the release: the automated runs at the candidate, and
the reports expected from whoever tests its artifacts. Each report lands
as a comment, the fixes of step 6 link to it, and the release tag closes
it. The list is the cycle's own. It shrinks as a workspace test takes over
what a manual check used to cover, and this procedure does not change
when it does.

The version and `path` pair is what makes a single pass work. Cargo strips
the `path` when it packages a crate and keeps the version requirement, so a
dependent crate can name the version its dependency is about to become
before that version exists on crates.io. Cargo 1.90 added workspace
publishing, which this workspace's 1.99 toolchain has: `cargo publish
--workspace` orders the crates by dependency, verifies each against a local
overlay of the ones packaged earlier in the same run, and uploads them in
order, waiting for each to appear in the index before the next. The
multi-round bump that releasing from `stable` needed is no longer required.

The changelog section takes the date of the cut, not of the eventual
release. The first candidate may become the release commit unchanged, and a
later edit of the date would force a new commit for nothing.

### 3. Pre-release checks

All of these run on the release pull request's head, locally or in CI.

- The pull request's checks pass. They are the same checks every pull
  request into `dev` runs.
- The publish dry run passes for the whole set in one command:

  ```sh
  cargo publish --workspace --dry-run
  ```

  The Binding Layer crates carry `publish = false`, since zingolib/0054
  rules they are never published to crates.io, so cargo passes them by.
  The dry run reports a crate whose version already exists on crates.io as
  a warning, so a crate left out of the set is passed with `--exclude`
  rather than relied on to be skipped.
- `cargo-semver-checks` agrees with every bump, as in step 1.
- `Cargo.lock` holds no yanked version. The dry run warns about each one
  it finds, and `cargo update <crate>` moves past it.

### 4. Merge and tag the first candidate

The release pull request merges into `dev` like any other. The merge commit
is the first candidate. Tag it `<lead crate>_vX.Y.Z-rc.0` with a signed
annotated tag whose message is `<lead crate> release X.Y.Z-rc.0`, and push
the tag by its full ref, `refs/tags/<tag>`, so a branch of a similar name
cannot shadow it.

`dev` does not stop. Work keeps merging after this point, and every later
pull request writes its changelog lines under the new `[Unreleased]`
heading and leaves the `[X.Y.Z]` section as it was cut. A change to that
section after the cut is a review defect.

### 5. Publish the candidate to consumers

Consumers test a candidate from the artifacts they will ship, so each
channel is published at the candidate commit:

- The Binding Layer is published by dispatching the publish workflow at the
  candidate's commit:

  ```sh
  gh workflow run publish-bindings.yaml --ref dev --field commit=<rc commit>
  ```

  The run pushes both Prebuilt Bundles to `ghcr.io` tagged by the commit,
  and opens the record pull request that `bindings/published.toml` and
  `bindings/CHANGELOG.md` take. Merge that record. A consumer pins the
  Published Commit with its Bundle Pin and tests it under its own plan.
- `zingo-cli` is tested from a checkout of the tag with `makers run-cli`,
  online and against mainnet, for whatever the tracking issue asks.

The candidate becomes the release when the lead judges the evidence on the
tracking issue sufficient. The decision is zingolib's. Evidence is of three
kinds: the candidate's own pull request checks and the nightly runs that
include it, the workspace test suite run at the candidate, and reports from
whoever tested the candidate's published artifacts. A report is positive
and named: who ran what against which commit, and that it passed. Silence
is not a report, since no issue found means nothing when nothing was run,
and a candidate with no report is not a release.

A consumer's report is input and nothing more. No consumer holds a veto
over the window, and no consumer's silence holds it open. The window has
no fixed length, and the lead may close it with the evidence in hand or
wait for a report that is still coming. Either way the tracking issue
shows what the release was judged on.

As of late 2026 the reports come mostly from zingo-mobile, which pins the
candidate's Published Commit, tests the app by hand on both platforms, and
ships a public beta from it. That is where the evidence comes from today,
not a dependency of the procedure. The aim is that the workspace's own
tests cover what a phone tester checks, and each cycle's issue drops a
manual item as a test takes it over.

### 6. A candidate needs a fix

Create the release line from the candidate's tag, and only now:

```sh
git switch -c release/<lead crate>_vX.Y.Z <lead crate>_vX.Y.Z-rc.0
git push -u zingolabs release/<lead crate>_vX.Y.Z
```

Every fix is a pull request into the release line, reviewed and checked
exactly as a pull request into `dev`. Each fix writes its changelog lines
into the `[X.Y.Z]` section, because that is the version it ships in. Nothing
is cherry-picked from `dev` onto the line unless it is the fix itself, and
nothing merges from `dev` into the line at all. The line exists to exclude
what landed on `dev` since the candidate.

When the line holds every fix the window asked for, tag its head
`<lead crate>_vX.Y.Z-rc.(N+1)`, then open the pull request that merges the
line into `dev`. That merge is required before the next step, for two
reasons: the fix must reach `dev`, and the Binding Layer publishes only a
commit that `dev` reaches. The merge is a true merge commit, never a squash
or a rebase, since either would rewrite the fixes into new commits that the
tag does not name and `dev` would not reach the candidate. The merge
resolves the changelog by keeping the fix's lines in the `[X.Y.Z]` section,
where the line put them.

The publication record and its tooling are required to accept the
candidate at this point. Today `binding-publish` refuses it whenever a
commit of `dev` was published between `rc.0` and the merge, because the
record's last entry must be an ancestor of the new commit, and the
changelog generator takes each entry's `since` from the previous entry.
zingolib#3113 specifies the change: the plan accepts a commit that `dev`
reaches and that descends from any earlier entry, and an entry's `since`
is its nearest recorded ancestor. This procedure does not function until
that change lands.

Repeat step 5 at the new candidate, naming the candidate's own commit in
the dispatch. The merge commit holds everything `dev` gained since `rc.0`
as well, so publishing it, or the head of `dev`, would hand consumers the
work the release line exists to exclude. The candidate's commit is the same
object on both branches, and its tree holds `rc.0` plus the fixes alone.
The release line is where every later candidate of the cycle is tagged.

### 7. Tag the release

The release commit is the commit of the last candidate, the one the window
closed on with no fix outstanding. It is the `rc.0` commit on `dev` when
nothing needed fixing, and the last `rc.N` commit on the release line
otherwise. Nothing is committed between the last candidate and the release,
so the bytes a consumer tested are the bytes that ship.

Tag the commit once per crate in the set, `<crate>_vX.Y.Z` with the message
`<crate> release X.Y.Z`, signed and pushed by full ref.

### 8. Publish

From a clean checkout of the release commit:

```sh
cargo publish --workspace
```

with every crate outside the set passed as `--exclude`. Cargo publishes
the set in dependency order and waits for each crate to be indexed before
the next.
The crates.io token is the publisher's own for now. Moving the publish into
CI under crates.io Trusted Publishing is listed under the open questions.

The Binding Layer needs no second publication when the release commit is a
candidate that step 5 already published, because the Published Commit is the
identity of a bundle and the registry already holds it. A consumer that
pinned the candidate's commit is already on the release. When the release
commit was never published, publish it as in step 5.

Create the GitHub Release for the lead crate's tag, as was done for 8.0.0,
with the crate's changelog section as its body.

### 9. Close the cycle

- Confirm the release line's last merge into `dev` has landed, then delete
  the line.
- Move zingo-mobile's Bundle Pin to the release commit if the mobile cycle
  calls for it. That is zingo-mobile's own release procedure, not this one.
- Publish the Bundle Pin package if `bindings/pin` changed since its last
  version. The push to `dev` does this on its own.

## A hotfix after the release

A defect in a published version, found after its release line is gone,
starts a patch cycle. The patch has a release line of its own, created from
the release tag rather than from `dev`:

```sh
git switch -c release/<lead crate>_vX.Y.(Z+1) <lead crate>_vX.Y.Z
```

The version bump and changelog cut of step 2 happen in the fix pull request
into that line, since there is no `dev` candidate to inherit them. Every
other step runs as written: the candidate is tagged on the line, the line
merges into `dev` before the Binding Layer publishes, the release is tagged
on the line's last candidate, and the line merges back once more if the
release commit added anything.

The changelog on `dev` gains the `[X.Y.(Z+1)]` section through that merge,
below `[Unreleased]` and above `[X.Y.Z]`.

## What dev now has to hold

`stable` used to absorb the cost of a `dev` that could sit half way through
a refactor for weeks. With the release cut from `dev`, a candidate can be
taken on any day, and every pull request is judged on whether `dev` could be
released the moment it merges.

- A pull request merges into `dev` only when the work it describes is
  complete. A portion of a larger feature or refactor that would leave
  `dev` mid-way, with a dead code path, a half-migrated type, a disabled
  test or a feature behind an unfinished flag, targets an integration
  branch instead.
- An integration branch is a long-lived branch named for the work,
  `integration/<name>`, that takes the small pull requests of one arc. It
  merges into `dev` when the arc is complete, or earlier when its author and
  reviewer judge that `dev` with the branch merged is no less releasable
  than `dev` without it. The pull request that merges it states which.
- Pull requests into an integration branch are reviewed as pull requests
  into `dev` are, and CI runs on them, since the branch's merge into `dev`
  is one large diff that nobody can review line by line at that point.
- The zingolib/0015 rule stands and gets harder: a wallet file format
  revision that lands on `dev` is shipped, and now on a candidate's clock.
- The changelog is written in the pull request that makes the change, in
  the `[Unreleased]` section of every crate the change touches, under the
  Keep a Changelog heading the change belongs to. A release cut finds no
  changelog work left to do.
- A dependency moves on `dev` only to a version on crates.io or to a git
  pin the team accepts shipping. `[patch.crates-io]` entries pointing at a
  branch block a candidate until they are resolved.

## Open questions

1. Whether the publish moves into CI. crates.io Trusted Publishing lets a
   workflow run at the release tag publish with a short-lived token and no
   stored secret, and zaino's pipeline records the same move as an open
   item. Until then the publish is a maintainer's local step and the signed
   tag is the record of who cut the release.
2. Whether `zingo-cli` keeps a changelog date and tag of its own in a cycle
   led by `zingolib`. It was released as 0.5.0 in the 8.0.0 commit and is
   on crates.io, so it is in the set like any other crate.
3. The retirement of `stable` itself is a decision for a zingo-adrs record
   in the `zingolib/` scope, superseding the `stable` parts of org record
   003 for this repository. This document is the procedure that record
   would point at.
4. The nightly Sync A/B job compares `dev` with the day before. A release
    line gets no benchmark. If a candidate's fix touches sync, the fix pull
    request runs `makers sync-ab` by hand and carries the two numbers, as
    AGENTS.md asks of a bisected regression.
