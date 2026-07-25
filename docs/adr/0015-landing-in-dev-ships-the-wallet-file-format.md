# Landing in dev ships the wallet file format

Commit `4158e20c2` re-pinned the wallet file version to 42 after the
`allow_v6_transactions` byte was removed, and ruled that files written
by the byte-carrying revision "are not readable" because "the removed
byte never shipped." That justification was wrong about what shipping
means for this project. We support users who build and run code from
dev; there is no release gate standing between dev and those users, so
nothing that lands in dev is unreleased. The ruling met reality on
2026-07-24, when a tester's device held wallets written by assorted
pre-release builds and the reader could not open them.

The decision: **a wallet-file format revision is shipped the moment it
lands in dev.** From that moment the wallet must remain readable and
writable going forward. Concretely: the reader must open a file written
by every format revision that has ever landed in dev; a version number,
once used by any revision that reached users, is burned and never
reassigned; and a format change may land in dev only together with the
read-side compatibility that keeps every previously shipped file
loading, proven by regression tests in the same change. Version 42's
two layouts are both read, disambiguated by an end-of-file-anchored
dual parse of the file tail: the canonical layout is preferred, so a
file written by current code never depends on the fallback, and a tail
that parses cleanly *both* ways is refused rather than guessed. The
refusal covers a case that cannot arise for a migration-free tail,
whose length is always odd while the two readings differ by exactly
one byte, but for which no such parity argument holds once a migration
section is present. Version 43 is burned; the next format bump is 44.

Beneath the compatibility guarantee sits a salvage floor for files
outside it — corrupt files, files from abandoned side branches, files
from the future: `LightWallet::read_recovery_info` recovers the seed
phrase, birthday, and account count from the stable prefix of any
version 32+ file without parsing the tail, so no wallet file can
strand its funds. The salvage floor is a backstop, not a substitute
for compatibility: restoring from seed forfeits local transaction
metadata and forces a rescan.

## Considered Options

Keeping the release-gate rule was rejected because the gate does not
exist: dev is built and run by supported users, so "not yet released"
describes no protective boundary. Declaring the orphaned layouts
unreadable and directing affected users to restore from seed was
rejected as a primary path for the same reason we keep the salvage
floor subordinate — it destroys wallet history that compatibility
preserves. Bumping to a fresh version number instead of disambiguating
42's two layouts was unavailable after the fact: both layouts already
claim the number 42 on users' disks, so only the reader can tell them
apart.

## Consequences

Format changes carry their compatibility with them: the change that
alters the layout also lands the version gate (or, where a number was
double-assigned, the disambiguation) and the regression tests that pin
every shipped revision to a loading wallet. The wallet-load test
corpus — the example wallets, the orphaned-layout tests, and the
local `data_wallets` sweep — is the enforcement mechanism. Retiring a
version number is documented in `LightWallet::serialized_version`'s
docs, which name the next free number. The dual parse for version 42
is permanent; it costs one buffered read of the small file tail and a
second parse of it on every load. Tests that exercise a format layout
must assert on the region the layout governs: a wallet-file test that
checks only recovery info proves nothing about the tail, because
recovery info is read from the file prefix and survives a misparsed
tail intact.
