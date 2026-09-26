//! Census rows 66 through 68: the sync-state era, where the wallet-file
//! version word stands still at 39 while three sub-records move underneath it.
//!
//! Every grammar in this era shares one top-level layout, replicated from
//! `LightWallet::write` in `zingolib/src/wallet/disk.rs` at each Defining
//! Commit. The three rows differ only at four byte positions, all of them
//! inside records the top-level writer delegates to.
//!
//! Row 66 (`b1c04e38c`) writes the `SyncState` inner version byte 2 and, with
//! it, the `ScanPriority` numbering that inserts `ScannedWithoutMapping` at 2.
//! Row 67 (`ff7ba3ec0`) writes the `SyncState` inner version byte 3, the
//! `ScanPriority` numbering that puts `RefetchingNullifiers` at 0, and a
//! `WalletNote` inner version byte 1 whose record gains a trailing
//! refetch-nullifier `Vector<(u32, u32)>`. Row 68 (`f86717800`) keeps row 67's
//! sync and note encodings unchanged and renumbers `ConfirmationStatus`
//! instead, bumping that record's own inner version byte from 0 to 1 so that
//! the status byte 4, `Failed`, becomes writable.
//!
//! Because the marks live in delegated records rather than in the top-level
//! layout, all three fixtures carry the same wallet contents: one unified key
//! store, one unified address, one transparent address, one outpoint-map entry
//! at this era's u16 output-index width, one written transaction record holding
//! one sapling note, and a `SyncState` holding one scan range. Keeping the
//! shape fixed is deliberate. It leaves the era's four moving bytes as the only
//! differences between neighbouring fixtures, which is exactly what a
//! Discriminator must key on.

use super::util::{
    push_bytes, push_compact_size, push_optional_none, push_optional_some, push_u16_le,
    push_u32_le, push_u64_le, push_u64_string, push_u8,
};
use super::Fixture;

/// The wallet-file version word every row in this era writes
/// (`LightWallet::serialized_version`).
const WALLET_VERSION: u64 = 39;

/// The chain name, written by `utils::write_string` in the u64-length
/// discipline. `ChainType::Mainnet` displays as `main`.
const CHAIN_NAME: &str = "main";

/// The wallet's birthday height, chosen below the transaction record's height
/// so that the two agree.
const BIRTHDAY: u32 = 1_900_000;

/// The height carried by the written transaction's `ConfirmationStatus`. It
/// sits inside mainnet's NU5 range, between that upgrade's activation at
/// 1_687_104 and NU6's at 2_726_400, so the consensus branch id embedded in the
/// transaction body below is unambiguous.
const TRANSACTION_HEIGHT: u32 = 2_000_000;

/// The transaction the wallet holds a record of.
const TXID_TRANSACTION: [u8; 32] = [0xA1; 32];

/// The transaction the outpoint map's scan target points at.
const TXID_SCAN_TARGET: [u8; 32] = [0xB2; 32];

/// The four bytes that separate this era's three grammars, plus the trailing
/// vector one of them appends. Every other byte of the three fixtures is
/// shared.
struct Grammar {
    /// `SyncState::serialized_version` at the Defining Commit.
    sync_state_version: u8,
    /// The `ScanPriority` discriminant written for the fixture's one scan
    /// range. The value names a variant whose number the row moved.
    scan_priority: u8,
    /// `WalletNote::serialized_version` at the Defining Commit.
    wallet_note_version: u8,
    /// Whether the note writer appends the refetch-nullifier ranges vector,
    /// which arrived with `WalletNote` version 1.
    writes_refetch_nullifier_ranges: bool,
    /// `ConfirmationStatus::serialized_version` at the Defining Commit.
    confirmation_status_version: u8,
    /// The `ConfirmationStatus` discriminant written for the fixture's one
    /// transaction record.
    confirmation_status: u8,
}

/// Census row 66's grammar.
const GRAMMAR_66: Grammar = Grammar {
    sync_state_version: 2,
    scan_priority: 2,
    wallet_note_version: 0,
    writes_refetch_nullifier_ranges: false,
    confirmation_status_version: 0,
    confirmation_status: 3,
};

/// Census row 67's grammar.
const GRAMMAR_67: Grammar = Grammar {
    sync_state_version: 3,
    scan_priority: 0,
    wallet_note_version: 1,
    writes_refetch_nullifier_ranges: true,
    confirmation_status_version: 0,
    confirmation_status: 3,
};

/// Census row 68's grammar.
const GRAMMAR_68: Grammar = Grammar {
    sync_state_version: 3,
    scan_priority: 0,
    wallet_note_version: 1,
    writes_refetch_nullifier_ranges: true,
    confirmation_status_version: 1,
    confirmation_status: 4,
};

/// A built fixture together with the offsets of the bytes the tests inspect.
/// Recording the offsets while writing keeps the tests from restating the
/// layout arithmetic, which would only drift from the writer. The offsets
/// serve the tests alone, so a fixture build outside `cfg(test)` leaves them
/// unread.
#[cfg_attr(not(test), allow(dead_code))]
struct Built {
    bytes: Vec<u8>,
    /// Offset of the `ConfirmationStatus` record's inner version byte. The
    /// status discriminant follows immediately.
    confirmation_status_offset: usize,
    /// Offset of the `SyncState` record's inner version byte.
    sync_state_version_offset: usize,
}

/// Census rows 66 through 68, in order.
pub fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            row: 66,
            defining_commit: "b1c04e38c",
            branch: "dev",
            bytes: row_66(),
        },
        Fixture {
            row: 67,
            defining_commit: "ff7ba3ec0",
            branch: "dev",
            bytes: row_67(),
        },
        Fixture {
            row: 68,
            defining_commit: "f86717800",
            branch: "dev",
            bytes: row_68(),
        },
    ]
}

/// Row 66, Defining Commit `b1c04e38c` (the merge of PR #1837,
/// `low_memory_nonlinear_scanning`, whose grammar arrived through `ecbb35574`
/// and `1cd5c9221`).
///
/// The top-level layout replicates `LightWallet::write` in
/// `zingolib/src/wallet/disk.rs` at that commit. The delegated records come
/// from `pepper-sync/src/wallet/serialization.rs` (`SyncState`, `ScanTarget`,
/// `NullifierMap`, `WalletTransaction`, `SaplingNote`, `ShardTrees`),
/// `pepper-sync/src/config.rs` (`SyncConfig`, `PerformanceLevel`),
/// `zingo-status/src/confirmation_status.rs` (`ConfirmationStatus`),
/// `zingolib/src/wallet/keys/unified.rs` (`UnifiedKeyStore`,
/// `UnifiedSpendingKey`, `ReceiverSelection`), and `zingo-price/src/lib.rs`
/// (`PriceList`).
///
/// The row's mark is the `SyncState` inner version byte 2. Under that version
/// the reader maps the scan-priority byte 2 to `ScannedWithoutMapping`, a
/// variant this commit inserted, so the fixture's one scan range carries that
/// byte and the renumbering is visible rather than merely declared. Row 67
/// writes the same version field as 3, so the two rows are always structurally
/// distinct.
///
/// The wallet holds a mnemonic seed, one spending key store, one unified
/// address, one transparent address, one outpoint-map entry, and one
/// transaction record containing one sapling note. The note's `WalletNote`
/// inner version is 0 here and its record ends after the optional spending
/// transaction, with no refetch-nullifier vector.
fn row_66() -> Vec<u8> {
    build(&GRAMMAR_66).bytes
}

/// Row 67, Defining Commit `ff7ba3ec0` (the merge of PR #2156,
/// `backport_stable`, whose grammar arrived through `7dc66315c`).
///
/// The sources replicated are the same files at this commit, and the top-level
/// `LightWallet::write` body is byte-identical to row 66's. Two delegated
/// records moved. `SyncState::serialized_version` became 3, and under that
/// version the scan-priority numbering shifts by one to make room for
/// `RefetchingNullifiers` at 0; the fixture's one scan range carries the byte 0
/// so the new variant appears in the file. `WalletNote::serialized_version`
/// became 1, and both note writers now append a refetch-nullifier
/// `Vector<Range<BlockHeight>>` after the optional spending transaction. The
/// writer emits that vector unconditionally, so the fixture gives it one entry,
/// which distinguishes a populated vector from an absent one as well as from an
/// empty one.
///
/// This fixture is one half of a deliberate pair. Its written transaction
/// record carries a `ConfirmationStatus` whose status byte is 3, a value legal
/// under both this row's grammar and row 68's, and whose record version byte is
/// 0. Row 68 writes the same record with version byte 1 and status byte 4. The
/// pairing makes the census's design question concrete: under this row's
/// numbering the byte 3 means `Confirmed`, while under row 68's it means
/// `Calculated`, so a reader that ignores the record's inner version byte reads
/// this file with the wrong meaning. See `row_68` for the other half.
fn row_67() -> Vec<u8> {
    build(&GRAMMAR_67).bytes
}

/// Row 68, Defining Commit `f86717800` (the merge of PR #2181,
/// `rework_resend_and_remove_tx`; a dev-line commit the dev walk's file set
/// missed, inserted into the census from the stable-arm sweep).
///
/// The top-level `LightWallet::write` body and every sync record are unchanged
/// from row 67. The single moving record is `ConfirmationStatus` in
/// `zingo-status/src/confirmation_status.rs`. Its writer gained a `Failed`
/// variant, renumbered the discriminants so that `Confirmed` is 0, `Mempool` is
/// 1, `Transmitted` is 2, `Calculated` is 3 and `Failed` is 4, and bumped the
/// record's own `serialized_version` from 0 to 1.
///
/// This fixture is the other half of the pair described on `row_67`. Its
/// transaction record carries the status byte 4, `Failed`, which only this
/// row's grammar can write, so the file is unambiguously row 68. Row 67's
/// fixture, by contrast, carries the status byte 3, a value both grammars
/// accept, and therefore parses byte for byte under this row's grammar as well.
/// The record's inner version byte is what resolves the two: this row's reader
/// dispatches on it and so still reads row 67's file correctly, whereas row
/// 67's reader ignores it and would read this row's status bytes with row 67's
/// meanings. The census records the row as fully discriminable by that version
/// byte, and these two fixtures pin the asymmetry that makes it so.
fn row_68() -> Vec<u8> {
    build(&GRAMMAR_68).bytes
}

/// Write one complete wallet file in this era's shared layout, varying only the
/// bytes named by `grammar`. The order below follows `LightWallet::write` at
/// each Defining Commit statement for statement.
fn build(grammar: &Grammar) -> Built {
    let mut out = Vec::new();

    push_u64_le(&mut out, WALLET_VERSION);
    push_u64_string(&mut out, CHAIN_NAME);

    // The mnemonic entropy, written as a `Vector` of bytes. Thirty-two bytes of
    // entropy is a twenty-four-word mnemonic; the value is zeroed because the
    // corpus never carries live key material.
    push_compact_size(&mut out, 32);
    push_bytes(&mut out, &[0u8; 32]);

    push_u32_le(&mut out, BIRTHDAY);

    // The key store map: `Vector<(u32 account id, UnifiedKeyStore)>`.
    push_compact_size(&mut out, 1);
    push_u32_le(&mut out, 0);
    push_unified_key_store(&mut out);

    // The unified addresses:
    // `Vector<(u32 account, u32 index, ReceiverSelection)>`.
    push_compact_size(&mut out, 1);
    push_u32_le(&mut out, 0);
    push_u32_le(&mut out, 0);
    push_receiver_selection(&mut out);

    // The transparent addresses: `Vector<(u32 account, u8 scope, u32 index)>`.
    push_compact_size(&mut out, 1);
    push_u32_le(&mut out, 0);
    push_u8(&mut out, 0);
    push_u32_le(&mut out, 0);

    // The wallet blocks. The era's marks need none, so the vector is empty.
    push_compact_size(&mut out, 0);

    // The wallet transactions: one record, which carries the
    // `ConfirmationStatus` byte this era's last row moved.
    push_compact_size(&mut out, 1);
    let confirmation_status_offset = push_wallet_transaction(&mut out, grammar);

    push_nullifier_map(&mut out);

    // The outpoint map: `Vector<(TxId, u16 output index, ScanTarget)>`. The u16
    // index width is this era's; census row 71 widens it to u32.
    push_compact_size(&mut out, 1);
    push_bytes(&mut out, &TXID_SCAN_TARGET);
    push_u16_le(&mut out, 1);
    push_scan_target(&mut out);

    push_shard_trees(&mut out);

    let sync_state_version_offset = push_sync_state(&mut out, grammar);

    push_sync_config(&mut out);

    // `min_confirmations`, appended by census row 65 (`ad6ded426`).
    push_u32_le(&mut out, 3);

    push_price_list(&mut out);

    Built {
        bytes: out,
        confirmation_status_offset,
        sync_state_version_offset,
    }
}

/// Write a `UnifiedKeyStore` holding a spending key, replicating the
/// `ReadableWriteable` impls in `zingolib/src/wallet/keys/unified.rs`: the
/// record's own version byte, then the key-type tag, then the key itself.
fn push_unified_key_store(out: &mut Vec<u8>) {
    push_u8(out, 0); // UnifiedKeyStore::VERSION
    push_u8(out, 2); // KEY_TYPE_SPEND

    // `UnifiedSpendingKey` writes a `CompactSize` length and then the opaque
    // `to_bytes(Era::Orchard)` encoding. The corpus fills such blobs with dummy
    // bytes, so the length here is representative rather than derived.
    push_compact_size(out, 128);
    push_bytes(out, &[0u8; 128]);
}

/// Write a `ReceiverSelection` with both shielded receivers set. The record's
/// inner version has read 2 since census row 62 retired the transparent bit
/// from the bitmask.
fn push_receiver_selection(out: &mut Vec<u8>) {
    push_u8(out, 2); // ReceiverSelection::VERSION
    push_u8(out, 0b11); // orchard | sapling
}

/// Write one `WalletTransaction` record and return the offset of the
/// `ConfirmationStatus` record's inner version byte, whose status discriminant
/// follows it.
fn push_wallet_transaction(out: &mut Vec<u8>, grammar: &Grammar) -> usize {
    push_u8(out, 0); // WalletTransaction::serialized_version
    push_bytes(out, &TXID_TRANSACTION);

    let confirmation_status_offset = out.len();
    push_u8(out, grammar.confirmation_status_version);
    push_u8(out, grammar.confirmation_status);
    push_u32_le(out, TRANSACTION_HEIGHT);

    push_transaction_body(out);

    push_u32_le(out, 1_700_000_000); // datetime

    push_compact_size(out, 0); // transparent coins
    push_compact_size(out, 1); // sapling notes
    push_sapling_note(out, grammar);
    push_compact_size(out, 0); // orchard notes
    push_compact_size(out, 0); // outgoing sapling notes
    push_compact_size(out, 0); // outgoing orchard notes

    confirmation_status_offset
}

/// Write the raw consensus encoding of the recorded transaction, which
/// `WalletTransaction::write` emits through `Transaction::write`.
///
/// The body is a v5 transaction with no transparent inputs or outputs, no
/// sapling spends or outputs, and no orchard actions. Under ZIP 225 an empty
/// bundle collapses to its count alone, so the whole encoding is twenty-five
/// bytes, which keeps the fixture's opaque region as small as the format
/// permits.
fn push_transaction_body(out: &mut Vec<u8>) {
    // ASSUMPTION: the ZIP 225 v5 transaction encoding, as `zcash_primitives`
    // writes it at these commits. The header sets the overwintered bit above
    // version 5; the version group id is v5's fixed 0x26A7270A; the consensus
    // branch id is NU5's 0xC2D6D0B4, which matches TRANSACTION_HEIGHT on
    // mainnet. `zcash_primitives` is not vendored locally at the pinned
    // revisions, so these three constants come from the published format rather
    // than from source read at the Defining Commits.
    push_u32_le(out, 0x8000_0005); // header: overwintered | version 5
    push_u32_le(out, 0x26A7_270A); // nVersionGroupId
    push_u32_le(out, 0xC2D6_D0B4); // nConsensusBranchId (NU5)
    push_u32_le(out, 0); // lock_time
    push_u32_le(out, 0); // nExpiryHeight
    push_compact_size(out, 0); // tx_in count
    push_compact_size(out, 0); // tx_out count
    push_compact_size(out, 0); // nSpendsSapling
    push_compact_size(out, 0); // nOutputsSapling
    push_compact_size(out, 0); // nActionsOrchard
}

/// Write one `SaplingNote`, the record whose inner version and trailing vector
/// census row 67 moved.
fn push_sapling_note(out: &mut Vec<u8>, grammar: &Grammar) {
    push_u8(out, grammar.wallet_note_version);

    push_bytes(out, &TXID_TRANSACTION);
    push_u16_le(out, 0); // output index

    push_u32_le(out, 0); // account id
    push_u8(out, 0); // scope: External

    // ASSUMPTION: `sapling_crypto::PaymentAddress::to_bytes` is the
    // forty-three-byte diversifier-and-public-key encoding. The corpus zeroes
    // key material, so these bytes are a placeholder and would not decode to a
    // point on the curve; the fixture pins the field's width and position, not
    // its cryptographic validity.
    push_bytes(out, &[0u8; 43]);

    push_u64_le(out, 100_000); // note value in zatoshis

    push_u8(out, 1); // rseed tag: AfterZip212
    push_bytes(out, &[0u8; 32]);

    push_optional_none(out); // nullifier
    push_optional_some(out); // position
    push_u64_le(out, 1_234);

    // ASSUMPTION: `Memo::Empty.encode()` is the five-hundred-and-twelve-byte
    // array whose first byte is 0xF6 and whose remainder is zero, per ZIP 302.
    let mut memo = [0u8; 512];
    memo[0] = 0xF6;
    push_bytes(out, &memo);

    push_optional_none(out); // spending transaction

    if grammar.writes_refetch_nullifier_ranges {
        // `Vector<Range<BlockHeight>>`, appended by `WalletNote` version 1.
        push_compact_size(out, 1);
        push_u32_le(out, 1_990_000);
        push_u32_le(out, 2_000_000);
    }
}

/// Write an empty `NullifierMap`. Its inner version has read 1 since the
/// outpoint values became `ScanTarget` records at census row 64.
fn push_nullifier_map(out: &mut Vec<u8>) {
    push_u8(out, 1); // NullifierMap::serialized_version
    push_compact_size(out, 0); // sapling
    push_compact_size(out, 0); // orchard
}

/// Write one `ScanTarget`, the outpoint map's value record since census row 64.
fn push_scan_target(out: &mut Vec<u8>) {
    push_u8(out, 0); // ScanTarget::serialized_version
    push_u32_le(out, 1_995_000); // block height
    push_bytes(out, &TXID_SCAN_TARGET);
    push_u8(out, 1); // narrow_scan_area
}

/// Write an empty `ShardTrees` pair. Each tree writes an empty shard vector, an
/// empty checkpoint vector, and its cap.
fn push_shard_trees(out: &mut Vec<u8>) {
    push_u8(out, 0); // ShardTrees::serialized_version
    push_empty_shardtree(out); // sapling
    push_empty_shardtree(out); // orchard
}

/// Write one empty memory-backed shard tree.
fn push_empty_shardtree(out: &mut Vec<u8>) {
    push_compact_size(out, 0); // located prunable trees
    push_compact_size(out, 0); // checkpoints

    // ASSUMPTION:
    // `zcash_client_backend::serialization::shardtree::write_shard` writes its
    // version tag 1 and then the tree, and an empty tree is the single Nil tag
    // 0. Verified against the locally available zcash_client_backend 0.23.0;
    // the commits pin 0.18.0 and 0.21.0, where the same two constants hold.
    push_bytes(out, &[1, 0]);
}

/// Write the `SyncState` record and return the offset of its inner version
/// byte, the mark that separates census row 66 from row 67.
fn push_sync_state(out: &mut Vec<u8>, grammar: &Grammar) -> usize {
    let version_offset = out.len();
    push_u8(out, grammar.sync_state_version);

    // The scan ranges: `Vector<(u32 start, u32 end, u8 priority)>`.
    push_compact_size(out, 1);
    push_u32_le(out, BIRTHDAY);
    push_u32_le(out, TRANSACTION_HEIGHT);
    push_u8(out, grammar.scan_priority);

    push_compact_size(out, 0); // sapling shard ranges
    push_compact_size(out, 0); // orchard shard ranges
    push_compact_size(out, 0); // scan targets

    version_offset
}

/// Write the `SyncConfig` record, whose inner version has read 1 since census
/// row 65 (`ad6ded426`) appended the performance level.
fn push_sync_config(out: &mut Vec<u8>) {
    push_u8(out, 1); // SyncConfig::serialized_version
    push_u8(out, 20); // transparent address discovery gap limit
    push_u8(out, 0b011); // scopes: external | internal
    push_u8(out, 0); // PerformanceLevel::serialized_version
    push_u8(out, 2); // PerformanceLevel::High
}

/// Write an empty `PriceList` in the shape census row 60 settled, after that
/// row dropped the leading `Optional<api_key>` census row 59 had appended with
/// the record itself.
fn push_price_list(out: &mut Vec<u8>) {
    push_u8(out, 0); // PriceList::serialized_version
    push_optional_none(out); // time historical prices last updated
    push_optional_none(out); // current price
    push_compact_size(out, 0); // daily prices
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The era holds the wallet-file version word still at 39: the three rows
    /// are told apart by their sub-records, never by the header.
    #[test]
    fn every_row_writes_version_word_thirty_nine() {
        for fixture in fixtures() {
            let word = u64::from_le_bytes(fixture.bytes[..8].try_into().expect("eight bytes"));
            assert_eq!(word, 39, "row {} header", fixture.row);
        }
    }

    /// The module contributes the rows it claims, in census order.
    #[test]
    fn fixtures_are_rows_sixty_six_through_sixty_eight() {
        let rows: Vec<u8> = fixtures().iter().map(|f| f.row).collect();
        assert_eq!(rows, vec![66, 67, 68]);
    }

    /// Row 66 writes the `SyncState` inner version byte 2 and rows 67 and 68
    /// write 3. This byte alone separates row 66 from the rest of the era.
    #[test]
    fn sync_state_version_separates_row_sixty_six() {
        let built_66 = build(&GRAMMAR_66);
        let built_67 = build(&GRAMMAR_67);
        let built_68 = build(&GRAMMAR_68);

        assert_eq!(built_66.bytes[built_66.sync_state_version_offset], 2);
        assert_eq!(built_67.bytes[built_67.sync_state_version_offset], 3);
        assert_eq!(built_68.bytes[built_68.sync_state_version_offset], 3);
    }

    /// Row 68 writes the status byte 4, `Failed`, which only its grammar can
    /// produce, while rows 66 and 67 write a byte both numberings accept. The
    /// record's inner version byte moves with the renumbering, from 0 to 1.
    #[test]
    fn confirmation_status_pairs_row_sixty_seven_against_row_sixty_eight() {
        let built_66 = build(&GRAMMAR_66);
        let built_67 = build(&GRAMMAR_67);
        let built_68 = build(&GRAMMAR_68);

        let status = |b: &Built| b.bytes[b.confirmation_status_offset + 1];
        let version = |b: &Built| b.bytes[b.confirmation_status_offset];

        assert!(
            status(&built_66) <= 3,
            "row 66 status must be legal under both numberings"
        );
        assert!(
            status(&built_67) <= 3,
            "row 67 status must be legal under both numberings"
        );
        assert_eq!(
            status(&built_68),
            4,
            "row 68 status must be the Failed byte"
        );

        assert_eq!(version(&built_66), 0);
        assert_eq!(version(&built_67), 0);
        assert_eq!(version(&built_68), 1);
    }

    /// Rows 67 and 68 share every byte except the two the `ConfirmationStatus`
    /// renumbering moved, and the record's own version byte is one of the two.
    /// That is what makes the pair discriminable, as the census now records.
    #[test]
    fn rows_sixty_seven_and_sixty_eight_differ_only_in_the_status_record() {
        let built_67 = build(&GRAMMAR_67);
        let built_68 = build(&GRAMMAR_68);
        assert_eq!(built_67.bytes.len(), built_68.bytes.len());

        let differing: Vec<usize> = built_67
            .bytes
            .iter()
            .zip(built_68.bytes.iter())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            differing,
            vec![
                built_67.confirmation_status_offset,
                built_67.confirmation_status_offset + 1
            ]
        );
    }

    /// Adjacent census rows produce different files, which is the corpus's
    /// reason to exist.
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
}
