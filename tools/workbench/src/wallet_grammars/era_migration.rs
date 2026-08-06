//! Census rows 72 through 77: the version-42 era and the one-day version 43.
//!
//! Six Defining Commits share one wallet-file skeleton. Five of them write the
//! version word 42 and one writes 43, so the version word alone identifies
//! almost nothing here; the grammar deltas live in a single settings byte and
//! in the migration section's private `INNER_VERSION`, which climbed 1 → 2 →
//! 3 → 4 in twelve days while `LightWallet::serialized_version()` never moved.
//!
//! The skeleton is `LightWallet::write` in `zingolib/src/wallet/disk.rs`. Its
//! sub-writers in this era are `pepper_sync::wallet::serialization`
//! (`NullifierMap`, `ScanTarget`, `ShardTrees`, `SyncState`),
//! `pepper_sync::config` (`SyncConfig`, `PerformanceLevel`),
//! `zingo_price::PriceList`, the `ReadableWriteable` implementations for
//! `UnifiedKeyStore` and `ReceiverSelection` in
//! `zingolib/src/wallet/keys/unified.rs`, and
//! `zingolib/src/wallet/migration/store.rs`. Every one of those files except
//! `store.rs` is byte-identical across all six commits, which is exactly why
//! this era's rows are so hard to tell apart: the diff from `fffcc9e02` to
//! `f48b15c9e` touches no write path outside `disk.rs` and `store.rs`.
//!
//! Every fixture here writes the migration section as `Some` with one part,
//! because that section is where five of the six rows differ. Every fixture
//! also carries one outpoint-map entry, whose output index is a u32 in this
//! era, continuing the width the preceding era's row 71 established.
//!
//! The migration parameter *values* are a single synthetic set held constant
//! across all six fixtures. The values are not part of the grammar — only the
//! field list is — and holding them fixed makes the byte differences between
//! rows isolate the grammar deltas rather than the drifting provisional ZIP
//! 318 constants. The real `MigrationParams::provisional` moved twice inside
//! this era, changing both its denomination ladder and its bucket modulus,
//! without altering a single field width.

use super::util::{
    push_bytes, push_compact_size, push_optional_none, push_optional_some, push_u32_le,
    push_u64_le, push_u8,
};
use super::Fixture;

/// The wallet's birthday, a plausible mainnet height for this era.
const BIRTHDAY: u32 = 3_000_000;

/// The transaction whose output the outpoint map points at.
const OUTPOINT_TXID: [u8; 32] = [0x11; 32];

/// The transaction the outpoint map's `ScanTarget` names.
const SCAN_TARGET_TXID: [u8; 32] = [0x22; 32];

/// The transaction holding the migration part's bound note.
const BOUND_NOTE_TXID: [u8; 32] = [0x33; 32];

/// The bound note's nullifier.
const BOUND_NOTE_NULLIFIER: [u8; 32] = [0x44; 32];

/// The bound note's commitment.
const BOUND_NOTE_COMMITMENT: [u8; 32] = [0x55; 32];

/// The height at which the outpoint map's scan target sits.
const SCAN_TARGET_HEIGHT: u32 = 3_012_345;

/// The migration part's bucket, a boundary height divided by the modulus.
const BUCKET_INDEX: u64 = 20_920;

/// The migration part's anchor bucket, one bucket earlier than its own.
const ANCHOR_BUCKET: u64 = 20_919;

/// The boundary height the migration part targets, `BUCKET_INDEX` times 144.
const TARGET_HEIGHT: u32 = 3_012_480;

/// The migration part's expiry, the target height plus the era's provisional
/// delta of one bucket length and the standard forty-block margin.
const EXPIRY_HEIGHT: u32 = 3_012_664;

/// The moment the user consented to the migration plan, as a Unix timestamp.
const CONSENTED_AT: u64 = 1_752_000_000;

/// This era's fixtures, in census order.
pub fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            row: 72,
            defining_commit: "fffcc9e02",
            branch: "dev",
            bytes: row_72(),
        },
        Fixture {
            row: 73,
            defining_commit: "32261bb5f",
            branch: "dev",
            bytes: row_73(),
        },
        Fixture {
            row: 74,
            defining_commit: "4158e20c2",
            branch: "dev",
            bytes: row_74(),
        },
        Fixture {
            row: 75,
            defining_commit: "a6c1354ad",
            branch: "dev",
            bytes: row_75(),
        },
        Fixture {
            row: 76,
            defining_commit: "894fe8e0a",
            branch: "dev",
            bytes: row_76(),
        },
        Fixture {
            row: 77,
            defining_commit: "f48b15c9e",
            branch: "dev",
            bytes: row_77(),
        },
    ]
}

/// Row 53, version 42 layout A, defined by merge `fffcc9e02` (PR #2428,
/// authored by `643e5eea8` and `bf3ebdd19`). This replicates
/// `LightWallet::write` in `zingolib/src/wallet/disk.rs` at that commit
/// together with `crate::wallet::migration::store::write` and its `write_part`
/// helper, whose `INNER_VERSION` is 1.
///
/// The grammar-unique mark is the `allow_v6_transactions` u8 written between
/// `min_confirmations` and the price list. No other grammar in the census
/// carries that byte: the setting was removed one day later and never
/// restored.
///
/// The wallet is a mainnet wallet holding a zeroed mnemonic seed, one account
/// whose key store is the `Empty` variant, no addresses, no blocks and no
/// transactions, one scanned range, one outpoint-map entry, empty shard trees,
/// and a migration in the `PartsScheduled` phase holding one assigned part.
/// Every other fixture in this era carries the same contents, so the six files
/// differ only where their grammars do.
fn row_72() -> Vec<u8> {
    let mut out = Vec::new();
    push_prefix(&mut out, 42, true);
    push_optional_some(&mut out);
    push_migration_section(&mut out, 1);
    out
}

/// Row 54, version 43, defined by `32261bb5f` ("feat!: remove the dead
/// allow_v6_transactions setting"). This replicates the same
/// `LightWallet::write` and the same `migration/store.rs` writer at
/// `INNER_VERSION` 1. The commit deleted one `write_u8` call from `disk.rs`
/// and raised `serialized_version()` from 42 to 43.
///
/// The grammar-unique mark is the version word 43, which dev wrote for roughly
/// one day on 2026-07-14 before `4158e20c2` renumbered it back. The wallet's
/// contents match row 72's exactly, so the two fixtures differ only where
/// their grammars do.
fn row_73() -> Vec<u8> {
    let mut out = Vec::new();
    push_prefix(&mut out, 43, false);
    push_optional_some(&mut out);
    push_migration_section(&mut out, 1);
    out
}

/// Row 55, version 42 reused, defined by `4158e20c2` ("fix: keep the wallet
/// file at version 42; the removed byte never shipped"). The commit changed
/// `serialized_version()` from 43 back to 42 and dropped the reader's
/// consume-and-discard branch for the removed byte. The written grammar is
/// otherwise identical to row 73's, so these two fixtures differ at the
/// version word alone.
///
/// Against row 72 the difference is the absent `allow_v6_transactions` byte,
/// one byte in the middle of the settings section, with the same version word
/// on both files. Telling those two apart requires parsing the tail, which is
/// what the dual-parse reader introduced at `a6c1354ad` does by anchoring the
/// price list and the migration section at end of file.
fn row_74() -> Vec<u8> {
    let mut out = Vec::new();
    push_prefix(&mut out, 42, false);
    push_optional_some(&mut out);
    push_migration_section(&mut out, 1);
    out
}

/// Row 56, version 42, defined by merge `a6c1354ad` (authored by `a2f4e2f08`).
/// This replicates `migration/store.rs::write` at `INNER_VERSION` 2, which
/// appends a `MigrationMode` u8 after the parts vector: 0 for `Scheduled`, 1
/// for `Immediate`. Nothing in `disk.rs` changed, so the file differs from row
/// 55 by the inner version byte and one trailing byte inside the migration
/// section. The fixture writes the `Scheduled` mode, the reading that same
/// commit's reader assigns to any version-1 blob.
fn row_75() -> Vec<u8> {
    let mut out = Vec::new();
    push_prefix(&mut out, 42, false);
    push_optional_some(&mut out);
    push_migration_section(&mut out, 2);
    out
}

/// Row 57, version 42, defined by `894fe8e0a` ("chore: align"). This
/// replicates `migration/store.rs::write` at `INNER_VERSION` 3, which removed
/// the params' `expiry_delta` u32: the canonical expiry became the fixed ZIP
/// 318 formula rather than a stored parameter. The neighbouring rename of
/// `dust_floor` to `max_residual_value` keeps the same u64 at the same offset
/// and so writes no different byte. The params record is therefore exactly
/// four bytes shorter than row 75's.
fn row_76() -> Vec<u8> {
    let mut out = Vec::new();
    push_prefix(&mut out, 42, false);
    push_optional_some(&mut out);
    push_migration_section(&mut out, 3);
    out
}

/// Row 58, version 42, defined by `f48b15c9e` ("chore: mobile initial batch
/// fix"). This replicates `migration/store.rs::write_part` at `INNER_VERSION`
/// 4, which appends an `Optional<u64>` `anchor_bucket` after each part's
/// `bucket_index`: the anchor became a bucket of its own rather than the
/// broadcast window's boundary (ADR 0018). This is the grammar dev writes
/// today. The fixture's part carries an anchor one bucket earlier than the
/// part's own, the arrangement the change exists to allow.
fn row_77() -> Vec<u8> {
    let mut out = Vec::new();
    push_prefix(&mut out, 42, false);
    push_optional_some(&mut out);
    push_migration_section(&mut out, 4);
    out
}

/// Everything `LightWallet::write` emits before the optional migration
/// section: the header, the key material, the sync structures, the settings
/// and the price list. `allow_v6_transactions` says whether to emit the u8
/// that only row 72's grammar carries.
fn push_prefix(out: &mut Vec<u8>, version_word: u64, allow_v6_transactions: bool) {
    push_header_through_min_confirmations(out, version_word);
    if allow_v6_transactions {
        // `writer.write_u8(u8::from(self.wallet_settings.allow_v6_transactions))`.
        // The setting is false, the value a fresh wallet held.
        push_u8(out, 0);
    }
    push_price_list(out);
}

/// The wallet file from its version word through `min_confirmations`, the last
/// field before the byte that distinguishes row 72.
fn push_header_through_min_confirmations(out: &mut Vec<u8>, version_word: u64) {
    push_u64_le(out, version_word);

    // The chain type as a u8, the encoding minted at row 69: 0 mainnet,
    // 1 testnet, 2 regtest.
    push_u8(out, 0);

    // The mnemonic entropy as a `Vector<u8>`. Thirty-two zero bytes stand in
    // for a twenty-four-word seed; key material in this corpus is zeroed.
    push_compact_size(out, 32);
    push_bytes(out, &[0u8; 32]);

    // The birthday as a u32, the width in force since row 56.
    push_u32_le(out, BIRTHDAY);

    // `Vector<(account u32, UnifiedKeyStore)>`. One account, whose key store
    // is the `Empty` variant: a version byte of 0 followed by the key-type
    // discriminant `KEY_TYPE_EMPTY`, also 0. Writing the spend or view variant
    // would embed a `UnifiedSpendingKey` or `UnifiedFullViewingKey`, neither
    // of which this era changed.
    push_compact_size(out, 1);
    push_u32_le(out, 0);
    push_u8(out, 0);
    push_u8(out, 0);

    // `Vector<(account u32, address index u32, ReceiverSelection)>`, the
    // unified addresses, and `Vector<(account u32, scope u8, address index
    // u32)>`, the transparent addresses. Both empty: neither is needed to
    // exhibit this era's marks, and the reader regenerates each stored
    // address from its account's key store, which the `Empty` variant cannot
    // do. Leaving them empty keeps every fixture in this era loadable, not
    // merely well-formed.
    push_compact_size(out, 0);
    push_compact_size(out, 0);

    // `Vector<WalletBlock>` and `Vector<WalletTransaction>`, both empty.
    push_compact_size(out, 0);
    push_compact_size(out, 0);

    push_nullifier_map(out);
    push_outpoint_map(out);
    push_shard_trees(out);
    push_sync_state(out);
    push_sync_config(out);

    // `min_confirmations`, a `NonZeroU32` written as a u32. Three is the value
    // the reader substitutes for files predating the field.
    push_u32_le(out, 3);
}

/// `NullifierMap::write` in `pepper-sync/src/wallet/serialization.rs`. Its
/// version is 2 throughout this era, the value that added the Ironwood map
/// beside the Sapling and Orchard ones. All three maps are empty.
fn push_nullifier_map(out: &mut Vec<u8>) {
    push_u8(out, 2);
    push_compact_size(out, 0);
    push_compact_size(out, 0);
    push_compact_size(out, 0);
}

/// The outpoint map, written inline by `LightWallet::write` as a `Vector` of
/// `(txid, output index u32, ScanTarget)`. The output index is a u32 here, the
/// width row 71 widened it to; `ScanTarget::write` contributes its own version
/// byte, 0 throughout this era.
fn push_outpoint_map(out: &mut Vec<u8>) {
    push_compact_size(out, 1);
    push_bytes(out, &OUTPOINT_TXID);
    push_u32_le(out, 1);

    push_u8(out, 0);
    push_u32_le(out, SCAN_TARGET_HEIGHT);
    push_bytes(out, &SCAN_TARGET_TXID);
    push_u8(out, 1);
}

/// `ShardTrees::write`, whose version is 1 throughout this era. It writes the
/// Sapling, Orchard and Ironwood trees in turn, each as a vector of shards, a
/// vector of checkpoints, and a cap. All three trees are empty.
fn push_shard_trees(out: &mut Vec<u8>) {
    push_u8(out, 1);
    for _ in 0..3 {
        push_compact_size(out, 0);
        push_compact_size(out, 0);
        push_empty_shard(out);
    }
}

/// The cap of an empty shard tree, written by
/// `zcash_client_backend::serialization::shardtree::write_shard`.
fn push_empty_shard(out: &mut Vec<u8>) {
    // ASSUMPTION: `write_shard` emits its `SER_V1` tag of 1 and then walks the
    // tree, emitting `NIL_TAG` of 0 for the empty node, so an empty cap is the
    // two bytes `01 00`. Read from zcash_client_backend 0.23.0's
    // `src/serialization/shardtree.rs`; this era's Cargo.lock pins that
    // version from a librustzcash git revision rather than from crates.io, and
    // the git tree was not consulted.
    push_u8(out, 1);
    push_u8(out, 0);
}

/// `SyncState::write`, whose version is 4 throughout this era. One scanned
/// range covers the wallet's history; the Sapling, Orchard and Ironwood shard
/// ranges and the scan targets are all empty.
fn push_sync_state(out: &mut Vec<u8>) {
    push_u8(out, 4);

    // `Vector<(start u32, end u32, priority u8)>`. Under version 4 the
    // priority discriminants run RefetchingNullifiers 0, Scanning 1, Scanned
    // 2, ScannedWithoutMapping 3, Historic 4, OpenAdjacent 5, FoundNote 6,
    // ChainTip 7, Verify 8. This range is Scanned.
    push_compact_size(out, 1);
    push_u32_le(out, BIRTHDAY);
    push_u32_le(out, SCAN_TARGET_HEIGHT + 1);
    push_u8(out, 2);

    push_compact_size(out, 0);
    push_compact_size(out, 0);
    push_compact_size(out, 0);
    push_compact_size(out, 0);
}

/// `SyncConfig::write` in `pepper-sync/src/config.rs`, whose version is 1
/// throughout this era. It writes the gap limit, a scope bitmask with external
/// at bit 0, internal at bit 1 and refund at bit 2, and then a
/// `PerformanceLevel` record carrying its own version byte of 0.
fn push_sync_config(out: &mut Vec<u8>) {
    push_u8(out, 1);
    push_u8(out, 10);
    push_u8(out, 0b101);
    push_u8(out, 0);
    push_u8(out, 2);
}

/// `PriceList::write` in `zingo-price/src/lib.rs`, whose version is 0
/// throughout this era. The wallet has never fetched a price, so both
/// optionals are absent and the daily-price vector is empty.
fn push_price_list(out: &mut Vec<u8>) {
    push_u8(out, 0);
    push_optional_none(out);
    push_optional_none(out);
    push_compact_size(out, 0);
}

/// The migration section, `zingolib/src/wallet/migration/store.rs::write`, at
/// the given `INNER_VERSION`. The four versions in this era share one field
/// order and differ in three places: version 2 appends the `MigrationMode`
/// byte after the parts vector, version 3 removes the params' `expiry_delta`
/// u32, and version 4 appends each part's `anchor_bucket`.
fn push_migration_section(out: &mut Vec<u8>, inner_version: u8) {
    push_u8(out, inner_version);

    // The params record. Its values are this module's synthetic set, held
    // constant across the era so the fixtures differ only where the grammars
    // do.
    push_u32_le(out, 1);
    push_compact_size(out, 2);
    push_u64_le(out, 100_000_000);
    push_u64_le(out, 10_000_000);
    push_u64_le(out, 100_000_000);
    // `dust_floor` through version 2, renamed `max_residual_value` at version
    // 3. The rename left the u64 at the same offset, so it writes no different
    // byte.
    push_u64_le(out, 10_000_000);
    push_u64_le(out, 10_000);
    push_u32_le(out, 144);
    push_u32_le(out, 8);
    push_u32_le(out, 6);
    // `max_actions_per_split_tx`, a usize widened to u64 on the wire.
    push_u64_le(out, 32);
    if inner_version <= 2 {
        // `expiry_delta`, removed at version 3.
        push_u32_le(out, 184);
    }
    push_u64_le(out, 10_000);

    // The consent binding: two raw thirty-two byte hashes and a timestamp. The
    // hashes are zeroed like the rest of this corpus's fixed-width material.
    push_bytes(out, &[0u8; 32]);
    push_bytes(out, &[0u8; 32]);
    push_u64_le(out, CONSENTED_AT);

    // The signing strategy: 0 for LazyAtBoundary, 1 for PreSigned.
    push_u8(out, 0);

    // The account index.
    push_u32_le(out, 0);

    // The phase: 0 Planned, 1 NoteSplitting (with a round and a txid vector),
    // 2 PartsScheduled, 3 Complete (with a residual). PartsScheduled carries
    // no payload and matches a wallet holding a scheduled part.
    push_u8(out, 2);

    // `Vector<PartRecord>`, written by `write_part`.
    push_compact_size(out, 1);
    push_part(out, inner_version);

    if inner_version >= 2 {
        // The `MigrationMode`: 0 Scheduled, 1 Immediate. Version 1 predates
        // the byte, and its reader defaults such a blob to Scheduled.
        push_u8(out, 0);
    }
}

/// One `PartRecord`, written by `write_part` in `migration/store.rs`. The part
/// is bound to a note, placed in a bucket, given a target height and an expiry,
/// and assigned but not yet signed, so it carries neither a signed blob nor a
/// transaction id nor a boundary witness.
fn push_part(out: &mut Vec<u8>, inner_version: u8) {
    push_u32_le(out, 0);
    push_u64_le(out, 100_000_000);

    // `Optional<BoundNote>`: the note's txid and u32 output index, then its
    // nullifier and commitment as raw thirty-two byte fields.
    push_optional_some(out);
    push_bytes(out, &BOUND_NOTE_TXID);
    push_u32_le(out, 0);
    push_bytes(out, &BOUND_NOTE_NULLIFIER);
    push_bytes(out, &BOUND_NOTE_COMMITMENT);

    // `Optional<u64>` bucket index.
    push_optional_some(out);
    push_u64_le(out, BUCKET_INDEX);

    if inner_version >= 4 {
        // `Optional<u64>` anchor bucket, appended at version 4.
        push_optional_some(out);
        push_u64_le(out, ANCHOR_BUCKET);
    }

    // `Optional<u32>` target height.
    push_optional_some(out);
    push_u32_le(out, TARGET_HEIGHT);

    // The part state: 0 Bound, 1 Assigned, 2 Signed, 3 Broadcast, 4 Confirmed
    // (with a u32 height), 5 Expired, 6 Invalidated.
    push_u8(out, 1);

    // `Optional<TxId>`, absent: nothing has been broadcast.
    push_optional_none(out);

    // `Optional<u32>` expiry height.
    push_optional_some(out);
    push_u32_le(out, EXPIRY_HEIGHT);

    // `Optional<Vector<u8>>` signed blob, absent under the lazy strategy.
    push_optional_none(out);

    // `Optional<BoundaryWitness>`, absent for the same reason.
    push_optional_none(out);

    // The attempt counter.
    push_u8(out, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offset at which row 72 writes its `allow_v6_transactions` byte,
    /// derived by writing the file up to that point.
    fn allow_v6_offset() -> usize {
        let mut out = Vec::new();
        push_header_through_min_confirmations(&mut out, 42);
        out.len()
    }

    /// The offset at which a row writes the migration section's inner version
    /// byte: everything before the section, plus the `Optional` Some marker.
    fn inner_version_offset(allow_v6_transactions: bool) -> usize {
        let mut out = Vec::new();
        push_prefix(&mut out, 42, allow_v6_transactions);
        out.len() + 1
    }

    /// The version words the six writers emit. Five rows share 42 and one
    /// writes the 43 that dev carried for a day, so the word identifies a row
    /// only in that single case.
    #[test]
    fn version_words_run_forty_two_forty_three_then_forty_two() {
        let words: Vec<u64> = fixtures()
            .iter()
            .map(|fixture| {
                let mut word = [0u8; 8];
                word.copy_from_slice(&fixture.bytes[..8]);
                u64::from_le_bytes(word)
            })
            .collect();
        assert_eq!(words, vec![42, 43, 42, 42, 42, 42]);
    }

    /// Rows 54 and 55 are the census's purest version-word-only pair:
    /// `4158e20c2` changed the literal 43 back to 42 and nothing else in the
    /// writer.
    #[test]
    fn rows_73_and_74_differ_only_in_the_version_word() {
        let earlier = row_73();
        let later = row_74();
        assert_eq!(earlier.len(), later.len());
        assert_eq!(earlier[8..], later[8..]);
        assert_ne!(earlier[..8], later[..8]);
    }

    /// Row 53's grammar-unique mark is one `allow_v6_transactions` byte between
    /// `min_confirmations` and the price list. Removing it turns row 72 into
    /// row 74 exactly, which is what leaves the two version-42 layouts
    /// distinguishable only by parsing the tail.
    #[test]
    fn row_72_carries_the_allow_v6_byte_and_row_74_does_not() {
        let layout_a = row_72();
        let reused = row_74();
        let offset = allow_v6_offset();

        assert_eq!(layout_a.len(), reused.len() + 1);
        assert_eq!(layout_a[offset], 0);

        let mut stripped = layout_a.clone();
        stripped.remove(offset);
        assert_eq!(stripped, reused);
    }

    /// The migration section's private `INNER_VERSION` is this era's real
    /// discriminator: it moved 1 → 2 → 3 → 4 while the wallet version word
    /// stayed at 42.
    #[test]
    fn migration_inner_versions_climb_one_two_three_four() {
        assert_eq!(row_72()[inner_version_offset(true)], 1);

        let rows = [
            (row_73(), 1u8),
            (row_74(), 1),
            (row_75(), 2),
            (row_76(), 3),
            (row_77(), 4),
        ];
        for (bytes, inner_version) in rows {
            assert_eq!(bytes[inner_version_offset(false)], inner_version);
        }
    }

    /// Version 3 removed the params' `expiry_delta` u32 and added nothing, so
    /// row 76 is exactly four bytes shorter than row 75.
    #[test]
    fn dropping_expiry_delta_shortens_the_params_record_by_four() {
        assert_eq!(row_75().len(), row_76().len() + 4);
    }

    /// Version 4 appended one present `Optional<u64>` to the single part, so
    /// row 77 is nine bytes longer than row 76.
    #[test]
    fn adding_an_anchor_bucket_lengthens_each_part_by_nine() {
        assert_eq!(row_77().len(), row_76().len() + 9);
    }

    /// Neighbouring rows are distinct files. The census claims every row is a
    /// grammar of its own, and adjacent rows are the hardest pairs.
    #[test]
    fn adjacent_rows_are_byte_distinct() {
        let fixtures = fixtures();
        for pair in fixtures.windows(2) {
            assert_ne!(
                pair[0].bytes, pair[1].bytes,
                "rows {} and {} produced identical bytes",
                pair[0].row, pair[1].row
            );
        }
    }

    /// The era covers census rows 72 through 77, every one of them minted on
    /// dev's first-parent line.
    #[test]
    fn fixtures_carry_their_census_identities() {
        let fixtures = fixtures();
        let rows: Vec<u8> = fixtures.iter().map(|fixture| fixture.row).collect();
        assert_eq!(rows, vec![72, 73, 74, 75, 76, 77]);

        let commits: Vec<&str> = fixtures
            .iter()
            .map(|fixture| fixture.defining_commit)
            .collect();
        assert_eq!(
            commits,
            vec![
                "fffcc9e02",
                "32261bb5f",
                "4158e20c2",
                "a6c1354ad",
                "894fe8e0a",
                "f48b15c9e"
            ]
        );

        assert!(fixtures.iter().all(|fixture| fixture.branch == "dev"));
    }
}
