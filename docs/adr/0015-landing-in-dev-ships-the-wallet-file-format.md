# ADR 0015: Landing in dev ships the wallet file format

## Status
Accepted. Supersedes the "never shipped" justification recorded in `4158e20c2`.
Amended 2026-07-29: the readability guarantee is floored at `cc78c2358` (see
the Amendment at the end of this document).

## Context
zingolib supports users who build and run `dev`. No release gate stands between
`dev` and those users, so a wallet-file format revision that lands in `dev` is on
someone's disk from that moment.

A wallet file is a version-tagged prefix followed by a body whose layout the
version governs. The body is not self-describing; the reader interprets it only
through the version number. The prefix has carried the same recovery fields
(chain marker, seed, birthday, account count) since version 32, in three
version-decodable encodings rather than one fixed layout: an account-index
field left at 35, and the chain marker's encoding changed at 41.

Losing the ability to read a body layout does not cost a seeded version-32+
wallet its funds, since the seed is recoverable from the prefix (the salvage
floor, below), but it costs local transaction metadata and a full rescan.

## Decision
A wallet-file format revision is shipped the moment it lands in `dev`, and stays
readable and writable from then on.

1. The reader opens a file written by every format revision that has landed in
   `dev`.
2. A version number used by a revision that reached `dev` is spent, and is never
   reassigned. `LightWallet::serialized_version`'s docs hold the register of
   spent numbers and the next free one.
3. A layout change lands in `dev` only together with the read-side compatibility
   that keeps every shipped file loading, and the regression tests that pin it,
   in the same change.

### Collisions that predate this rule
Version 42 names two layouts on disk and version 43 names a third file
population. Only the reader can separate them: a fresh version number is
available only before a layout reaches a disk.

- Version 43 is read as the canonical 42 layout, which it matches byte for byte.
  The number stays spent.
- Version 42 is disambiguated by an end-of-file-anchored dual parse of the
  tail (the trailing region holding the price list and the optional migration
  section, the only region where the two layouts differ): the pre-release
  layout opens it with one extra settings byte. The pre-release reading is
  consulted only when the canonical reading fails to parse, so a file written
  by current code never loads through the fallback.
- A tail that parses cleanly both ways is refused, not guessed. The wrong reading
  substitutes a different price list and migration state, and no later check
  catches it. Parity confines the refusal: a migration-free tail's length is
  always odd and the two readings differ by exactly one byte, so they cannot
  both be migration-free. The refusal can fire only when at least one reading
  carries a migration section, whose length parity is unconstrained. Within that
  case it is a deliberate exception to rule 1: a well-formed file whose tail
  happens to also parse the other way fails to load and falls to the salvage
  floor, because a detectable refusal is recoverable and a silent wrong guess is
  not.

### Salvage floor
`LightWallet::read_recovery_info` recovers the seed phrase, birthday, and
account count from the prefix of any seeded version-32+ file without parsing
the body, and `zingo-cli` falls back to it when the wallet will not load and
the user asks for `recovery_info`. Within that reach (seeded, version 32 or
later, prefix intact), no file strands funds: not a file from an abandoned
branch, nor a file from a future version that keeps the recovery fields in the
prefix.

The floor has edges. A pre-32 file keeps its seed too deep in the body to reach
without a full parse; a view-only file stores no seed; corruption inside the
prefix defeats it, and a damaged seed field of valid length decodes to a
plausible wrong mnemonic, since any entropy of valid length yields a
well-formed one. The floor also holds for future files only while revisions
keep the recovery fields in the prefix: moving them would sink the floor for
every binary shipped before the move.

The floor is subordinate to the compatibility guarantee, not an alternative to
it: restoring from seed forfeits local transaction metadata and forces a rescan.

## Considered options
**Compatibility bounded by releases.** Requires a release gate. None exists:
`dev` is built and run by supported users, so "not yet released" names no
protective boundary.

**Declare orphaned layouts unreadable and direct affected users to restore from
seed.** Promotes the salvage floor to the guarantee, at the cost of the wallet
history that compatibility preserves.

**Assign a fresh version number to the second layout instead of disambiguating
42.** Available only before the second layout reaches a disk. Once two layouts
claim one number, the reader is the only place they can be separated.

## Consequences
A layout change carries its compatibility with it: the same change lands the
version gate (or, where a number is already double-assigned, the
disambiguation) and the regression tests that pin every shipped revision to a
loading wallet.

The wallet-load test corpus enforces this: the example wallets, the
orphaned-layout tests, and the local `data_wallets` sweep.

A test that exercises a layout asserts on the region that layout governs.
Recovery info is read from the prefix and survives a misparsed tail intact, so a
wallet-file test that checks only recovery info proves nothing about the tail.

The version-42 dual parse is permanent. It costs one buffered read of the file
tail and a second parse of it on every load of a version-42 file. Until the
next format bump, that is every file current code writes.

## Amendment (2026-07-29): the readability floor

The Format Census (issue #2590) falsified rule 1 as a statement of fact.
Every format older than `cc78c2358` (2022-10-26, the WalletCapability
inception) rejects or garbage-parses at `read_v0`'s opening
`WalletCapability::read` — forty-one grammars, back through the inherited
zecwallet lineage, that no current reader opens. Three younger windows
misparse for one shared root cause, a sub-record grammar that changed while
its version field stood still.

The guarantee is scoped, not abandoned. Its floor is `cc78c2358`, the oldest
grammar the modern reader ever opened.

At or above the floor, rules 1–3 bind as written, and the three misparse
windows are breaches owed fixes in the same arc that lands Format
Recognition: dev-v40 files (2026-03-25 to 06-15, the unbounded `read_string`
allocation abort), `cc78c2358`-era files (2022-10-26 to 11-08, the trailing
encrypted-byte desync), and version-34 price files (2025-05-15 to 05-23, the
api-key Optional shift). Each is restored by dispatching the read on the
Recognition Verdict — identification by Defining Commit — rather than on the
Wallet Version alone, which these windows prove insufficient.

Below the floor, the promise is identification plus salvage: a pre-floor file
receives a precise naming of its grammar by Defining Commit and, where its
prefix permits, the salvage floor above. No pre-floor reader will be
resurrected; readability restoration for any such format, if ever wanted,
is its own decision.

Recognition itself promises identification only, never readability (see
Format Recognition in `zingolib/CONTEXT.md`): a recognized verdict does not
imply the running build can load the file.

Rule 3 extends to recognition (ratified 2026-07-29). A layout change lands
in `dev` only together with its Format Recognition arm and the neighbor
tests pinning its Discriminator, in the same change. Two standing tests
enforce the extension. The fixture-corpus test asserts that every census
fixture is recognized as exactly its own arm and no other, pinning the
frozen grammars' transcriptions forever. The live round-trip tripwire
serializes a wallet with the current writer and asserts the bytes are
recognized as the newest arm: a writer change without its matching
recognizer arm turns the build's own output unrecognizable and fails
immediately, so the extension cannot be forgotten, only defeated on
purpose.
