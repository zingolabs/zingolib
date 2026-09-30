//! Census rows 22 through 39: the zecwallet-light-cli era, from the
//! collapse of the two sapling key vectors into one `Vector<WalletZKey>`
//! through the Blazesync restructure and on into the orchard additions of
//! 2022.
//!
//! Every fixture in this module is derived from the writer source at its
//! Defining Commit, read with `git show <commit>:<writer-path>` and the
//! sub-writers that file calls. Three file layouts appear here.
//!
//! Rows 22 through 29 use the flat layout, in which `LightWallet::write` in
//! `lib/src/lightwallet.rs` emits the key material inline. Rows 30 through
//! 35 use the layout Blazesync introduced at `87ad71c28`, in which a `Keys`
//! record holds the key material while the block and transaction sets
//! delegate to their own writers. Rows 36 through 39 continue that layout
//! with a `Keys` record that has absorbed the transparent addresses into
//! `WalletTKey` records and, from row 38, gained an orchard key vector; at
//! `a6f8a0bd6` the writer's home moves from `lib/src/lightwallet.rs` to
//! `lib/src/wallet.rs`.
//!
//! The wallet these fixtures describe is the same throughout: unencrypted,
//! born at height 1000000 on chain "main", holding one HD sapling key, one
//! transparent key with its address, one block, and one transaction that
//! carries one sapling note and one transparent output. From row 38, where
//! the grammar gains orchard vectors, it also holds one orchard key and one
//! orchard note. Key and seed material is zeroed; identifiers and hashes
//! carry repeated marker bytes so that a hexdump reads easily; opaque
//! length-prefixed blobs carry the encodings their own writers would have
//! produced.

use super::util::{
    push_bytes, push_compact_size, push_i32_le, push_optional_none, push_optional_some,
    push_u32_le, push_u64_le, push_u64_string, push_u8,
};
use super::Fixture;

/// The chain this wallet was configured for; `write_string` framed. From
/// `a6f8a0bd6` the writer takes the string from `Network`'s `Display`
/// implementation, whose mainnet arm still renders "main".
const CHAIN_NAME: &str = "main";

/// The wallet's birthday, written as a little-endian u64.
const BIRTHDAY: u64 = 1_000_000;

/// The height of the single block and of the single transaction.
const BLOCK_HEIGHT: i32 = 1_000_000;

/// The block's Unix mining time, and the transaction's datetime.
const BLOCK_TIME: u32 = 1_600_000_000;

/// A serialized zip32 `ExtendedSpendingKey` or `ExtendedFullViewingKey`.
/// Both records occupy the same width: a depth byte, a four-byte parent
/// tag, a four-byte child index, a thirty-two-byte chain code, a
/// ninety-six-byte key body, and a thirty-two-byte diversifier key.
const EXTENDED_KEY_LEN: usize = 169;

/// A serialized orchard `FullViewingKey`.
const ORCHARD_FVK_LEN: usize = 96;

/// The width of the encrypted-seed field, which the writer emits raw and
/// unprefixed whether or not the wallet is encrypted.
const ENC_SEED_LEN: usize = 48;

/// The width of the plaintext seed field, and of a secp256k1 or orchard
/// spending key.
const SEED_LEN: usize = 32;

/// The single transaction's identifier.
const TXID: [u8; 32] = [0x11; 32];

/// The single sapling note's nullifier.
const NULLIFIER: [u8; 32] = [0x22; 32];

/// The single block's hash.
const BLOCK_HASH: [u8; 32] = [0x33; 32];

/// The single block's predecessor's hash.
const PREV_BLOCK_HASH: [u8; 32] = [0x44; 32];

/// The single note commitment carried by the block's compact transaction.
const CMU: [u8; 32] = [0x55; 32];

/// A note's diversifier. Sapling and orchard diversifiers are both eleven
/// bytes wide.
const DIVERSIFIER: [u8; 11] = [0x66; 11];

/// The sapling note's random seed material, written bare before row 25 and
/// behind a type tag from row 25 onward.
const RSEED: [u8; 32] = [0x77; 32];

/// The orchard note's rho, written inside the note record.
const ORCHARD_RHO: [u8; 32] = [0x88; 32];

/// The orchard note's random seed.
const ORCHARD_RSEED: [u8; 32] = [0x99; 32];

/// The orchard note's nullifier, written beside the note record.
const ORCHARD_NULLIFIER: [u8; 32] = [0xAA; 32];

/// The value of the single sapling note, in zatoshi.
const NOTE_VALUE: u64 = 100_000;

/// The value of the single orchard note, in zatoshi.
const ORCHARD_NOTE_VALUE: u64 = 25_000;

/// The value of the single transparent output, in zatoshi.
const UTXO_VALUE: u64 = 50_000;

/// The wallet's single transparent address. The reader asserts that a
/// UTXO's address begins with `t`, so the fixture honors that.
const TADDR: &str = "t1FixtureTransparentAddress00000000";

/// The transparent output's script: the opening bytes of a
/// pay-to-pubkey-hash script, enough to exercise the length-prefixed
/// script vector.
const SCRIPT: [u8; 3] = [0x76, 0xa9, 0x14];

/// The `Rseed::AfterZip212` type tag, which `write_rseed` emits from
/// `28b795139` onward.
const RSEED_TAG_AFTER_ZIP212: u8 = 2;

/// `MemoDownloadOption::WalletMemos`, the default the `WalletOptions`
/// record carries from `7f59c5320` onward.
const MEMO_DOWNLOAD_WALLET_MEMOS: u8 = 1;

/// `MAX_TRANSACTION_SIZE_DEFAULT`, the transaction-size filter that
/// `2e8b86670` introduces and defaults to.
const TRANSACTION_SIZE_FILTER: u32 = 500;

/// This era's fixtures, in census order.
pub fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            row: 22,
            defining_commit: "fb1135328",
            branch: "dev",
            bytes: row_22(),
        },
        Fixture {
            row: 23,
            defining_commit: "49ee4c406",
            branch: "dev",
            bytes: row_23(),
        },
        Fixture {
            row: 24,
            defining_commit: "8e425fc6b",
            branch: "dev",
            bytes: row_24(),
        },
        Fixture {
            row: 25,
            defining_commit: "28b795139",
            branch: "dev",
            bytes: row_25(),
        },
        Fixture {
            row: 26,
            defining_commit: "b61175345",
            branch: "dev",
            bytes: row_26(),
        },
        Fixture {
            row: 27,
            defining_commit: "bcf38a6fa",
            branch: "dev",
            bytes: row_27(),
        },
        Fixture {
            row: 28,
            defining_commit: "7212e2bf1",
            branch: "dev",
            bytes: row_28(),
        },
        Fixture {
            row: 29,
            defining_commit: "4a279179f",
            branch: "dev",
            bytes: row_29(),
        },
        Fixture {
            row: 30,
            defining_commit: "87ad71c28",
            branch: "dev",
            bytes: row_30(),
        },
        Fixture {
            row: 31,
            defining_commit: "ead95fe0a",
            branch: "dev",
            bytes: row_31(),
        },
        Fixture {
            row: 32,
            defining_commit: "0cd53900b",
            branch: "dev",
            bytes: row_32(),
        },
        Fixture {
            row: 33,
            defining_commit: "a1b9b0bbe",
            branch: "dev",
            bytes: row_33(),
        },
        Fixture {
            row: 34,
            defining_commit: "ed3b21c09",
            branch: "dev",
            bytes: row_34(),
        },
        Fixture {
            row: 35,
            defining_commit: "7f59c5320",
            branch: "dev",
            bytes: row_35(),
        },
        Fixture {
            row: 36,
            defining_commit: "5e73adef4",
            branch: "dev",
            bytes: row_36(),
        },
        Fixture {
            row: 37,
            defining_commit: "a6f8a0bd6",
            branch: "dev",
            bytes: row_37(),
        },
        Fixture {
            row: 38,
            defining_commit: "6dd62d5e2",
            branch: "dev",
            bytes: row_38(),
        },
        Fixture {
            row: 39,
            defining_commit: "2e8b86670",
            branch: "dev",
            bytes: row_39(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Dependency-derived encodings
// ---------------------------------------------------------------------------

/// Append a serialized zip32 extended key, spending or viewing.
//
// ASSUMPTION: librustzcash's `ExtendedSpendingKey::write` and
// `ExtendedFullViewingKey::write` are not vendored in this repository, so
// their widths come from the stable zip32 encoding: depth u8, parent
// fingerprint tag 4 bytes, child index u32, chain code 32 bytes, key body
// 96 bytes, diversifier key 32 bytes, for 169 bytes each. All-zero material
// is legal to write, since neither writer validates what it is given.
fn push_extended_key(out: &mut Vec<u8>) {
    push_bytes(out, &[0u8; EXTENDED_KEY_LEN]);
}

/// Append a serialized orchard full viewing key.
//
// ASSUMPTION: the `orchard` crate is not vendored here. Its
// `FullViewingKey::write` emits the raw ninety-six-byte encoding, the
// concatenation of `ak`, `nk`, and `rivk` at thirty-two bytes each, which
// the matching reader at `6dd62d5e2` confirms by reading `[0; 96]`.
fn push_orchard_full_viewing_key(out: &mut Vec<u8>) {
    push_bytes(out, &[0u8; ORCHARD_FVK_LEN]);
}

/// Append the serialization of an empty sapling `CommitmentTree`.
//
// ASSUMPTION: librustzcash's `CommitmentTree::write` emits
// `Optional<left>`, `Optional<right>`, and a `Vector<Optional<Node>>` of
// parents. An empty tree therefore serializes as three zero bytes: two
// `None` markers and a zero-length vector.
fn push_empty_commitment_tree(out: &mut Vec<u8>) {
    push_optional_none(out);
    push_optional_none(out);
    push_compact_size(out, 0);
}

/// Append a protobuf base-128 varint.
//
// ASSUMPTION: the block and tree-state blobs are produced by `prost`, whose
// encoding this helper and its callers reproduce: a key varint holding the
// field number and the wire type, then the payload, with proto3 default
// values omitted entirely.
fn push_proto_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        if value < 0x80 {
            push_u8(out, value as u8);
            return;
        }
        push_u8(out, ((value & 0x7F) as u8) | 0x80);
        value >>= 7;
    }
}

/// Append a protobuf field key: the field number and the wire type.
fn push_proto_key(out: &mut Vec<u8>, field: u64, wire_type: u64) {
    push_proto_varint(out, (field << 3) | wire_type);
}

/// Append a protobuf varint field, omitting it when the value is the
/// proto3 default.
fn push_proto_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    if value == 0 {
        return;
    }
    push_proto_key(out, field, 0);
    push_proto_varint(out, value);
}

/// Append a protobuf length-delimited field, omitting it when the payload
/// is empty.
fn push_proto_bytes_field(out: &mut Vec<u8>, field: u64, payload: &[u8]) {
    if payload.is_empty() {
        return;
    }
    push_proto_key(out, field, 2);
    push_proto_varint(out, payload.len() as u64);
    push_bytes(out, payload);
}

/// The protobuf encoding of the `CompactBlock` this wallet's single block
/// stores, as `lib/proto/compact_formats.proto` defines it at `87ad71c28`:
/// height in field 2, hash in field 3, prevHash in field 4, time in field
/// 5, and the compact transactions in field 7. `BlockData::new` clears the
/// header and each output's ciphertext and ephemeral key before encoding,
/// so the fixture omits them too.
fn compact_block_protobuf() -> Vec<u8> {
    let mut spend = Vec::new();
    push_proto_bytes_field(&mut spend, 1, &NULLIFIER);

    let mut output = Vec::new();
    push_proto_bytes_field(&mut output, 1, &CMU);

    let mut transaction = Vec::new();
    push_proto_bytes_field(&mut transaction, 2, &TXID);
    push_proto_bytes_field(&mut transaction, 4, &spend);
    push_proto_bytes_field(&mut transaction, 5, &output);

    let mut block = Vec::new();
    push_proto_varint_field(&mut block, 2, BLOCK_HEIGHT as u64);
    push_proto_bytes_field(&mut block, 3, &BLOCK_HASH);
    push_proto_bytes_field(&mut block, 4, &PREV_BLOCK_HASH);
    push_proto_varint_field(&mut block, 5, u64::from(BLOCK_TIME));
    push_proto_bytes_field(&mut block, 7, &transaction);
    block
}

/// The protobuf encoding of the `TreeState` that rows 32 onward store
/// behind an `Optional`, as `lib/proto/service.proto` defines it at
/// `0cd53900b`: network in field 1, height in field 2, hash in field 3,
/// time in field 4, and the hex-encoded sapling commitment tree in field 5.
fn tree_state_protobuf() -> Vec<u8> {
    let mut empty_tree = Vec::new();
    push_empty_commitment_tree(&mut empty_tree);
    let tree_hex: String = empty_tree.iter().map(|b| format!("{:02x}", b)).collect();
    let hash_hex: String = BLOCK_HASH.iter().map(|b| format!("{:02x}", b)).collect();

    let mut state = Vec::new();
    push_proto_bytes_field(&mut state, 1, CHAIN_NAME.as_bytes());
    push_proto_varint_field(&mut state, 2, BIRTHDAY);
    push_proto_bytes_field(&mut state, 3, hash_hex.as_bytes());
    push_proto_varint_field(&mut state, 4, u64::from(BLOCK_TIME));
    push_proto_bytes_field(&mut state, 5, tree_hex.as_bytes());
    state
}

// ---------------------------------------------------------------------------
// Sub-records shared across the era
// ---------------------------------------------------------------------------

/// Append one `WalletZKey`, replicating `WalletZKey::write` in
/// `lib/src/lightwallet/walletzkey.rs` and, from `a6f8a0bd6`, in
/// `lib/src/wallet/keys/sapling.rs`. That writer is byte-identical across
/// every row in this module. The fixture's key is an HD key, unlocked,
/// carrying both its spending key and its viewing key, at HD index zero.
fn push_wallet_zkey(out: &mut Vec<u8>) {
    push_u8(out, 1); // The record's own version byte.
    push_u32_le(out, 0); // WalletZKeyType::HdKey.
    push_u8(out, 0); // Not locked.
    push_optional_some(out); // The spending key is present.
    push_extended_key(out);
    push_extended_key(out); // The full viewing key, written unconditionally.
    push_optional_some(out); // The HD key number is present.
    push_u32_le(out, 0);
    push_optional_none(out); // No encrypted key.
    push_optional_none(out); // No nonce.
}

/// Append one `WalletTKey`, replicating `WalletTKey::write` in
/// `lib/src/lightwallet/wallettkey.rs` at `5e73adef4` and, from
/// `a6f8a0bd6`, in `lib/src/wallet/keys/transparent.rs`. The record folds
/// the transparent address into the key, which is why the `Keys` record can
/// drop its separate address vector. The fixture's key is an HD key,
/// unlocked, at HD index zero.
fn push_wallet_tkey(out: &mut Vec<u8>) {
    push_u8(out, 1); // The record's own version byte.
    push_u32_le(out, 0); // WalletTKeyType::HdKey.
    push_u8(out, 0); // Not locked.
    push_optional_some(out); // The secret key is present.
    push_bytes(out, &[0u8; SEED_LEN]);
    push_u64_string(out, TADDR);
    push_optional_some(out); // The HD key number is present.
    push_u32_le(out, 0);
    push_optional_none(out); // No encrypted key.
    push_optional_none(out); // No nonce.
}

/// Append one `WalletOKey`, replicating `OrchardKey::write` in
/// `lib/src/wallet/keys/orchard.rs` at `6dd62d5e2`. Note that this record's
/// own version byte is 0, not 1 as its sapling and transparent siblings
/// use, and that it writes a key-type discriminant byte rather than the u32
/// the older records write. The unified address is derived on read and
/// never serialized.
fn push_wallet_okey(out: &mut Vec<u8>) {
    push_u8(out, 0); // The record's own version byte.
    push_u8(out, 0); // Not locked.
    push_u8(out, 0); // WalletOKeyInner::HdKey.
    push_bytes(out, &[0u8; SEED_LEN]); // The orchard spending key.
    push_optional_some(out); // The HD key number is present.
    push_u32_le(out, 0);
    push_optional_none(out); // No encrypted key.
    push_optional_none(out); // No nonce.
}

/// Append the `WalletZecPriceInfo` record, replicating its writer in
/// `lib/src/lightwallet/data.rs` at `4a279179f`. That writer is unchanged
/// through `2e8b86670` apart from its version word, which moves from 1 to
/// 20 at the Blazesync restructure. The fixture has never fetched a
/// historical price.
fn push_price_info(out: &mut Vec<u8>, version: u64) {
    push_u64_le(out, version);
    push_optional_none(out); // last_historical_prices_fetched_at.
    push_u64_le(out, 0); // historical_prices_retry_count.
}

/// Append the `WalletOptions` record, replicating its writer in
/// `lib/src/lightwallet.rs` at `7f59c5320` and in `lib/src/wallet.rs` at
/// `2e8b86670`, which raises the record to version 2 and appends an
/// `Optional<u32>` transaction-size filter. The fixture downloads memos for
/// its own transactions and keeps the default filter.
fn push_wallet_options(out: &mut Vec<u8>, version: u64) {
    push_u64_le(out, version);
    push_u8(out, MEMO_DOWNLOAD_WALLET_MEMOS);
    if version >= 2 {
        push_optional_some(out);
        push_u32_le(out, TRANSACTION_SIZE_FILTER);
    }
}

// ---------------------------------------------------------------------------
// The flat layout: rows 22 through 29
// ---------------------------------------------------------------------------

/// Append one `BlockData` in the flat era's encoding, replicating
/// `BlockData::write` in `lib/src/lightwallet/data.rs`, which is unchanged
/// from `fb1135328` through `4a279179f`: the height, the block hash, the
/// commitment tree, and a literal end tag of 11.
fn push_flat_block(out: &mut Vec<u8>) {
    push_i32_le(out, BLOCK_HEIGHT);
    push_bytes(out, &BLOCK_HASH);
    push_empty_commitment_tree(out);
    push_u64_le(out, 11);
}

/// Append one `SaplingNoteData` as the writer at the given row's Defining
/// Commit emits it. The record's own version word tracks the row: 1 at
/// `fb1135328`, 2 once `49ee4c406` adds `spent_at_height`, 3 once
/// `8e425fc6b` adds the spendability flag, 4 once `28b795139` tags the note
/// randomness, and 5 once `7212e2bf1` adds the unconfirmed-spend option.
/// The note is unspent, has no memo, is not change, and is spendable.
fn push_flat_note(out: &mut Vec<u8>, row: u8) {
    let version: u64 = match row {
        22 => 1,
        23 => 2,
        24 => 3,
        25..=27 => 4,
        _ => 5,
    };
    push_u64_le(out, version);
    push_u64_le(out, 0); // The account index.
    push_extended_key(out); // The note's extended full viewing key.
    push_bytes(out, &DIVERSIFIER);
    push_u64_le(out, NOTE_VALUE);
    if row >= 25 {
        // `28b795139` replaced the bare randomness with a tagged `Rseed`.
        push_u8(out, RSEED_TAG_AFTER_ZIP212);
    }
    push_bytes(out, &RSEED);
    push_compact_size(out, 0); // No witnesses are retained.
    push_bytes(out, &NULLIFIER);
    push_optional_none(out); // spent.
    if row >= 23 {
        push_optional_none(out); // spent_at_height, added at `49ee4c406`.
    }
    if row >= 28 {
        push_optional_none(out); // unconfirmed_spent, added at `7212e2bf1`.
    }
    push_optional_none(out); // memo.
    push_u8(out, 0); // is_change.
    if row >= 24 {
        // `8e425fc6b` wrote this as `is_spendable`; `b61175345` renamed it
        // to `have_spending_key` without changing its width or position.
        push_u8(out, 1);
    }
}

/// Append one `Utxo` as the writer at the given row's Defining Commit
/// emits it. The record's own version word moves from 1 to 2 at
/// `b61175345`, which adds `spent_at_height`, and to 3 at `7212e2bf1`,
/// which adds the unconfirmed-spend option. The address is framed by a u32
/// length here, not by the u64 `write_string` framing that the top-level
/// address vector uses.
fn push_flat_utxo(out: &mut Vec<u8>, row: u8) {
    let version: u64 = match row {
        22..=25 => 1,
        26 | 27 => 2,
        _ => 3,
    };
    push_u64_le(out, version);
    push_u32_le(out, TADDR.len() as u32);
    push_bytes(out, TADDR.as_bytes());
    push_bytes(out, &TXID);
    push_u64_le(out, 0); // output_index.
    push_u64_le(out, UTXO_VALUE);
    push_i32_le(out, BLOCK_HEIGHT);
    push_compact_size(out, SCRIPT.len() as u64);
    push_bytes(out, &SCRIPT);
    push_optional_none(out); // spent.
    if row >= 26 {
        push_optional_none(out); // spent_at_height, added at `b61175345`.
    }
    if row >= 28 {
        push_optional_none(out); // unconfirmed_spent, added at `7212e2bf1`.
    }
}

/// Append one `WalletTx` in the flat era's encoding. The record's version
/// word is 4 through `7212e2bf1` and 5 from `4a279179f`, which appends the
/// per-transaction ZEC price.
fn push_flat_wallet_tx(out: &mut Vec<u8>, row: u8) {
    let version: u64 = if row >= 29 { 5 } else { 4 };
    push_u64_le(out, version);
    push_i32_le(out, BLOCK_HEIGHT);
    push_u64_le(out, u64::from(BLOCK_TIME)); // datetime.
    push_bytes(out, &TXID);
    push_compact_size(out, 1);
    push_flat_note(out, row);
    push_compact_size(out, 1);
    push_flat_utxo(out, row);
    push_u64_le(out, 0); // total_shielded_value_spent.
    push_u64_le(out, 0); // total_transparent_value_spent.
    push_compact_size(out, 0); // No outgoing metadata.
    push_u8(out, 1); // full_tx_scanned.
    if row >= 29 {
        push_optional_none(out); // zec_price, added at `4a279179f`.
    }
}

/// Build a flat-layout wallet file for one of rows 22 through 29,
/// replicating `LightWallet::write` in `lib/src/lightwallet.rs` at that
/// row's Defining Commit.
fn flat_layout(row: u8) -> Vec<u8> {
    let version: u64 = match row {
        22 => 7,
        23 => 8,
        24 => 9,
        25 => 10,
        26 => 12,
        27 | 28 => 13,
        _ => 14,
    };
    let mut out = Vec::new();
    push_u64_le(&mut out, version);
    push_u8(&mut out, 0); // Not encrypted.
    push_bytes(&mut out, &[0u8; ENC_SEED_LEN]); // enc_seed, raw and unprefixed.
    push_compact_size(&mut out, 0); // The nonce vector is empty.
    push_bytes(&mut out, &[0u8; SEED_LEN]); // The seed, raw and unprefixed.
    push_compact_size(&mut out, 1); // One `WalletZKey`.
    push_wallet_zkey(&mut out);
    push_compact_size(&mut out, 1); // One transparent key, 32 raw bytes.
    push_bytes(&mut out, &[0u8; SEED_LEN]);
    push_compact_size(&mut out, 1); // One transparent address.
    push_u64_string(&mut out, TADDR);
    push_compact_size(&mut out, 1); // One block.
    push_flat_block(&mut out);
    push_compact_size(&mut out, 1); // One transaction, keyed by its txid.
    push_bytes(&mut out, &TXID);
    push_flat_wallet_tx(&mut out, row);
    push_u64_string(&mut out, CHAIN_NAME);
    push_u64_le(&mut out, BIRTHDAY);
    if row >= 27 {
        push_u8(&mut out, 0); // sapling_tree_verified, added at `bcf38a6fa`.
    }
    if row >= 29 {
        push_price_info(&mut out, 1);
    }
    out
}

// ---------------------------------------------------------------------------
// The Blazesync layout: rows 30 through 39
// ---------------------------------------------------------------------------

/// The `Keys` record at the given row, replicating `Keys::write` in
/// `lib/src/lightwallet/keys.rs` and, from `a6f8a0bd6`, in
/// `lib/src/wallet/keys.rs`. Version 20 at `87ad71c28` writes the sapling
/// keys, the raw transparent secret keys, and the transparent addresses as
/// three separate vectors. Version 21 at `5e73adef4` collapses the last two
/// into one `Vector<WalletTKey>`. Version 22 at `6dd62d5e2` inserts a
/// `Vector<WalletOKey>` between the sapling and transparent vectors.
fn keys_record(row: u8) -> Vec<u8> {
    let version: u64 = match row {
        30..=35 => 20,
        36 | 37 => 21,
        _ => 22,
    };
    let mut out = Vec::new();
    push_u64_le(&mut out, version);
    push_u8(&mut out, 0); // Not encrypted.
    push_bytes(&mut out, &[0u8; ENC_SEED_LEN]);
    push_compact_size(&mut out, 0); // The nonce vector is empty.
    push_bytes(&mut out, &[0u8; SEED_LEN]);
    push_compact_size(&mut out, 1); // One sapling key.
    push_wallet_zkey(&mut out);
    if version >= 22 {
        push_compact_size(&mut out, 1); // One orchard key.
        push_wallet_okey(&mut out);
    }
    if version >= 21 {
        push_compact_size(&mut out, 1); // One `WalletTKey`, address included.
        push_wallet_tkey(&mut out);
    } else {
        push_compact_size(&mut out, 1); // One transparent key, 32 raw bytes.
        push_bytes(&mut out, &[0u8; SEED_LEN]);
        push_compact_size(&mut out, 1); // One transparent address.
        push_u64_string(&mut out, TADDR);
    }
    out
}

/// Append one `BlockData` in the opaque-blob encoding, replicating
/// `BlockData::write` in `lib/src/lightwallet/data.rs` at `87ad71c28`: the
/// height, the hash, an empty commitment tree, the record's version word of
/// 20, and the encoded compact block as a byte vector. The same writer
/// returns at `7f59c5320` after the one-commit detour through the compact
/// encoding, and it survives the move to `lib/src/wallet/data.rs`.
fn push_ecb_block(out: &mut Vec<u8>) {
    let ecb = compact_block_protobuf();
    push_i32_le(out, BLOCK_HEIGHT);
    push_bytes(out, &BLOCK_HASH);
    push_empty_commitment_tree(out);
    push_u64_le(out, 20);
    push_compact_size(out, ecb.len() as u64);
    push_bytes(out, &ecb);
}

/// Append one `BlockData` in the compact encoding that `ed3b21c09`
/// introduced: the height, the hash, an empty commitment tree written only
/// so the reader can reach the version word, the record's version word of
/// 21, the predecessor's hash, the mining time, and a vector of
/// `CCompactTx`. Each `CCompactTx` carries its own version word of 21, its
/// hash, its nullifiers, and its note commitments.
fn push_compact_block(out: &mut Vec<u8>) {
    push_i32_le(out, BLOCK_HEIGHT);
    push_bytes(out, &BLOCK_HASH);
    push_empty_commitment_tree(out);
    push_u64_le(out, 21);
    push_bytes(out, &PREV_BLOCK_HASH);
    push_u32_le(out, BLOCK_TIME);
    push_compact_size(out, 1); // One compact transaction.
    push_u64_le(out, 21); // The `CCompactTx` version word.
    push_bytes(out, &TXID);
    push_compact_size(out, 1); // One nullifier.
    push_bytes(out, &NULLIFIER);
    push_compact_size(out, 1); // One note commitment.
    push_bytes(out, &CMU);
}

/// Append one sapling note record in the Blazesync encoding, replicating
/// the writer at `87ad71c28`. Its version word is 20 and its bytes stay
/// identical through `2e8b86670`, where the concrete writer has become the
/// blanket `ReadableWriteable` implementation for `NoteAndMetadata` in
/// `lib/src/wallet/traits.rs`. The account index and the separate
/// `spent_at_height` option are gone; the witness cache writes its top
/// height after the witness vector, and both spend records carry a height
/// beside the transaction identifier.
fn push_blaze_note(out: &mut Vec<u8>) {
    push_u64_le(out, 20);
    push_extended_key(out);
    push_bytes(out, &DIVERSIFIER);
    push_u64_le(out, NOTE_VALUE);
    push_u8(out, RSEED_TAG_AFTER_ZIP212);
    push_bytes(out, &RSEED);
    push_compact_size(out, 0); // No witnesses are retained.
    push_u64_le(out, BLOCK_HEIGHT as u64); // The witness cache's top height.
    push_bytes(out, &NULLIFIER);
    push_optional_none(out); // spent.
    push_optional_none(out); // unconfirmed_spent.
    push_optional_none(out); // memo.
    push_u8(out, 0); // is_change.
    push_u8(out, 1); // have_spending_key.
}

/// Append one orchard note record, replicating the same blanket
/// `NoteAndMetadata` writer at `6dd62d5e2` with orchard's associated types.
/// The surrounding frame matches the sapling note exactly; only the key and
/// the note body differ, the latter being a value, a rho, and a random
/// seed where sapling writes a value and a tagged rseed.
fn push_orchard_note(out: &mut Vec<u8>) {
    push_u64_le(out, 20);
    push_orchard_full_viewing_key(out);
    push_bytes(out, &DIVERSIFIER);
    push_u64_le(out, ORCHARD_NOTE_VALUE);
    push_bytes(out, &ORCHARD_RHO);
    push_bytes(out, &ORCHARD_RSEED);
    push_compact_size(out, 0); // No witnesses are retained.
    push_u64_le(out, BLOCK_HEIGHT as u64); // The witness cache's top height.
    push_bytes(out, &ORCHARD_NULLIFIER);
    push_optional_none(out); // spent.
    push_optional_none(out); // unconfirmed_spent.
    push_optional_none(out); // memo.
    push_u8(out, 0); // is_change.
    push_u8(out, 1); // have_spending_key.
}

/// Append one `Utxo` in the Blazesync encoding, replicating the writer at
/// `87ad71c28`, whose version word is 3 and which stays byte-identical
/// through `2e8b86670`.
fn push_blaze_utxo(out: &mut Vec<u8>) {
    push_u64_le(out, 3);
    push_u32_le(out, TADDR.len() as u32);
    push_bytes(out, TADDR.as_bytes());
    push_bytes(out, &TXID);
    push_u64_le(out, 0); // output_index.
    push_u64_le(out, UTXO_VALUE);
    push_i32_le(out, BLOCK_HEIGHT);
    push_compact_size(out, SCRIPT.len() as u64);
    push_bytes(out, &SCRIPT);
    push_optional_none(out); // spent.
    push_optional_none(out); // spent_at_height.
    push_optional_none(out); // unconfirmed_spent.
}

/// Append one transaction record in the Blazesync encoding, the type named
/// `WalletTx` until `6dd62d5e2` renames it `TransactionMetadata`. Its
/// version word is 20 at `87ad71c28`, 21 from `ead95fe0a`, which inserts
/// the `unconfirmed` flag after the block height, 22 from `a6f8a0bd6`,
/// which replaces the sapling-and-transparent value-spent pair with a
/// [transparent, sapling, orchard] triple and appends a second nullifier
/// vector, and 23 from `6dd62d5e2`, which inserts an orchard-note vector
/// after the sapling one. Note that the pair and the triple disagree on
/// order as well as on width: version 21 writes sapling before transparent,
/// while version 22 writes transparent first.
fn push_blaze_wallet_tx(out: &mut Vec<u8>, row: u8) {
    let version: u64 = match row {
        30 => 20,
        31..=36 => 21,
        37 => 22,
        _ => 23,
    };
    push_u64_le(out, version);
    push_i32_le(out, BLOCK_HEIGHT);
    if version >= 21 {
        push_u8(out, 0); // unconfirmed, added at `ead95fe0a`.
    }
    push_u64_le(out, u64::from(BLOCK_TIME)); // datetime.
    push_bytes(out, &TXID);
    push_compact_size(out, 1); // One sapling note.
    push_blaze_note(out);
    if version >= 23 {
        push_compact_size(out, 1); // One orchard note, added at `6dd62d5e2`.
        push_orchard_note(out);
    }
    push_compact_size(out, 1); // One transparent output.
    push_blaze_utxo(out);
    if version >= 22 {
        push_u64_le(out, 0); // total_transparent_value_spent.
        push_u64_le(out, 0); // total_sapling_value_spent.
        push_u64_le(out, 0); // total_orchard_value_spent.
    } else {
        push_u64_le(out, 0); // total_sapling_value_spent.
        push_u64_le(out, 0); // total_transparent_value_spent.
    }
    push_compact_size(out, 0); // No outgoing metadata.
    push_u8(out, 1); // full_tx_scanned.
    push_optional_none(out); // zec_price.
    push_compact_size(out, 0); // No spent sapling nullifiers.
    if version >= 22 {
        push_compact_size(out, 0); // No spent orchard nullifiers.
    }
}

/// Append the transaction-set record, replicating `WalletTxns::write` in
/// `lib/src/lightwallet/wallet_txns.rs` and, from `a6f8a0bd6`,
/// `TransactionMetadataSet::write` in `lib/src/wallet/transactions.rs`. At
/// `87ad71c28` it writes its version word of 20, the confirmed
/// transactions, and a second vector of mempool transactions. At
/// `ead95fe0a` the version word becomes 21 and the mempool vector
/// disappears; it stays at 21 through `2e8b86670`.
fn push_wallet_txns(out: &mut Vec<u8>, row: u8) {
    let version: u64 = if row >= 31 { 21 } else { 20 };
    push_u64_le(out, version);
    push_compact_size(out, 1); // One confirmed transaction.
    push_bytes(out, &TXID);
    push_blaze_wallet_tx(out, row);
    if row == 30 {
        push_compact_size(out, 0); // The mempool vector, dropped at `ead95fe0a`.
    }
}

/// Append the `Optional<TreeState>` that `0cd53900b` introduced: the
/// `Optional` marker, then the protobuf-encoded tree state framed as a byte
/// vector. The fixture's wallet has verified its tree at its birthday
/// height.
fn push_optional_tree_state(out: &mut Vec<u8>) {
    let state = tree_state_protobuf();
    push_optional_some(out);
    push_compact_size(out, state.len() as u64);
    push_bytes(out, &state);
}

/// Build a Blazesync-layout wallet file for one of rows 30 through 39,
/// replicating `LightWallet::write` in `lib/src/lightwallet.rs` — and, from
/// `a6f8a0bd6`, in `lib/src/wallet.rs` — at that row's Defining Commit.
fn blaze_layout(row: u8) -> Vec<u8> {
    let version: u64 = match row {
        30 => 20,
        31 => 21,
        32 => 22,
        33 => 23,
        _ => 24,
    };
    let mut out = Vec::new();
    push_u64_le(&mut out, version);
    push_bytes(&mut out, &keys_record(row));
    push_compact_size(&mut out, 1); // One block.
    if row == 34 {
        push_compact_block(&mut out);
    } else {
        push_ecb_block(&mut out);
    }
    push_wallet_txns(&mut out, row);
    push_u64_string(&mut out, CHAIN_NAME);
    if row >= 35 {
        // `7f59c5320` writes `WalletOptions` between the chain name and the
        // birthday, not at the tail. `2e8b86670` raises it to version 2.
        push_wallet_options(&mut out, if row >= 39 { 2 } else { 1 });
    }
    push_u64_le(&mut out, BIRTHDAY);
    if row <= 32 {
        push_u8(&mut out, 0); // sapling_tree_verified, dropped at `a1b9b0bbe`.
    }
    if row >= 32 {
        push_optional_tree_state(&mut out);
    }
    push_price_info(&mut out, 20);
    out
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// Row 22, Defining Commit `fb1135328` ("Viewing Keys (#32)", 2020-07-21),
/// version word 7. Replicates `LightWallet::write` in
/// `lib/src/lightwallet.rs` together with `WalletZKey::write` in
/// `lib/src/lightwallet/walletzkey.rs` and the `BlockData`, `WalletTx`,
/// `SaplingNoteData`, and `Utxo` writers in `lib/src/lightwallet/data.rs`.
/// The grammar's mark is that the separate spending-key and viewing-key
/// vectors have collapsed into one `Vector<WalletZKey>`, each element
/// carrying its own u8 version byte, so the fixture holds exactly one such
/// element.
fn row_22() -> Vec<u8> {
    flat_layout(22)
}

/// Row 23, Defining Commit `49ee4c406` ("Add spent_at_height for notes",
/// 2020-07-21), version word 8. Replicates the same writers as row 22 with
/// `SaplingNoteData::write` at version 2, which appends an
/// `Optional<i32 spent_at_height>` after the spend option. The fixture's
/// single note is unspent, so the option is `None` and the record is
/// exactly one byte longer than row 22's.
fn row_23() -> Vec<u8> {
    flat_layout(23)
}

/// Row 24, Defining Commit `8e425fc6b` ("Don't update view key witnesses",
/// 2020-08-24), version word 9. `SaplingNoteData::write` reaches version 3
/// and appends an `is_spendable` u8 after the change flag. The fixture's
/// note is spendable, so that byte is 1.
fn row_24() -> Vec<u8> {
    flat_layout(24)
}

/// Row 25, Defining Commit `28b795139` ("Update Librustzcash dependency
/// (#60)", 2020-10-15), version word 10. `SaplingNoteData::write` reaches
/// version 4 and replaces the bare thirty-two-byte randomness with the
/// tagged `Rseed` that `write_rseed` emits: a type byte, 1 for
/// `BeforeZip212` and 2 for `AfterZip212`, then the thirty-two bytes. The
/// fixture's note uses the post-ZIP-212 tag.
fn row_25() -> Vec<u8> {
    flat_layout(25)
}

/// Row 26, Defining Commit `b61175345` ("Speed up sync with multiple
/// parallel witness updates (#67)", 2020-12-01), version word 12; version
/// 11 was never minted. `Utxo::write` reaches version 2 and appends an
/// `Optional<i32 spent_at_height>` after the spend option, so the fixture's
/// single unspent transparent output is one byte longer than row 25's. The
/// note's spendability flag is renamed `have_spending_key` at this commit
/// without changing its width or its position.
fn row_26() -> Vec<u8> {
    flat_layout(26)
}

/// Row 27, Defining Commit `bcf38a6fa` ("Fast Initial Sync (#69)",
/// 2021-04-22), version word 13. `LightWallet::write` appends a
/// `sapling_tree_verified` u8 after the birthday. The fixture has not
/// verified its tree, so the byte is 0 and it is the file's last byte.
fn row_27() -> Vec<u8> {
    flat_layout(27)
}

/// Row 28, Defining Commit `7212e2bf1` ("Add commands to track progress of
/// building and sending a transaction (#70)", 2021-05-05), version word 13
/// reused. `SaplingNoteData::write` reaches version 5 and `Utxo::write`
/// reaches version 3, each appending an
/// `Optional<(txid[32], u32 height)>` unconfirmed-spend record: the note's
/// after `spent_at_height` and before the memo, the UTXO's after
/// `spent_at_height` and at the record's end. `WalletTx` stays at version 4
/// and the file carries no price record, which is what separates this
/// grammar from row 29's. The fixture's note and output are both unspent,
/// so each new option is `None` and the file is two bytes longer than row
/// 27's under an unchanged version word.
fn row_28() -> Vec<u8> {
    flat_layout(28)
}

/// Row 29, Defining Commit `4a279179f` ("Prices (#71)", 2021-05-18),
/// version word 14. `LightWallet::write` appends the `WalletZecPriceInfo`
/// record — its own version word, an `Optional<u64>` fetch timestamp, and a
/// u64 retry count — after the `sapling_tree_verified` byte, and
/// `WalletTx::write` reaches version 5, gaining an `Optional<f64>` ZEC
/// price after the scan flag. The note and UTXO bumps that the 58-row
/// census table folded into this row belong to row 28, where `7212e2bf1`
/// minted them.
fn row_29() -> Vec<u8> {
    flat_layout(29)
}

/// Row 30, Defining Commit `87ad71c28` ("Blazesync (#74)", 2021-06-25),
/// version word 20; versions 15 through 19 were never minted. The file is
/// restructured: `LightWallet::write` in `lib/src/lightwallet.rs` now emits
/// its version word, a `Keys` record from `lib/src/lightwallet/keys.rs`,
/// the block vector, a `WalletTxns` record from
/// `lib/src/lightwallet/wallet_txns.rs`, the chain name, the birthday, the
/// `sapling_tree_verified` byte, and the price record. Every sub-record's
/// own version word moves to 20 as well, `BlockData` becomes an opaque
/// encoded compact block behind a byte vector, `SaplingNoteData` drops its
/// account index and its `spent_at_height` option while gaining a witness
/// top height, and `WalletTxns` writes a second, empty vector of mempool
/// transactions.
fn row_30() -> Vec<u8> {
    blaze_layout(30)
}

/// Row 31, Defining Commit `ead95fe0a` ("Mempool monitoring (#76)",
/// 2021-07-14), version word 21. `WalletTx::write` reaches version 21 and
/// inserts an `unconfirmed` u8 directly after the block height, while
/// `WalletTxns::write` reaches version 21 and stops writing the separate
/// mempool vector. The two changes cancel in length, so this fixture is
/// exactly as long as row 30's and differs from it only in content.
fn row_31() -> Vec<u8> {
    blaze_layout(31)
}

/// Row 32, Defining Commit `0cd53900b` ("Sapling tree verification",
/// 2021-07-27), version word 22. `LightWallet::write` appends an
/// `Optional<TreeState>` after the `sapling_tree_verified` byte, encoding
/// the protobuf message into a byte vector. The fixture's wallet has
/// verified a tree at its birthday height, so the option is `Some` and the
/// blob is a real `TreeState` encoding.
fn row_32() -> Vec<u8> {
    blaze_layout(32)
}

/// Row 33, Defining Commit `a1b9b0bbe` ("Clean up initial verification",
/// 2021-07-27), version word 23. `LightWallet::write` drops the
/// `sapling_tree_verified` byte that `bcf38a6fa` added, leaving the
/// `Optional<TreeState>` to stand alone between the birthday and the price
/// record.
///
/// This grammar was minted twice. On 2021-08-05 the commit `c2c99265f`
/// ("Cleanup") reverted row 34's compact block encoding byte for byte and
/// restored version word 23, so a file written between 2021-08-05 and
/// `7f59c5320` on 2021-09-24 is indistinguishable from a file written in
/// this row's first window. The census therefore folds that second minting
/// into this row rather than giving it a row of its own, and this single
/// fixture stands for both windows.
fn row_33() -> Vec<u8> {
    blaze_layout(33)
}

/// Row 34, Defining Commit `ed3b21c09` ("use CCompactTx", 2021-07-29),
/// version word 24. `BlockData::write` in `lib/src/lightwallet/data.rs`
/// reaches version 21 and re-encodes each block structurally: after the
/// height, the hash, and an empty commitment tree written only so the
/// reader can reach the version word, it emits the predecessor's hash, the
/// mining time, and a `Vector<CCompactTx>`. The fixture's block carries one
/// `CCompactTx` holding one nullifier and one note commitment, the same
/// content that row 35 hides inside its opaque blob.
fn row_34() -> Vec<u8> {
    blaze_layout(34)
}

/// Row 35, Defining Commit `7f59c5320` ("Optionally download memos",
/// 2021-09-24), version word 24 reused. `BlockData::write` has returned to
/// the opaque encoded-compact-block encoding with its version word of 20,
/// and `LightWallet::write` writes a `WalletOptions` record — its own
/// version word and a `download_memos` u8 — between the chain name and the
/// birthday, not at the tail as the census summary suggests. The fixture's
/// wallet downloads memos for its own transactions, which is the default.
/// Rows 34 and 35 share the file's version word and are told apart only by
/// the block record's encoding.
fn row_35() -> Vec<u8> {
    blaze_layout(35)
}

/// Row 36, Defining Commit `5e73adef4` ("Taddress priv key import (#83)",
/// 2021-10-13), version word 24 reused. `Keys::write` in
/// `lib/src/lightwallet/keys.rs` reaches version 21: the raw transparent
/// secret keys and the transparent address strings, which were two
/// independent trailing vectors, collapse into one `Vector<WalletTKey>`
/// whose element writer lives in the new
/// `lib/src/lightwallet/wallettkey.rs`. Each `WalletTKey` carries its own
/// u8 version byte, a key-type u32, a locked flag, an optional secret key,
/// its address as a `write_string`, an optional HD index, and the encrypted
/// key and nonce options. Nothing outside the `Keys` record changes.
fn row_36() -> Vec<u8> {
    blaze_layout(36)
}

/// Row 37, Defining Commit `a6f8a0bd6` (merge of "keep_orcharding" via
/// `74bace493` and `9b1faacac`, 2022-07-23), version word 24 reused. The
/// writer's home has moved from `lib/src/lightwallet.rs` to
/// `lib/src/wallet.rs`, and `WalletTx::write` in `lib/src/wallet/data.rs`
/// reaches version 22. The sapling-and-transparent value-spent pair becomes
/// the `[transparent, sapling, orchard]` triple that `value_spent_by_pool`
/// returns, which both widens the field by eight bytes and reverses the
/// order of the two values that were already there, and a
/// `Vector<orchard nullifier>` follows the sapling nullifier vector. The
/// `Keys` record stays at version 21, so the wallet still holds no orchard
/// key.
fn row_37() -> Vec<u8> {
    blaze_layout(37)
}

/// Row 38, Defining Commit `6dd62d5e2` (merge of "orchardize_more" via
/// `085cd0661` and `17f3f5b5b`, 2022-08-23), version word 24 reused. Two
/// marks land together. `Keys::write` in `lib/src/wallet/keys.rs` reaches
/// version 22 and inserts a `Vector<WalletOKey>` between the sapling and
/// transparent vectors, its element writer being `OrchardKey::write` in
/// `lib/src/wallet/keys/orchard.rs`. The transaction record, renamed
/// `TransactionMetadata`, reaches version 23 and inserts a
/// `Vector<orchard note>` between the sapling-note and UTXO vectors; both
/// note vectors are now written by the blanket `ReadableWriteable`
/// implementation for `NoteAndMetadata` in `lib/src/wallet/traits.rs`,
/// which frames orchard and sapling notes identically and differs only in
/// the key and note bodies. The fixture holds one orchard key and one
/// orchard note so that both marks appear in the bytes.
fn row_38() -> Vec<u8> {
    blaze_layout(38)
}

/// Row 39, Defining Commit `2e8b86670` (merge of
/// "transaction_filter_persists" via `649713ffb`, 2022-09-16), version word
/// 24 reused. `WalletOptions::write` in `lib/src/wallet.rs` reaches version
/// 2 and appends an `Optional<u32>` transaction-size filter after the
/// memo-download byte. Nothing else in the file changes: the accompanying
/// commit only reroutes the writer's field access through a
/// `TransactionContext`. The fixture keeps the record's own default of 500.
fn row_39() -> Vec<u8> {
    blaze_layout(39)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The version word a fixture opens with.
    fn version_word(bytes: &[u8]) -> u64 {
        u64::from_le_bytes(bytes[0..8].try_into().unwrap())
    }

    #[test]
    fn every_row_opens_with_its_version_word() {
        assert_eq!(version_word(&row_22()), 7);
        assert_eq!(version_word(&row_23()), 8);
        assert_eq!(version_word(&row_24()), 9);
        assert_eq!(version_word(&row_25()), 10);
        assert_eq!(version_word(&row_26()), 12);
        assert_eq!(version_word(&row_27()), 13);
        assert_eq!(version_word(&row_28()), 13);
        assert_eq!(version_word(&row_29()), 14);
        assert_eq!(version_word(&row_30()), 20);
        assert_eq!(version_word(&row_31()), 21);
        assert_eq!(version_word(&row_32()), 22);
        assert_eq!(version_word(&row_33()), 23);
        assert_eq!(version_word(&row_34()), 24);
        assert_eq!(version_word(&row_35()), 24);
        assert_eq!(version_word(&row_36()), 24);
        assert_eq!(version_word(&row_37()), 24);
        assert_eq!(version_word(&row_38()), 24);
        assert_eq!(version_word(&row_39()), 24);
    }

    /// Row 22's mark: one key vector, whose single element opens with the
    /// `WalletZKey` version byte. The count sits at offset 90, after the
    /// version word, the encrypted flag, the encrypted seed, the empty
    /// nonce vector, and the seed.
    #[test]
    fn row_22_writes_one_wallet_zkey_vector() {
        let bytes = row_22();
        let offset = 8 + 1 + ENC_SEED_LEN + 1 + SEED_LEN;
        assert_eq!(offset, 90);
        assert_eq!(bytes[offset], 1, "the key vector holds one element");
        assert_eq!(bytes[offset + 1], 1, "the element's own version byte");
    }

    /// Rows 23, 24, and 25 each widen the note record by exactly one byte:
    /// the `spent_at_height` `None` marker, the spendability flag, and the
    /// `Rseed` type tag in turn.
    #[test]
    fn rows_23_through_25_each_add_one_note_byte() {
        assert_eq!(row_23().len(), row_22().len() + 1);
        assert_eq!(row_24().len(), row_23().len() + 1);
        assert_eq!(row_25().len(), row_24().len() + 1);
    }

    /// Row 25's mark: the note's randomness now carries a type tag
    /// immediately before its thirty-two bytes.
    #[test]
    fn row_25_tags_the_note_randomness() {
        let mut untagged = Vec::new();
        push_flat_note(&mut untagged, 24);
        let mut tagged = Vec::new();
        push_flat_note(&mut tagged, 25);

        let head = untagged
            .windows(RSEED.len())
            .position(|w| w == RSEED)
            .expect("the untagged note carries the randomness bare");
        assert_eq!(tagged[head], RSEED_TAG_AFTER_ZIP212);
        assert_eq!(tagged[head + 1..head + 1 + RSEED.len()], RSEED);
        assert_eq!(tagged.len(), untagged.len() + 1);
    }

    /// Row 26's mark: the UTXO record gains one `Optional` marker.
    #[test]
    fn row_26_widens_the_utxo_record_by_one_byte() {
        assert_eq!(row_26().len(), row_25().len() + 1);
        let mut before = Vec::new();
        push_flat_utxo(&mut before, 25);
        let mut after = Vec::new();
        push_flat_utxo(&mut after, 26);
        assert_eq!(after.len(), before.len() + 1);
        // Everything after the record's own version word is unchanged; only
        // the trailing option is new.
        assert_eq!(after[8..before.len()], before[8..]);
    }

    /// Row 27's mark: a single `sapling_tree_verified` byte at the tail.
    #[test]
    fn row_27_appends_the_sapling_tree_verified_byte() {
        let bytes = row_27();
        assert_eq!(bytes.len(), row_26().len() + 1);
        assert_eq!(*bytes.last().unwrap(), 0);
    }

    /// Row 28's mark: the note and the UTXO each gain one unconfirmed-spend
    /// option under a version word that has not moved, so the file grows by
    /// two bytes while offset 0 still reads 13.
    #[test]
    fn row_28_adds_two_unconfirmed_spend_options_without_a_version_bump() {
        let previous = row_27();
        let bytes = row_28();
        assert_eq!(version_word(&bytes), version_word(&previous));
        assert_eq!(bytes.len(), previous.len() + 2);

        let mut note_before = Vec::new();
        push_flat_note(&mut note_before, 27);
        let mut note_after = Vec::new();
        push_flat_note(&mut note_after, 28);
        assert_eq!(note_after.len(), note_before.len() + 1);
        assert_eq!(u64::from_le_bytes(note_after[0..8].try_into().unwrap()), 5);

        let mut utxo_before = Vec::new();
        push_flat_utxo(&mut utxo_before, 27);
        let mut utxo_after = Vec::new();
        push_flat_utxo(&mut utxo_after, 28);
        assert_eq!(utxo_after.len(), utxo_before.len() + 1);
        assert_eq!(u64::from_le_bytes(utxo_after[0..8].try_into().unwrap()), 3);
    }

    /// Row 29's mark: the `WalletZecPriceInfo` record closes the file.
    #[test]
    fn row_29_appends_the_price_record() {
        let bytes = row_29();
        let mut price = Vec::new();
        push_price_info(&mut price, 1);
        assert_eq!(price.len(), 17);
        assert_eq!(&bytes[bytes.len() - price.len()..], &price[..]);
    }

    /// Row 30's mark: a `Keys` record, carrying its own version word of 20,
    /// begins immediately after the file's version word.
    #[test]
    fn row_30_delegates_the_key_material_to_a_keys_record() {
        let bytes = row_30();
        let keys = keys_record(30);
        assert_eq!(u64::from_le_bytes(keys[0..8].try_into().unwrap()), 20);
        assert_eq!(&bytes[8..8 + keys.len()], &keys[..]);
    }

    /// Row 31's mark: the transaction record gains an `unconfirmed` byte
    /// while the mempool vector disappears, so the file's length is
    /// unchanged and only its content moves.
    #[test]
    fn row_31_trades_the_mempool_vector_for_an_unconfirmed_byte() {
        let previous = row_30();
        let bytes = row_31();
        assert_eq!(bytes.len(), previous.len());
        assert_ne!(bytes, previous);
    }

    /// Row 32's mark: an `Optional<TreeState>` blob sits between the
    /// `sapling_tree_verified` byte and the price record.
    #[test]
    fn row_32_appends_the_optional_tree_state() {
        let bytes = row_32();
        let mut tree_state = Vec::new();
        push_optional_tree_state(&mut tree_state);
        assert_eq!(bytes.len(), row_31().len() + tree_state.len());

        let mut price = Vec::new();
        push_price_info(&mut price, 20);
        let tail = bytes.len() - price.len();
        assert_eq!(&bytes[tail - tree_state.len()..tail], &tree_state[..]);
    }

    /// Row 33's mark: the `sapling_tree_verified` byte is gone, so the file
    /// is exactly one byte shorter than row 32's.
    #[test]
    fn row_33_drops_the_sapling_tree_verified_byte() {
        assert_eq!(row_33().len(), row_32().len() - 1);
    }

    /// Rows 34 and 35 share version word 24 and are told apart only by the
    /// block record's own version word: 21 for the compact encoding and 20
    /// for the opaque encoded-compact-block encoding. That word sits after
    /// the file version, the `Keys` record, the block vector's count, the
    /// height, the hash, and the empty commitment tree.
    #[test]
    fn rows_34_and_35_differ_in_the_block_encoding_alone_at_version_24() {
        let compact = row_34();
        let opaque = row_35();
        assert_eq!(version_word(&compact), version_word(&opaque));
        assert_ne!(compact, opaque);

        let offset = 8 + keys_record(34).len() + 1 + 4 + 32 + 3;
        assert_eq!(
            u64::from_le_bytes(compact[offset..offset + 8].try_into().unwrap()),
            21
        );
        assert_eq!(
            u64::from_le_bytes(opaque[offset..offset + 8].try_into().unwrap()),
            20
        );
    }

    /// Row 35's mark: a `WalletOptions` record, its version word followed
    /// by the memo-download option, sits between the chain name and the
    /// birthday.
    #[test]
    fn row_35_writes_wallet_options_before_the_birthday() {
        let bytes = row_35();
        let mut needle = Vec::new();
        push_u64_string(&mut needle, CHAIN_NAME);
        push_wallet_options(&mut needle, 1);
        push_u64_le(&mut needle, BIRTHDAY);
        assert_eq!(needle.len(), 12 + 9 + 8);
        assert!(bytes.windows(needle.len()).any(|w| w == needle));
    }

    /// Row 36's mark: the `Keys` record reaches version 21, and its two
    /// trailing transparent vectors become one `Vector<WalletTKey>` whose
    /// element opens with its own version byte and carries the address the
    /// old vector held separately.
    #[test]
    fn row_36_collapses_the_transparent_vectors_into_wallet_tkeys() {
        let previous = keys_record(35);
        let keys = keys_record(36);
        assert_eq!(u64::from_le_bytes(previous[0..8].try_into().unwrap()), 20);
        assert_eq!(u64::from_le_bytes(keys[0..8].try_into().unwrap()), 21);

        // Both records agree from the encrypted flag through the sapling
        // key vector; they diverge only in what follows it.
        let mut zkey = Vec::new();
        push_wallet_zkey(&mut zkey);
        let head = 8 + 1 + ENC_SEED_LEN + 1 + SEED_LEN + 1 + zkey.len();
        assert_eq!(keys[8..head], previous[8..head]);

        // Version 20 ended with a raw-key vector and an address vector.
        assert_eq!(
            previous.len() - head,
            (1 + SEED_LEN) + (1 + 8 + TADDR.len())
        );

        // Version 21 ends with one `Vector<WalletTKey>` instead.
        let mut tail = Vec::new();
        push_compact_size(&mut tail, 1);
        push_wallet_tkey(&mut tail);
        assert_eq!(&keys[head..], &tail[..]);
        assert_ne!(row_36(), row_35());
    }

    /// Row 37's mark: the transaction record reaches version 22, trading a
    /// two-value spend field for a three-value one and appending a second
    /// nullifier vector, which together add nine bytes.
    #[test]
    fn row_37_writes_the_pool_triple_and_a_second_nullifier_vector() {
        let mut previous = Vec::new();
        push_blaze_wallet_tx(&mut previous, 36);
        let mut bytes = Vec::new();
        push_blaze_wallet_tx(&mut bytes, 37);
        assert_eq!(u64::from_le_bytes(previous[0..8].try_into().unwrap()), 21);
        assert_eq!(u64::from_le_bytes(bytes[0..8].try_into().unwrap()), 22);
        assert_eq!(bytes.len(), previous.len() + 8 + 1);
        assert_eq!(row_37().len(), row_36().len() + 9);
    }

    /// Row 38's two marks: the `Keys` record reaches version 22 and carries
    /// a `Vector<WalletOKey>` between its sapling and transparent vectors,
    /// and the transaction record reaches version 23 and carries a
    /// `Vector<orchard note>` after its sapling-note vector.
    #[test]
    fn row_38_adds_the_orchard_key_and_orchard_note_vectors() {
        let keys = keys_record(38);
        assert_eq!(u64::from_le_bytes(keys[0..8].try_into().unwrap()), 22);

        let mut okey = Vec::new();
        push_wallet_okey(&mut okey);
        let okey_at = keys
            .windows(okey.len())
            .position(|w| w == okey)
            .expect("the orchard key vector holds one element");
        let mut tkey = Vec::new();
        push_wallet_tkey(&mut tkey);
        let tkey_at = keys
            .windows(tkey.len())
            .position(|w| w == tkey)
            .expect("the transparent key vector holds one element");
        assert!(okey_at < tkey_at, "orchard keys precede transparent keys");

        let mut transaction = Vec::new();
        push_blaze_wallet_tx(&mut transaction, 38);
        assert_eq!(
            u64::from_le_bytes(transaction[0..8].try_into().unwrap()),
            23
        );
        let mut orchard_note = Vec::new();
        push_orchard_note(&mut orchard_note);
        let mut sapling_note = Vec::new();
        push_blaze_note(&mut sapling_note);
        let sapling_at = transaction
            .windows(sapling_note.len())
            .position(|w| w == sapling_note)
            .expect("the sapling note is present");
        let orchard_at = transaction
            .windows(orchard_note.len())
            .position(|w| w == orchard_note)
            .expect("the orchard note is present");
        assert!(sapling_at < orchard_at, "sapling notes come first");
    }

    /// Row 39's mark: the `WalletOptions` record reaches version 2 and
    /// appends an `Optional<u32>` transaction-size filter, five bytes in
    /// its `Some` form, and nothing else in the file moves.
    #[test]
    fn row_39_appends_the_transaction_size_filter() {
        let mut previous = Vec::new();
        push_wallet_options(&mut previous, 1);
        let mut options = Vec::new();
        push_wallet_options(&mut options, 2);
        assert_eq!(options.len(), previous.len() + 5);
        // Only the version word moves; the memo-download byte keeps its
        // value and its position, and the filter follows it.
        assert_eq!(options[8], previous[8]);
        assert_eq!(options[9], 1, "the Optional Some marker");
        assert_eq!(
            u32::from_le_bytes(options[10..14].try_into().unwrap()),
            TRANSACTION_SIZE_FILTER
        );

        assert_eq!(row_39().len(), row_38().len() + 5);
        let bytes = row_39();
        let mut needle = Vec::new();
        push_u64_string(&mut needle, CHAIN_NAME);
        push_wallet_options(&mut needle, 2);
        push_u64_le(&mut needle, BIRTHDAY);
        assert!(bytes.windows(needle.len()).any(|w| w == needle));
    }

    /// Adjacent census rows are distinguishable, which is the claim the
    /// census makes for every neighboring pair.
    #[test]
    fn adjacent_rows_are_byte_distinct() {
        let all = fixtures();
        for pair in all.windows(2) {
            assert_ne!(
                pair[0].bytes, pair[1].bytes,
                "rows {} and {} produced identical bytes",
                pair[0].row, pair[1].row
            );
        }
    }

    /// The fixtures this module emits carry the row numbers the 77-row
    /// census table assigns, in order and without gaps.
    #[test]
    fn the_era_covers_rows_22_through_39() {
        let rows: Vec<u8> = fixtures().iter().map(|f| f.row).collect();
        assert_eq!(rows, (22..=39).collect::<Vec<u8>>());
    }
}
