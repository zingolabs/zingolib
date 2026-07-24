# Wallet file format salvage (dismad's unreadable test wallets)

## Problem

Testing APKs built from the ironwood line before 2026-07-14 wrote wallet
version 42 with an extra `allow_v6_transactions` byte after
`min_confirmations`; builds from the one-day window between `32261bb5f`
and `4158e20c2` wrote version 43 (byte-identical to final 42). The
current reader misparses the former one byte into `PriceList::read` and
rejects the latter on the version range check. Report: "breaking change
in the file formats … not migrating the file properly" (dismad, via
zancas, 2026-07-24). Direction ratified by zancas: tolerate both
orphaned layouts and provide a salvage path around any failed
migration.

## Work

1. Accept version 43 as the final-42 layout; burn 43 (next bump is 44).
2. For version 42, disambiguate the two revisions by EOF-anchored dual
   parse of the post-`min_confirmations` tail (canonical first).
3. Add a prefix-only salvage reader that recovers seed + birthday from
   any v32+ file without parsing the tail, exposed for frontends to
   offer "recover and rescan" when full parse fails.
4. Unit tests: tail-parse disambiguation both layouts, v43 whole-file
   acceptance, salvage on a corrupt-tail file.

## File claims

- zingolib/src/wallet/disk.rs (read path)
- zingolib/src/wallet/disk/testing/tests.rs (new tests)
- zingolib/src/wallet.rs (RecoveryInfo construction only, if needed)
- zingo-cli/src/lib.rs (recovery_info salvage fallback on startup failure)
- docs/adr/0015-landing-in-dev-ships-the-wallet-file-format.md (new ADR)
- zingolib/CONTEXT.md (Persistence glossary: Wallet Version, Shipped
  Format, Recovery Salvage)
- .gitignore (data_wallets corpus guard)

Claimed 2026-07-24 by the session working dismad's migration report.
