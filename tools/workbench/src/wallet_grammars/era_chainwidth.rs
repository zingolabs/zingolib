//! Census rows 69 through 71: the chain-encoding and outpoint-width era.
//!
//! These three rows are the Format Census's motivating collision. The `dev`
//! and `stable` branches each minted a wallet file version 40, and the two
//! grammars are mutually unreadable. Row 69 is `dev`'s version 40, which
//! replaced the length-prefixed chain-name string with a single chain-type
//! byte. Row 70 is `stable`'s version 40, which kept the chain-name string
//! and instead widened the outpoint index from `u16` to `u32` at six write
//! sites. Row 71 is the union that `dev` reached when it merged `stable`
//! back in: it carries both changes and bumps the version word to 41.
//!
//! Today's reader dispatches on the version word alone, so it reads a row 69
//! file with row 70's grammar and misparses everything after byte 8. The two
//! fixtures below differ at exactly that byte, which is what makes the defect
//! reproducible.
//!
//! All three fixtures describe the same wallet — one mainnet account, one
//! unified address, one transparent address, one scanned block, one wallet
//! transaction carrying one output of each kind, one nullifier of each pool,
//! and one outpoint-map entry — so that every byte on which they differ is a
//! grammar difference rather than a content difference.

use super::util::{
    push_bytes, push_compact_size, push_compact_vec_u8, push_i64_le, push_optional_none,
    push_optional_some, push_u16_le, push_u32_le, push_u64_le, push_u64_string, push_u8,
};
use super::Fixture;

/// The two axes on which rows 69 through 71 differ, plus the inner record
/// versions those axes drag along.
///
/// Every other byte of the three fixtures is identical, so this struct is the
/// complete statement of what the census distinguishes here.
struct Grammar {
    /// The version word written at offset 0.
    version_word: u64,
    /// Whether the chain is a single type byte (rows 69 and 71) rather than a
    /// u64-length-prefixed name string (row 70).
    chain_as_byte: bool,
    /// Whether output indices are `u32` (rows 70 and 71) rather than `u16`.
    wide_output_index: bool,
    /// `TransparentCoin::serialized_version` at the defining commit.
    transparent_coin_version: u8,
    /// `WalletNote::serialized_version` at the defining commit; it governs the
    /// sapling and orchard note records.
    wallet_note_version: u8,
    /// `OutgoingNote::serialized_version` at the defining commit.
    outgoing_note_version: u8,
}

/// The chain-type byte `ChainType::Mainnet` writes at rows 69 and 71.
const CHAIN_TYPE_MAINNET: u8 = 0;
/// The chain name `ChainType`'s `Display` yields for mainnet at row 70.
const CHAIN_NAME_MAINNET: &str = "main";

/// The account this wallet holds; the census needs only one.
const ACCOUNT_ID: u32 = 0;
/// The birthday height, chosen inside NU6 so that the embedded consensus
/// transaction's branch identifier is the one its height implies.
const BIRTHDAY: u32 = 2_800_000;
/// The height of the wallet's single scanned block.
const BLOCK_HEIGHT: u32 = 2_800_100;
/// The wallet's single scan range covers the birthday through the scanned
/// block, inclusive.
const SCAN_RANGE_END: u32 = BLOCK_HEIGHT + 1;

/// The transaction the wallet's single block and single wallet transaction
/// both name.
const WALLET_TXID: [u8; 32] = [0xA1; 32];
/// The hash of the wallet's single scanned block.
const BLOCK_HASH: [u8; 32] = [0xB0; 32];
/// The hash of that block's predecessor.
const PREV_HASH: [u8; 32] = [0xB1; 32];
/// The sapling nullifier the nullifier map holds.
const SAPLING_NULLIFIER: [u8; 32] = [0x51; 32];
/// The orchard nullifier the nullifier map holds.
const ORCHARD_NULLIFIER: [u8; 32] = [0x01; 32];
/// The wallet's timestamp for both its block and its transaction.
const TIMESTAMP: u32 = 1_760_000_000;

/// `UnifiedKeyStore`'s `ReadableWriteable::VERSION`.
const UNIFIED_KEY_STORE_VERSION: u8 = 0;
/// `KEY_TYPE_SPEND`, the discriminant for a spending key store.
const KEY_TYPE_SPEND: u8 = 2;
/// `ReceiverSelection`'s `ReadableWriteable::VERSION`.
const RECEIVER_SELECTION_VERSION: u8 = 2;
/// A receiver selection naming both shielded receivers.
const RECEIVERS_ORCHARD_AND_SAPLING: u8 = 0b11;
/// `TransparentScope::External`, the first variant, as its `as u8` cast.
const TRANSPARENT_SCOPE_EXTERNAL: u8 = 0;
/// `zip32::Scope::External`, the first variant, as its `as u8` cast.
const ZIP32_SCOPE_EXTERNAL: u8 = 0;
/// `ConfirmationStatus::serialized_version`.
const CONFIRMATION_STATUS_VERSION: u8 = 1;
/// `ConfirmationStatus::Confirmed`'s discriminant.
const CONFIRMATION_STATUS_CONFIRMED: u8 = 0;
/// `WalletBlock::serialized_version`.
const WALLET_BLOCK_VERSION: u8 = 0;
/// `TreeBounds::serialized_version`.
const TREE_BOUNDS_VERSION: u8 = 0;
/// `WalletTransaction::serialized_version`.
const WALLET_TRANSACTION_VERSION: u8 = 0;
/// `NullifierMap::serialized_version`.
const NULLIFIER_MAP_VERSION: u8 = 1;
/// `ScanTarget::serialized_version`.
const SCAN_TARGET_VERSION: u8 = 0;
/// `ShardTrees::serialized_version`.
const SHARD_TREES_VERSION: u8 = 0;
/// `SyncState::serialized_version`.
const SYNC_STATE_VERSION: u8 = 3;
/// `ScanPriority::Scanned`'s discriminant under `SyncState` version 3, whose
/// priority list gained `RefetchingNullifiers` at position zero.
const SCAN_PRIORITY_SCANNED: u8 = 2;
/// `SyncConfig::serialized_version`.
const SYNC_CONFIG_VERSION: u8 = 1;
/// `PerformanceLevel::serialized_version`.
const PERFORMANCE_LEVEL_VERSION: u8 = 0;
/// `PerformanceLevel::High`, the default.
const PERFORMANCE_LEVEL_HIGH: u8 = 2;
/// The default transparent-address-discovery gap limit.
const GAP_LIMIT: u8 = 10;
/// The default discovery scopes: external and refund, not internal.
const DISCOVERY_SCOPES_EXTERNAL_AND_REFUND: u8 = 0b101;
/// `PriceList::serialized_version`.
const PRICE_LIST_VERSION: u8 = 0;
/// The wallet's minimum confirmations.
const MIN_CONFIRMATIONS: u32 = 1;

/// A mainnet transparent address in canonical base58 form; only its length
/// and framing matter to the grammar.
const TRANSPARENT_ADDRESS: &str = "t1Hsc1LR8yKnbbe3twRp88p6vFfC5t7DLbs";

/// The value of the wallet's transparent coin, in zatoshis.
const TRANSPARENT_VALUE: u64 = 100_000;
/// The value of the wallet's sapling note, in zatoshis.
const SAPLING_VALUE: u64 = 200_000;
/// The value of the wallet's orchard note, in zatoshis.
const ORCHARD_VALUE: u64 = 300_000;
/// The value of the wallet's outgoing sapling note, in zatoshis.
const OUTGOING_SAPLING_VALUE: u64 = 50_000;
/// The value of the wallet's outgoing orchard note, in zatoshis.
const OUTGOING_ORCHARD_VALUE: u64 = 60_000;

/// This era's fixtures, in census order.
pub fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            row: 69,
            defining_commit: "eda1dca85",
            branch: "dev",
            bytes: row_69(),
        },
        Fixture {
            row: 70,
            defining_commit: "5d8fda797",
            branch: "stable",
            bytes: row_70(),
        },
        Fixture {
            row: 71,
            defining_commit: "6ae5c270d",
            branch: "dev",
            bytes: row_71(),
        },
    ]
}

/// Row 69: `dev`'s wallet file version 40, defined by merge commit
/// `eda1dca85` (authoring interior `3f95e4520`, "fix wallet ser/deser due to
/// change to chain type fmt::Display").
///
/// This replicates `LightWallet::write` in `zingolib/src/wallet/disk.rs` at
/// that commit, together with the record writers in
/// `pepper-sync/src/wallet/serialization.rs`, `SyncConfig::write` in
/// `pepper-sync/src/config.rs`, `ConfirmationStatus::write` in
/// `zingo-status/src/confirmation_status.rs`, `PriceList::write` in
/// `zingo-price/src/lib.rs`, and the `ReadableWriteable` implementations for
/// `UnifiedKeyStore` and `ReceiverSelection` in
/// `zingolib/src/wallet/keys/unified.rs`.
///
/// The grammar-unique mark sits at byte 8: where every earlier version wrote
/// a u64-length-prefixed chain name, this writer emits a single chain-type
/// byte, `0` for mainnet. Outpoint and output indices remain `u16` here. This
/// is the grammar today's reader misparses, because it dispatches on the
/// version word alone and version 40 also names row 70's incompatible
/// `stable` grammar.
///
/// The fixture is a mainnet wallet holding one account, one unified address,
/// one transparent address, one scanned block, one wallet transaction with
/// one output of each of the five kinds, one nullifier per shielded pool, and
/// one outpoint-map entry. The outpoint-map entry is mandatory across this
/// era: its index width is the column that separates this row from rows 70
/// and 71.
fn row_69() -> Vec<u8> {
    wallet(&Grammar {
        version_word: 40,
        chain_as_byte: true,
        wide_output_index: false,
        transparent_coin_version: 0,
        wallet_note_version: 1,
        outgoing_note_version: 0,
    })
}

/// Row 70: `stable`'s independent wallet file version 40, defined by merge
/// commit `5d8fda797` ("Merge PR #2360 fix_output_id_type", authoring
/// interior `83bbd10c9`). The grammar shipped under tag
/// `zingolib_nu6_2_for_zaino_v0.4.0` and was superseded on `stable` by
/// `0d6c997a6` on 2026-06-09.
///
/// This replicates the same writers as row 69, read from `5d8fda797`'s tree.
/// Two differences distinguish it. The chain stays a u64-length-prefixed name
/// string, which `ChainType`'s `Display` renders as `"main"` for mainnet, so
/// bytes 8 through 15 are the length `4` rather than a chain-type byte. And
/// the output index widens from `u16` to `u32` at six write sites: the
/// outpoint map in `LightWallet::write`, and `TransparentCoin`,
/// `SaplingNote`, `OrchardNote`, `OutgoingSaplingNote` and
/// `OutgoingOrchardNote` in `pepper-sync/src/wallet/serialization.rs`. Each
/// widened record bumped its own inner version so that a reader can tell the
/// widths apart: `TransparentCoin` went from 0 to 1, `WalletNote` from 1 to
/// 2, and `OutgoingNote` from 0 to 1.
///
/// The wallet contents match row 69 exactly, so every differing byte is a
/// grammar difference.
fn row_70() -> Vec<u8> {
    wallet(&Grammar {
        version_word: 40,
        chain_as_byte: false,
        wide_output_index: true,
        transparent_coin_version: 1,
        wallet_note_version: 2,
        outgoing_note_version: 1,
    })
}

/// Row 71: the union, wallet file version 41, defined by merge commit
/// `6ae5c270d` (authoring interior `344dc548d`, "solve merge conflicts with
/// stable").
///
/// This replicates the same writers as rows 69 and 70, read from
/// `6ae5c270d`'s tree. It carries `dev`'s chain-type byte and `stable`'s
/// `u32` output indices at once, and it bumps the version word to 41 so that
/// the union is distinguishable from either parent. The pepper-sync record
/// writers here are byte-for-byte identical to row 70's, so this fixture also
/// exhibits the bumped inner versions — `TransparentCoin` 1, `WalletNote` 2,
/// `OutgoingNote` 1 — which appear only inside a populated wallet
/// transaction. That is why the fixture populates one output of each kind
/// rather than leaving the transaction vector empty.
///
/// The wallet contents match rows 69 and 70 exactly.
fn row_71() -> Vec<u8> {
    wallet(&Grammar {
        version_word: 41,
        chain_as_byte: true,
        wide_output_index: true,
        transparent_coin_version: 1,
        wallet_note_version: 2,
        outgoing_note_version: 1,
    })
}

/// Write the complete wallet file under `grammar`.
///
/// The field order follows `LightWallet::write` at all three defining
/// commits, which agree on everything but the chain encoding and the index
/// widths.
fn wallet(grammar: &Grammar) -> Vec<u8> {
    let mut out = Vec::new();

    push_u64_le(&mut out, grammar.version_word);

    if grammar.chain_as_byte {
        push_u8(&mut out, CHAIN_TYPE_MAINNET);
    } else {
        push_u64_string(&mut out, CHAIN_NAME_MAINNET);
    }

    // The mnemonic's entropy, written as a byte vector; 32 bytes is the
    // 24-word length. Seed material is zeroed throughout the corpus.
    push_compact_vec_u8(&mut out, &[0u8; 32]);

    push_u32_le(&mut out, BIRTHDAY);

    // The unified key store: one spending-key entry for account zero.
    push_compact_size(&mut out, 1);
    push_u32_le(&mut out, ACCOUNT_ID);
    push_u8(&mut out, UNIFIED_KEY_STORE_VERSION);
    push_u8(&mut out, KEY_TYPE_SPEND);
    push_unified_spending_key(&mut out);

    // The unified addresses: one address at index zero with both shielded
    // receivers.
    push_compact_size(&mut out, 1);
    push_u32_le(&mut out, ACCOUNT_ID);
    push_u32_le(&mut out, 0);
    push_u8(&mut out, RECEIVER_SELECTION_VERSION);
    push_u8(&mut out, RECEIVERS_ORCHARD_AND_SAPLING);

    // The transparent addresses: one external address at index zero.
    push_compact_size(&mut out, 1);
    push_u32_le(&mut out, ACCOUNT_ID);
    push_u8(&mut out, TRANSPARENT_SCOPE_EXTERNAL);
    push_u32_le(&mut out, 0);

    // The wallet blocks.
    push_compact_size(&mut out, 1);
    push_wallet_block(&mut out);

    // The wallet transactions.
    push_compact_size(&mut out, 1);
    push_wallet_transaction(&mut out, grammar);

    push_nullifier_map(&mut out);
    push_outpoint_map(&mut out, grammar);
    push_shard_trees(&mut out);
    push_sync_state(&mut out);
    push_sync_config(&mut out);

    push_u32_le(&mut out, MIN_CONFIRMATIONS);

    push_price_list(&mut out);

    out
}

/// Append an output index in the width `grammar` selects.
///
/// This is the single discriminating column of the era, and it appears at six
/// write sites: the outpoint map in `disk.rs`, and the five output records in
/// pepper-sync.
fn push_output_index(out: &mut Vec<u8>, grammar: &Grammar, index: u32) {
    if grammar.wide_output_index {
        push_u32_le(out, index);
    } else {
        push_u16_le(out, index as u16);
    }
}

/// Append a `UnifiedSpendingKey` as `ReadableWriteable for UnifiedSpendingKey`
/// writes it: a CompactSize length, then `UnifiedSpendingKey::to_bytes` under
/// the Orchard era.
///
/// The key bytes themselves are zeroed, as the corpus does for all key
/// material; only their lengths shape the grammar.
fn push_unified_spending_key(out: &mut Vec<u8>) {
    // ASSUMPTION: `UnifiedSpendingKey::to_bytes` is a dependency encoding
    // (`zcash_keys`), read from the vendored 0.13.0 source rather than from
    // the zingolib tree. It writes the era identifier, then a CompactSize
    // typecode and a CompactSize-length-prefixed key for orchard, sapling and
    // transparent in that order.
    let mut usk = Vec::new();
    // ASSUMPTION: `Era::Orchard`'s identifier is the NU5 consensus branch id.
    push_u32_le(&mut usk, 0xC2D6_D0B4);
    // ASSUMPTION: ZIP 316 typecodes: orchard 3, sapling 2, P2PKH 0.
    push_compact_size(&mut usk, 3);
    push_compact_vec_u8(&mut usk, &[0u8; 32]);
    push_compact_size(&mut usk, 2);
    // ASSUMPTION: a sapling `ExtendedSpendingKey` serializes to 169 bytes.
    push_compact_vec_u8(&mut usk, &[0u8; 169]);
    push_compact_size(&mut usk, 0);
    // ASSUMPTION: `AccountPrivKey::to_bytes` yields the 78-byte BIP 32 xprv
    // encoding less its 4-byte prefix, so 74 bytes.
    push_compact_vec_u8(&mut usk, &[0u8; 74]);

    push_compact_vec_u8(out, &usk);
}

/// Append a `WalletBlock` as `WalletBlock::write` in
/// `pepper-sync/src/wallet/serialization.rs` writes it. The block names the
/// wallet's single transaction and carries the tree bounds that bracket it.
fn push_wallet_block(out: &mut Vec<u8>) {
    push_u8(out, WALLET_BLOCK_VERSION);
    push_u32_le(out, BLOCK_HEIGHT);
    push_bytes(out, &BLOCK_HASH);
    push_bytes(out, &PREV_HASH);
    push_u32_le(out, TIMESTAMP);
    push_compact_size(out, 1);
    push_bytes(out, &WALLET_TXID);

    push_u8(out, TREE_BOUNDS_VERSION);
    push_u32_le(out, 3_000_000);
    push_u32_le(out, 3_000_001);
    push_u32_le(out, 5_000_000);
    push_u32_le(out, 5_000_002);
}

/// Append a `WalletTransaction` as `WalletTransaction::write` writes it: the
/// record version, the transaction identifier, the confirmation status, the
/// whole consensus transaction, the wallet's timestamp, and then the five
/// output vectors whose index width this era changed.
fn push_wallet_transaction(out: &mut Vec<u8>, grammar: &Grammar) {
    push_u8(out, WALLET_TRANSACTION_VERSION);
    push_bytes(out, &WALLET_TXID);

    push_u8(out, CONFIRMATION_STATUS_VERSION);
    push_u8(out, CONFIRMATION_STATUS_CONFIRMED);
    push_u32_le(out, BLOCK_HEIGHT);

    push_consensus_transaction(out);

    push_u32_le(out, TIMESTAMP);

    push_compact_size(out, 1);
    push_transparent_coin(out, grammar);
    push_compact_size(out, 1);
    push_sapling_note(out, grammar);
    push_compact_size(out, 1);
    push_orchard_note(out, grammar);
    push_compact_size(out, 1);
    push_outgoing_sapling_note(out, grammar);
    push_compact_size(out, 1);
    push_outgoing_orchard_note(out, grammar);
}

/// Append the embedded consensus transaction.
///
/// `WalletTransaction::write` calls `Transaction::write` with no length
/// prefix, so a reader must parse the consensus encoding to find where the
/// record resumes. The fixture therefore emits a well-formed but minimal
/// version 5 transaction: one transparent output, the one the wallet's
/// transparent coin claims, and no shielded bundles. Reproducing real sapling
/// and orchard bundles would mean synthesising zero-knowledge proofs, which a
/// std-only generator cannot do, and no census discriminator reads inside the
/// bundles.
fn push_consensus_transaction(out: &mut Vec<u8>) {
    // ASSUMPTION: the whole of this function is a dependency encoding
    // (`zcash_primitives::transaction::Transaction::write_v5` and
    // `zcash_transparent`'s `TxOut` and `Script`), read from the vendored
    // 0.28.0 and 0.8.0 sources. Rows 70 and 71 pin exactly those versions;
    // row 69 pins zcash_primitives 0.26.4 and zcash_transparent 0.6.3, which
    // are not vendored here, and this fixture assumes the version 5 layout is
    // unchanged between them.

    // The header: the overwintered bit set over transaction version 5, then
    // the version 5 version-group identifier.
    push_u32_le(out, 0x8000_0000 | 5);
    push_u32_le(out, 0x26A7_270A);
    // ASSUMPTION: the NU6 consensus branch identifier, which is the one
    // BLOCK_HEIGHT implies on mainnet.
    push_u32_le(out, 0xC8E7_1055);
    // The lock time and the expiry height.
    push_u32_le(out, 0);
    push_u32_le(out, BLOCK_HEIGHT + 40);

    // The transparent bundle: no inputs, one P2PKH output.
    push_compact_size(out, 0);
    push_compact_size(out, 1);
    push_i64_le(out, TRANSPARENT_VALUE as i64);
    push_compact_vec_u8(out, &p2pkh_script());

    // No sapling bundle: an empty spend vector and an empty output vector.
    push_compact_size(out, 0);
    push_compact_size(out, 0);

    // No orchard bundle: an empty action vector.
    push_compact_size(out, 0);
}

/// The 25-byte P2PKH script `OP_DUP OP_HASH160 <20 bytes> OP_EQUALVERIFY
/// OP_CHECKSIG`, with a zeroed key hash.
fn p2pkh_script() -> Vec<u8> {
    let mut script = vec![0x76, 0xA9, 0x14];
    script.extend_from_slice(&[0u8; 20]);
    script.extend_from_slice(&[0x88, 0xAC]);
    script
}

/// Append a `TransparentCoin` as `TransparentCoin::write` writes it. Its
/// record version is 0 at row 69 and 1 at rows 70 and 71, and that version is
/// what tells a reader whether the output index that follows is two bytes or
/// four.
fn push_transparent_coin(out: &mut Vec<u8>, grammar: &Grammar) {
    push_u8(out, grammar.transparent_coin_version);
    push_bytes(out, &WALLET_TXID);
    push_output_index(out, grammar, 0);

    push_u32_le(out, ACCOUNT_ID);
    push_u8(out, TRANSPARENT_SCOPE_EXTERNAL);
    push_u32_le(out, 0);

    // The address uses the historical u64-length string framing, which
    // pepper-sync keeps in its own `write_string`.
    push_u64_string(out, TRANSPARENT_ADDRESS);
    push_compact_vec_u8(out, &p2pkh_script());
    push_u64_le(out, TRANSPARENT_VALUE);
    push_optional_none(out);
}

/// Append a `SaplingNote` as `SaplingNote::write` writes it. The record
/// version is `WalletNote::serialized_version`, 1 at row 69 and 2 at rows 70
/// and 71.
///
/// The note has a nullifier but no commitment-tree position, which is the
/// state of a note found by a scan whose shard trees have not yet been built.
/// The fixture's shard trees are correspondingly empty.
fn push_sapling_note(out: &mut Vec<u8>, grammar: &Grammar) {
    push_u8(out, grammar.wallet_note_version);
    push_bytes(out, &WALLET_TXID);
    push_output_index(out, grammar, 0);

    push_u32_le(out, ACCOUNT_ID);
    push_u8(out, ZIP32_SCOPE_EXTERNAL);

    // The sapling payment address is 43 bytes.
    push_bytes(out, &[0u8; 43]);
    push_u64_le(out, SAPLING_VALUE);
    // An after-ZIP-212 rseed, then its 32 bytes.
    push_u8(out, 1);
    push_bytes(out, &[0u8; 32]);

    push_optional_some(out);
    push_bytes(out, &SAPLING_NULLIFIER);
    push_optional_none(out);
    push_empty_memo(out);
    push_optional_none(out);

    // The refetch-nullifier ranges, present from `WalletNote` version 1.
    push_compact_size(out, 0);
}

/// Append an `OrchardNote` as `OrchardNote::write` writes it. It shares
/// `WalletNote`'s record version with the sapling note and differs in
/// carrying a rho alongside its rseed.
fn push_orchard_note(out: &mut Vec<u8>, grammar: &Grammar) {
    push_u8(out, grammar.wallet_note_version);
    push_bytes(out, &WALLET_TXID);
    push_output_index(out, grammar, 0);

    push_u32_le(out, ACCOUNT_ID);
    push_u8(out, ZIP32_SCOPE_EXTERNAL);

    // The orchard raw address is 43 bytes.
    push_bytes(out, &[0u8; 43]);
    push_u64_le(out, ORCHARD_VALUE);
    push_bytes(out, &[0u8; 32]);
    push_bytes(out, &[0u8; 32]);

    push_optional_some(out);
    push_bytes(out, &ORCHARD_NULLIFIER);
    push_optional_none(out);
    push_empty_memo(out);
    push_optional_none(out);

    push_compact_size(out, 0);
}

/// Append an `OutgoingSaplingNote` as `OutgoingSaplingNote::write` writes it.
/// Its record version is `OutgoingNote::serialized_version`, 0 at row 69 and
/// 1 at rows 70 and 71.
fn push_outgoing_sapling_note(out: &mut Vec<u8>, grammar: &Grammar) {
    push_u8(out, grammar.outgoing_note_version);
    push_bytes(out, &WALLET_TXID);
    push_output_index(out, grammar, 0);

    push_u32_le(out, ACCOUNT_ID);
    push_u8(out, ZIP32_SCOPE_EXTERNAL);

    push_bytes(out, &[0u8; 43]);
    push_u64_le(out, OUTGOING_SAPLING_VALUE);
    push_u8(out, 1);
    push_bytes(out, &[0u8; 32]);

    push_empty_memo(out);
    // No recorded full unified address for the recipient.
    push_optional_none(out);
}

/// Append an `OutgoingOrchardNote` as `OutgoingOrchardNote::write` writes it.
fn push_outgoing_orchard_note(out: &mut Vec<u8>, grammar: &Grammar) {
    push_u8(out, grammar.outgoing_note_version);
    push_bytes(out, &WALLET_TXID);
    push_output_index(out, grammar, 0);

    push_u32_le(out, ACCOUNT_ID);
    push_u8(out, ZIP32_SCOPE_EXTERNAL);

    push_bytes(out, &[0u8; 43]);
    push_u64_le(out, OUTGOING_ORCHARD_VALUE);
    push_bytes(out, &[0u8; 32]);
    push_bytes(out, &[0u8; 32]);

    push_empty_memo(out);
    push_optional_none(out);
}

/// Append the 512-byte ZIP 302 encoding of an absent memo.
fn push_empty_memo(out: &mut Vec<u8>) {
    // ASSUMPTION: `MemoBytes::empty` is a dependency encoding
    // (`zcash_protocol`), read from the vendored 0.9.0 source: the marker
    // byte 0xF6 followed by 511 zero bytes.
    push_u8(out, 0xF6);
    push_bytes(out, &[0u8; 511]);
}

/// Append the `NullifierMap` as `NullifierMap::write` writes it: the record
/// version, then the sapling and orchard maps, each a vector of a raw 32-byte
/// nullifier followed by the scan target that will resolve it.
fn push_nullifier_map(out: &mut Vec<u8>) {
    push_u8(out, NULLIFIER_MAP_VERSION);

    push_compact_size(out, 1);
    push_bytes(out, &SAPLING_NULLIFIER);
    push_scan_target(out);

    push_compact_size(out, 1);
    push_bytes(out, &ORCHARD_NULLIFIER);
    push_scan_target(out);
}

/// Append the outpoint map as `LightWallet::write` writes it: a vector whose
/// elements are a raw transaction identifier, the output index, and a scan
/// target.
///
/// This is the write site the census reads first. Row 69 emits a `u16` index
/// here and rows 70 and 71 emit a `u32`, and unlike the pepper-sync records
/// this one carries no inner version to announce the change.
fn push_outpoint_map(out: &mut Vec<u8>, grammar: &Grammar) {
    push_compact_size(out, 1);
    push_bytes(out, &WALLET_TXID);
    push_output_index(out, grammar, 0);
    push_scan_target(out);
}

/// Append a `ScanTarget` as `ScanTarget::write` writes it.
fn push_scan_target(out: &mut Vec<u8>) {
    push_u8(out, SCAN_TARGET_VERSION);
    push_u32_le(out, BLOCK_HEIGHT);
    push_bytes(out, &WALLET_TXID);
    push_u8(out, 1);
}

/// Append the `ShardTrees` as `ShardTrees::write` writes them: the record
/// version, then the sapling and orchard trees in turn.
fn push_shard_trees(out: &mut Vec<u8>) {
    push_u8(out, SHARD_TREES_VERSION);
    push_empty_shard_tree(out);
    push_empty_shard_tree(out);
}

/// Append one empty memory-backed shard tree: no shards, no checkpoints, and
/// an empty cap.
fn push_empty_shard_tree(out: &mut Vec<u8>) {
    push_compact_size(out, 0);
    push_compact_size(out, 0);
    // ASSUMPTION: `write_shard` is a dependency encoding
    // (`zcash_client_backend::serialization::shardtree`), read from the
    // vendored 0.23.0 source: the serialization version 1, then the tree,
    // whose empty form is the single nil tag 0. `MemoryShardStore::empty`
    // starts with an empty cap, so that is what an unsynced wallet writes.
    push_u8(out, 1);
    push_u8(out, 0);
}

/// Append the `SyncState` as `SyncState::write` writes it: the record
/// version, the scan ranges, the sapling and orchard shard ranges, and the
/// scan targets.
fn push_sync_state(out: &mut Vec<u8>) {
    push_u8(out, SYNC_STATE_VERSION);

    push_compact_size(out, 1);
    push_u32_le(out, BIRTHDAY);
    push_u32_le(out, SCAN_RANGE_END);
    push_u8(out, SCAN_PRIORITY_SCANNED);

    push_compact_size(out, 0);
    push_compact_size(out, 0);
    push_compact_size(out, 0);
}

/// Append the `SyncConfig` as `SyncConfig::write` in
/// `pepper-sync/src/config.rs` writes it: the record version, the gap limit,
/// the discovery scope bitfield, and the nested performance level.
fn push_sync_config(out: &mut Vec<u8>) {
    push_u8(out, SYNC_CONFIG_VERSION);
    push_u8(out, GAP_LIMIT);
    push_u8(out, DISCOVERY_SCOPES_EXTERNAL_AND_REFUND);
    push_u8(out, PERFORMANCE_LEVEL_VERSION);
    push_u8(out, PERFORMANCE_LEVEL_HIGH);
}

/// Append the `PriceList` as `PriceList::write` in `zingo-price/src/lib.rs`
/// writes it. A wallet that has never fetched a price writes the record
/// version, two absent optionals, and an empty vector.
fn push_price_list(out: &mut Vec<u8>) {
    push_u8(out, PRICE_LIST_VERSION);
    push_optional_none(out);
    push_optional_none(out);
    push_compact_size(out, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read the version word a fixture writes at offset 0.
    fn version_word(bytes: &[u8]) -> u64 {
        u64::from_le_bytes(bytes[..8].try_into().expect("fixture has a version word"))
    }

    #[test]
    fn the_era_covers_rows_69_through_71_in_order() {
        let rows: Vec<u8> = fixtures().iter().map(|fixture| fixture.row).collect();
        assert_eq!(rows, vec![69, 70, 71]);
    }

    /// The two mutually unreadable version 40s and the version 41 union.
    #[test]
    fn version_words_are_40_40_and_41() {
        assert_eq!(version_word(&row_69()), 40);
        assert_eq!(version_word(&row_70()), 40);
        assert_eq!(version_word(&row_71()), 41);
    }

    /// The census's motivating defect in one assertion: the two version 40s
    /// diverge at byte 8, where `dev` writes a chain-type byte and `stable`
    /// writes the first byte of a u64 string length.
    #[test]
    fn the_two_version_40s_diverge_at_byte_8() {
        let dev = row_69();
        let stable = row_70();

        assert_eq!(dev[8], 0x00, "row 69 writes the mainnet chain-type byte");

        let length = u64::from_le_bytes(
            stable[8..16]
                .try_into()
                .expect("row 70 writes a u64 string length"),
        );
        assert_eq!(length, CHAIN_NAME_MAINNET.len() as u64);
        assert_eq!(&stable[16..20], CHAIN_NAME_MAINNET.as_bytes());
    }

    /// Row 71 keeps `dev`'s chain-type byte alongside `stable`'s widths.
    #[test]
    fn the_union_keeps_the_chain_type_byte() {
        assert_eq!(row_71()[8], 0x00);
    }

    /// The wide index costs two bytes at each of the six write sites, and
    /// rows 69 and 71 share the chain-type byte, so the union is exactly
    /// twelve bytes longer than row 69.
    #[test]
    fn widening_the_output_index_adds_two_bytes_at_six_sites() {
        assert_eq!(row_71().len() - row_69().len(), 12);
    }

    /// Each row is a distinguishable grammar, which is the census's claim.
    #[test]
    fn the_three_rows_are_pairwise_distinct() {
        let fixtures = fixtures();
        for (index, earlier) in fixtures.iter().enumerate() {
            for later in &fixtures[index + 1..] {
                assert_ne!(
                    earlier.bytes, later.bytes,
                    "rows {} and {} produced identical bytes",
                    earlier.row, later.row
                );
            }
        }
    }
}
