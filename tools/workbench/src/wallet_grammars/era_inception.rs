//! Census rows 1 through 21: the writer's inception, on zecwallet-light-cli's
//! `dev` line between 2019-09-06 and 2020-05-09.
//!
//! Every fixture below is derived from the writer source at its row's Defining
//! Commit, read with `git show <commit>:<path>`. The serializer moved four
//! times inside this era, so the replicated path differs by row:
//! `rust-lightclient/src/lightwallet.rs` for rows 1 through 7,
//! `src/lightwallet.rs` for rows 8 and 9, `src/lightwallet/mod.rs` with
//! `src/lightwallet/data.rs` for rows 10 through 13, and
//! `lib/src/lightwallet.rs` with `lib/src/lightwallet/data.rs` for rows 14
//! through 21.
//!
//! The era's wallet is a single-account, unencrypted mainnet wallet holding
//! exactly one transaction. Its seed and key material are all-zero, its block
//! vector is empty except at row 1, its chain name is `main`, and its birthday
//! is 1000000. The one transaction is what makes the era legible: eight of its
//! twenty-one grammars change nothing but the `WalletTx`, `SaplingNoteData`,
//! or `Utxo` sub-records, and a wallet with no transactions would render those
//! eight rows byte-identical to their neighbors. Every fixture therefore
//! carries one transaction with one note, plus one UTXO and one
//! outgoing-metadata record once the grammar admits them.
//!
//! Two findings from this walk contradict the issue table, and both are
//! documented on the rows they affect: row 1's writer does emit an 8-byte
//! version word, which makes rows 1 and 2 byte-identical grammars, and row 20
//! compresses with gzip, not zstd.

use super::util::{
    push_bytes, push_compact_size, push_compact_vec_u8, push_i32_le, push_optional_none,
    push_u32_le, push_u64_le, push_u64_string, push_u8,
};
use super::Fixture;

/// The wallet's seed, written raw as `[u8; 32]` from row 4 onward.
const SEED: [u8; 32] = [0u8; 32];

/// One `zip32::ExtendedSpendingKey`, all-zero.
///
// ASSUMPTION: librustzcash is a path or git dependency at every commit in this
// era (`../../librustzcash/zcash_primitives` at row 4, then
// `github.com/adityapk00/librustzcash` rev `98f9bda32` by row 20), so its
// source is not available in this checkout. The 169-byte layout is taken from
// the crate's documented and stable `ExtendedSpendingKey::write`: `depth` u8,
// `parent_fvk_tag` 4 bytes, `child_index` u32 LE, `chain_code` 32 bytes,
// `ExpandedSpendingKey` (`ask` 32, `nsk` 32, `ovk` 32), and `dk` 32 bytes.
// Zeroing every field makes the record 169 zero bytes. A real `ask` and `nsk`
// would be Jubjub scalars, but the writer never validates on the way out and
// nothing in the census reads them back through the curve.
const EXTENDED_SPENDING_KEY: [u8; 169] = [0u8; 169];

/// One `zip32::ExtendedFullViewingKey`, all-zero. Every note record carries
/// one of these, and row 17 adds a wallet-level vector of them.
///
// ASSUMPTION: the same dependency situation as `EXTENDED_SPENDING_KEY`. The
// 169-byte layout is `depth` u8, `parent_fvk_tag` 4 bytes, `child_index` u32
// LE, `chain_code` 32 bytes, `FullViewingKey` (`ak` 32, `nk` 32, `ovk` 32),
// and `dk` 32 bytes. The two key records coincide in width, which is why the
// census separates them by position rather than by size.
const EXTENDED_FULL_VIEWING_KEY: [u8; 169] = [0u8; 169];

/// One transparent secret key, written as 32 raw bytes from row 7 onward.
///
// ASSUMPTION: `secp256k1::SecretKey` derefs to its 32 big-endian scalar bytes,
// which is how the writer reaches them, through `&self.tkeys[0][..]`.
const TRANSPARENT_KEY: [u8; 32] = [0u8; 32];

/// The wallet's transparent address, written from row 18 onward at the wallet
/// level and from row 6 onward inside every `Utxo`.
///
/// This is the genuine Zcash mainnet P2PKH address for a twenty-byte all-zero
/// HASH160: Base58Check over the `0x1CB8` prefix and that hash.
const TRANSPARENT_ADDRESS: &str = "t1Hsc1LR8yKnbbe3twRp88p6vFfC5t7DLbs";

/// The recipient recorded in the outgoing-metadata record that row 10
/// introduces. `scan_full_tx` recovers outgoing sends through the outgoing
/// viewing key, so the recipient is always a Sapling address.
///
// ASSUMPTION: this is the well-formed mainnet Sapling address (Bech32, HRP
// `zs`) over a 43-byte all-zero payload; its checksum verifies. The payload is
// not a valid diversifier and `pk_d` pair, but the writer stores the address
// as an opaque string and never decodes it.
const SAPLING_ADDRESS: &str =
    "zs1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqpq6d8g";

/// The chain name, written from row 11 onward through `utils::write_string`.
const CHAIN_NAME: &str = "main";

/// The wallet birthday, written from row 13 onward as a u64.
const BIRTHDAY: u64 = 1_000_000;

/// The height carried by the single `BlockData` of row 1, by the wallet's one
/// transaction, and by the single `Utxo`.
const HEIGHT: i32 = 1_000_000;

/// The transaction timestamp that row 16 inserts into `WalletTx`. This is
/// 2019-10-02T05:46:40Z, inside the window in which the field was added.
const DATETIME: u64 = 1_570_000_000;

/// The value of the single `Utxo`, in zatoshi.
const UTXO_VALUE: u64 = 100_000;

/// The value of the single `SaplingNoteData`, in zatoshi.
const NOTE_VALUE: u64 = 50_000;

/// The value recorded in the outgoing-metadata record, in zatoshi.
const OUTGOING_VALUE: u64 = 25_000;

/// The `enc_seed` field, `[u8; 48]`, written raw from row 19 onward. An
/// unencrypted wallet leaves it zeroed.
const ENC_SEED: [u8; 48] = [0u8; 48];

/// This era's fixtures, census rows 1 through 21 of the 77-row table. The row
/// numbers here are informational: `wallet_grammars::all` reassigns them from
/// the central manifest by Defining Commit hash.
pub fn fixtures() -> Vec<Fixture> {
    let rows: [(u8, &'static str, Vec<u8>); 21] = [
        (1, "7ebc8686e", row_01()),
        (2, "c2e26fbbc", row_02()),
        (3, "8ff6d15e3", row_03()),
        (4, "5bd8b754d", row_04()),
        (5, "db549f5b6", row_05()),
        (6, "f532b70ca", row_06()),
        (7, "b24f174b5", row_07()),
        (8, "0e8ab4d27", row_08()),
        (9, "f93267507", row_09()),
        (10, "b0f7d8fcf", row_10()),
        (11, "b3ca226ff", row_11()),
        (12, "df12ccf31", row_12()),
        (13, "88a80f574", row_13()),
        (14, "ba706ab7c", row_14()),
        (15, "e3f972508", row_15()),
        (16, "ebf3c7133", row_16()),
        (17, "e3a0fd2de", row_17()),
        (18, "fc15de568", row_18()),
        (19, "72548e077", row_19()),
        (20, "796663c97", row_20()),
        (21, "cbffd69c6", row_21()),
    ];
    rows.into_iter()
        .map(|(row, defining_commit, bytes)| Fixture {
            row,
            defining_commit,
            branch: "dev",
            bytes,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The transaction sub-record, which eight of this era's rows are about
// ---------------------------------------------------------------------------

/// The `WalletTx` and `SaplingNoteData` grammar in force at a given row. Each
/// field names the Defining Commit that introduced it, so a row's shape states
/// which sub-record commits that row stands after.
#[derive(Clone, Copy)]
struct TransactionShape {
    /// What `WalletTx::serialized_version()` returns.
    version: u64,
    /// The 32-byte txid after `block`, the trailing
    /// `total_shielded_value_spent`, and the note record's `is_change` byte.
    /// All three arrive together at row 3 (`8ff6d15e3`).
    txid_and_shielded_total: bool,
    /// `total_transparent_value_spent`, introduced at row 5 (`db549f5b6`).
    transparent_total: bool,
    /// The in-transaction `Vector<Utxo>`, introduced at row 8 (`0e8ab4d27`).
    /// `Some(true)` while `Utxo` still wrote its trailing unconfirmed-spent
    /// `Optional`, `Some(false)` from row 9 (`f93267507`) onward.
    utxos: Option<bool>,
    /// The `Vector<OutgoingTxMetadata>`, introduced at row 10 (`b0f7d8fcf`)
    /// alongside the sub-version bump from 1 to 2.
    outgoing_metadata: bool,
    /// The trailing `full_tx_scanned` byte, introduced at row 12
    /// (`df12ccf31`) without a sub-version bump.
    full_tx_scanned: bool,
    /// The u64 `datetime` inserted after `block`, introduced at row 16
    /// (`ebf3c7133`) alongside the sub-version bump from 3 to 4.
    datetime: bool,
}

/// Rows 1 and 2: the born transaction record, a version word, the block
/// height, and the note vector.
const TX_BORN: TransactionShape = TransactionShape {
    version: 1,
    txid_and_shielded_total: false,
    transparent_total: false,
    utxos: None,
    outgoing_metadata: false,
    full_tx_scanned: false,
    datetime: false,
};

/// Rows 3 and 4, after `8ff6d15e3`.
const TX_TXID: TransactionShape = TransactionShape {
    txid_and_shielded_total: true,
    ..TX_BORN
};

/// Rows 5 through 7, after `db549f5b6`.
const TX_TRANSPARENT_TOTAL: TransactionShape = TransactionShape {
    transparent_total: true,
    ..TX_TXID
};

/// Row 8, after `0e8ab4d27`, whose `Utxo` still writes two `Optional` spend
/// markers.
const TX_INLINE_UTXO: TransactionShape = TransactionShape {
    utxos: Some(true),
    ..TX_TRANSPARENT_TOTAL
};

/// Row 9, after `f93267507` trimmed the `Utxo`'s unconfirmed-spent `Optional`.
const TX_TRIMMED_UTXO: TransactionShape = TransactionShape {
    utxos: Some(false),
    ..TX_INLINE_UTXO
};

/// Rows 10 and 11, after `b0f7d8fcf`.
const TX_OUTGOING: TransactionShape = TransactionShape {
    version: 2,
    outgoing_metadata: true,
    ..TX_TRIMMED_UTXO
};

/// Rows 12 through 14, after `df12ccf31`.
const TX_SCANNED: TransactionShape = TransactionShape {
    full_tx_scanned: true,
    ..TX_OUTGOING
};

/// Row 15, after `e3f972508` bumped the sub-version from 2 to 3 without
/// changing a byte of the record's shape.
const TX_SCANNED_V3: TransactionShape = TransactionShape {
    version: 3,
    ..TX_SCANNED
};

/// Rows 16 through 21, after `ebf3c7133`.
const TX_DATETIME: TransactionShape = TransactionShape {
    version: 4,
    datetime: true,
    ..TX_SCANNED_V3
};

/// Append the wallet's transaction map: a `Vector` of one tuple, the txid as
/// 32 raw bytes followed by the `WalletTx` record. The map's own txid is
/// written even after row 3 gave `WalletTx` a second copy of it.
fn push_transaction_map(out: &mut Vec<u8>, shape: TransactionShape) {
    push_compact_size(out, 1);
    push_bytes(out, &[0u8; 32]);
    push_wallet_tx(out, shape);
}

/// Append one `WalletTx` as `WalletTx::write` emits it under `shape`.
fn push_wallet_tx(out: &mut Vec<u8>, shape: TransactionShape) {
    push_u64_le(out, shape.version);
    push_i32_le(out, HEIGHT);
    if shape.datetime {
        push_u64_le(out, DATETIME);
    }
    if shape.txid_and_shielded_total {
        push_bytes(out, &[0u8; 32]);
    }
    push_compact_size(out, 1);
    push_sapling_note_data(out, shape.txid_and_shielded_total);
    if let Some(unconfirmed_spent) = shape.utxos {
        push_compact_size(out, 1);
        push_utxo(out, unconfirmed_spent);
    }
    if shape.txid_and_shielded_total {
        push_u64_le(out, 0);
    }
    if shape.transparent_total {
        push_u64_le(out, 0);
    }
    if shape.outgoing_metadata {
        push_compact_size(out, 1);
        push_outgoing_tx_metadata(out);
    }
    if shape.full_tx_scanned {
        push_u8(out, 0);
    }
}

/// Append one `SaplingNoteData` as `SaplingNoteData::write` emits it: the
/// sub-record version 1 as a u64, the account as a u64, the note's own
/// `ExtendedFullViewingKey`, the 11-byte diversifier, the note value as a u64,
/// the note randomness as 32 raw bytes, the witness vector, the 32-byte
/// nullifier, and two `Optional`s for the spending txid and the memo. The
/// `is_change` byte closes the record from row 3 onward.
///
/// The note is unspent with no memo and no witnesses, so all three of those
/// fields collapse to a single byte each. The sub-record version stays at 1
/// across this whole era.
fn push_sapling_note_data(out: &mut Vec<u8>, is_change: bool) {
    push_u64_le(out, 1);
    push_u64_le(out, 0);
    push_bytes(out, &EXTENDED_FULL_VIEWING_KEY);
    push_bytes(out, &[0u8; 11]);
    push_u64_le(out, NOTE_VALUE);
    push_bytes(out, &[0u8; 32]);
    push_compact_size(out, 0);
    push_bytes(out, &[0u8; 32]);
    push_optional_none(out);
    push_optional_none(out);
    if is_change {
        push_u8(out, 0);
    }
}

/// Append one `Utxo` as `Utxo::write` emits it: the sub-record version 1 as a
/// u64, the address as a u32 little-endian byte length followed by its ASCII,
/// the txid as 32 raw bytes, the output index and the value as u64s, the
/// height as an i32, the script as a `Vector<u8>`, and the spend markers. The
/// address length is the era's third length discipline: neither a CompactSize
/// nor `write_string`'s u64, but a bare u32 that only `Utxo` uses.
///
/// `unconfirmed_spent` selects the trailing `Optional<TxId>` that `f93267507`
/// removed at row 9.
fn push_utxo(out: &mut Vec<u8>, unconfirmed_spent: bool) {
    push_u64_le(out, 1);
    push_u32_le(out, TRANSPARENT_ADDRESS.len() as u32);
    push_bytes(out, TRANSPARENT_ADDRESS.as_bytes());
    push_bytes(out, &[0u8; 32]);
    push_u64_le(out, 0);
    push_u64_le(out, UTXO_VALUE);
    push_i32_le(out, HEIGHT);
    push_compact_vec_u8(out, &p2pkh_script());
    push_optional_none(out);
    if unconfirmed_spent {
        push_optional_none(out);
    }
}

/// Append one `OutgoingTxMetadata` as `OutgoingTxMetadata::write` emits it:
/// the address in the `write_string` discipline of a u64 length and its UTF-8,
/// the value as a u64, and the memo as 512 raw bytes with neither a length nor
/// an `Optional` to frame it.
fn push_outgoing_tx_metadata(out: &mut Vec<u8>) {
    push_u64_string(out, SAPLING_ADDRESS);
    push_u64_le(out, OUTGOING_VALUE);
    push_bytes(out, &[0u8; 512]);
}

// ---------------------------------------------------------------------------
// Other sub-record writers
// ---------------------------------------------------------------------------

/// Append one `BlockData` as `BlockData::write` emits it: the height as an
/// i32, the block hash as 32 raw bytes, the Sapling `CommitmentTree`, and the
/// literal end tag 11 as a u64. The shape holds unchanged across this era.
fn push_block_data(out: &mut Vec<u8>, height: i32) {
    push_i32_le(out, height);
    push_bytes(out, &[0u8; 32]);
    push_empty_commitment_tree(out);
    push_u64_le(out, 11);
}

/// Append an empty `CommitmentTree<Node>`.
///
// ASSUMPTION: librustzcash's source is not in this checkout, so the encoding
// comes from the crate's documented `CommitmentTree::write`: `Optional<Node>
// left`, `Optional<Node> right`, then `Vector<Optional<Node>> parents`. A tree
// that has absorbed no notes writes three zero bytes, a None, a None, and an
// empty vector.
fn push_empty_commitment_tree(out: &mut Vec<u8>) {
    push_optional_none(out);
    push_optional_none(out);
    push_compact_size(out, 0);
}

/// The standard 25-byte P2PKH `scriptPubKey` for the all-zero HASH160 that
/// [`TRANSPARENT_ADDRESS`] encodes: `OP_DUP`, `OP_HASH160`, twenty pushed
/// bytes, `OP_EQUALVERIFY`, `OP_CHECKSIG`.
fn p2pkh_script() -> Vec<u8> {
    let mut script = vec![0x76, 0xA9, 0x14];
    script.extend_from_slice(&[0u8; 20]);
    script.extend_from_slice(&[0x88, 0xAC]);
    script
}

/// Append the `Vector<ExtendedSpendingKey>` holding this era's single key.
fn push_spending_keys(out: &mut Vec<u8>) {
    push_compact_size(out, 1);
    push_bytes(out, &EXTENDED_SPENDING_KEY);
}

/// Append the `Vector<ExtendedFullViewingKey>` that row 17 introduces.
fn push_full_viewing_keys(out: &mut Vec<u8>) {
    push_compact_size(out, 1);
    push_bytes(out, &EXTENDED_FULL_VIEWING_KEY);
}

/// Append the `Vector<secp256k1::SecretKey>` that row 14 introduces, each key
/// written as its 32 raw bytes.
fn push_transparent_key_vector(out: &mut Vec<u8>) {
    push_compact_size(out, 1);
    push_bytes(out, &TRANSPARENT_KEY);
}

/// Append the `Vector<String>` of transparent addresses that row 18
/// introduces. The vector uses `Vector`'s CompactSize count while each element
/// uses `write_string`'s u64 length, so the two disciplines meet inside one
/// field.
fn push_transparent_address_vector(out: &mut Vec<u8>) {
    push_compact_size(out, 1);
    push_u64_string(out, TRANSPARENT_ADDRESS);
}

/// Append the standalone `Vector<Utxo>` that rows 6 and 7 carry at the wallet
/// level, holding one UTXO.
fn push_standalone_utxo_vector(out: &mut Vec<u8>) {
    push_compact_size(out, 1);
    push_utxo(out, true);
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// Row 1, Defining Commit `7ebc8686e` ("Save and Read wallet", 2019-09-06),
/// replicating `LightWallet::write`, `BlockData::write`, `WalletTx::write`,
/// and `SaplingNoteData::write` in `rust-lightclient/src/lightwallet.rs`. The
/// writer emits a u64 version word, the `Vector<BlockData>`, and the
/// transaction map as a `Vector` of txid and `WalletTx` tuples. No key
/// material reaches disk at all: the reader re-derives the wallet's keys from
/// a hard-coded `[1; 32]`.
///
/// The wallet holds one block at height 1000000 with a zero hash and an empty
/// commitment tree, and one transaction with one note.
///
/// DISCREPANCY: the issue table records row 1 as carrying no version prefix,
/// with the file beginning at a `Vector` length. The source disagrees. This
/// commit's `LightWallet::write` opens with
/// `writer.write_u64::<LittleEndian>(1)?`, and its parent `81b6b52ba` has no
/// `LightWallet::write` at all, so no earlier writer omitted the word. Row 2's
/// commit `c2e26fbbc` only replaces that literal with a call to a new
/// `serialized_version()` that returns 1, which changes no byte. Rows 1 and 2
/// are therefore one grammar, and no discriminator can separate them. These
/// two fixtures differ only in wallet contents: row 1 carries the block that
/// row 2 omits.
fn row_01() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 1);
    push_compact_size(&mut out, 1);
    push_block_data(&mut out, HEIGHT);
    push_transaction_map(&mut out, TX_BORN);
    out
}

/// Row 2, Defining Commit `c2e26fbbc` ("Cleanup", 2019-09-06), replicating
/// `LightWallet::write` in `rust-lightclient/src/lightwallet.rs`. The commit
/// introduces `LightWallet::serialized_version()`,
/// `WalletTx::serialized_version()`, and
/// `SaplingNoteData::serialized_version()`, each returning the literal its
/// writer already emitted.
///
/// The wallet holds no blocks and one transaction. See [`row_01`] for the
/// finding that this grammar is byte-identical to row 1's.
fn row_02() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 1);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_BORN);
    out
}

/// Row 3, Defining Commit `8ff6d15e3` ("Add more fields to sapling note
/// data", 2019-09-06), replicating `SaplingNoteData::write` and
/// `WalletTx::write` in `rust-lightclient/src/lightwallet.rs`. The wallet's
/// own writer is untouched: every new byte lands inside the transaction.
/// `SaplingNoteData` gains a trailing `is_change` u8, and `WalletTx` inserts
/// its own copy of the 32-byte txid after the block height and appends
/// `total_shielded_value_spent` as a u64.
///
/// The wallet is row 2's, so those three fields are the whole difference.
fn row_03() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 1);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_TXID);
    out
}

/// Row 4, Defining Commit `5bd8b754d` ("Derive addresses from seed",
/// 2019-09-07), replicating `LightWallet::write` in
/// `rust-lightclient/src/lightwallet.rs`. The writer gains the raw 32-byte
/// seed and a `Vector<ExtendedSpendingKey>` between the version word and the
/// block vector, so the wallet's own key material reaches disk for the first
/// time.
///
/// The wallet is row 3's plus the zeroed seed and one all-zero spending key.
/// The key vector carries one element because the row's mark is that vector's
/// presence.
fn row_04() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 1);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_TXID);
    out
}

/// Row 5, Defining Commit `db549f5b6` ("scan tx for transparent inputs",
/// 2019-09-13), replicating `WalletTx::write` in
/// `rust-lightclient/src/lightwallet.rs`. The commit teaches the scanner to
/// recognise the wallet's own transparent inputs and records what it finds as
/// `total_transparent_value_spent`, a u64 appended after
/// `total_shielded_value_spent`. The wallet's own writer is untouched.
///
/// The wallet is row 4's, eight bytes longer inside the transaction.
fn row_05() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 1);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_TRANSPARENT_TOTAL);
    out
}

/// Row 6, Defining Commit `f532b70ca` ("Add transparent support for
/// transactions", 2019-09-13), replicating `LightWallet::write` and
/// `Utxo::write` in `rust-lightclient/src/lightwallet.rs`. A standalone
/// `Vector<Utxo>` is inserted between the spending keys and the block vector.
///
/// The wallet holds one UTXO so the new vector is visibly present: the mainnet
/// address of [`TRANSPARENT_ADDRESS`], a zero txid, output index 0, 100000
/// zatoshi at height 1000000, the standard P2PKH script, and neither of the
/// two spend markers set.
fn row_06() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 1);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_standalone_utxo_vector(&mut out);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_TRANSPARENT_TOTAL);
    out
}

/// Row 7, Defining Commit `b24f174b5` ("Separate t / z balance", 2019-09-16),
/// replicating `LightWallet::write` in `rust-lightclient/src/lightwallet.rs`.
/// The writer appends `self.tkeys[0]` as 32 raw, unprefixed bytes between the
/// spending-key vector and the UTXO vector. There is no count and no length,
/// so the reader recovers the field by position alone.
///
/// The wallet is row 6's, plus the zeroed transparent key.
fn row_07() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 1);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_bytes(&mut out, &TRANSPARENT_KEY);
    push_standalone_utxo_vector(&mut out);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_TRANSPARENT_TOTAL);
    out
}

/// Row 8, Defining Commit `0e8ab4d27` ("Read UTXOs from walletTx",
/// 2019-09-17), replicating `LightWallet::write` and `WalletTx::write` in
/// `src/lightwallet.rs`, the path the serializer moved to at this commit. The
/// standalone `Vector<Utxo>` is deleted from the wallet, and the same commit
/// adds a `Vector<Utxo>` inside `WalletTx::write` after the note vector, so
/// UTXOs now travel with their transaction.
///
/// The wallet is row 7's with its one UTXO relocated, which is what makes the
/// move legible: the same 127-byte record appears at a new offset rather than
/// vanishing.
fn row_08() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 1);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_bytes(&mut out, &TRANSPARENT_KEY);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_INLINE_UTXO);
    out
}

/// Row 9, Defining Commit `f93267507` ("Don't write unconfirmed fields to
/// disk", 2019-09-17), replicating `Utxo::write` in `src/lightwallet.rs`. The
/// trailing `Optional<TxId>` that recorded an unconfirmed spend is dropped
/// from both the reader and the writer, on the reasoning that a restarted
/// wallet should not be bound to a transaction that may expire. The wallet's
/// own writer is untouched, and the note record keeps its own spend
/// `Optional`.
///
/// The wallet is row 8's, one byte shorter inside its UTXO.
fn row_09() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 1);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_bytes(&mut out, &TRANSPARENT_KEY);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_TRIMMED_UTXO);
    out
}

/// Row 10, Defining Commit `b0f7d8fcf` ("Write outgoing metadata",
/// 2019-09-19), replicating `OutgoingTxMetadata::write` and `WalletTx::write`
/// in `src/lightwallet/data.rs`, the file the sub-records moved to when
/// `lightwallet` became a directory. `WalletTx::serialized_version()` moves
/// from 1 to 2 and a `Vector<OutgoingTxMetadata>` is appended after the two
/// value-spent totals. Each element writes its address in the `write_string`
/// discipline, its value as a u64, and its memo as 512 raw bytes.
///
/// The transaction carries one outgoing record so the new vector is visibly
/// present, naming the Sapling recipient of [`SAPLING_ADDRESS`]. The reader
/// gates the vector on the sub-version rather than on emptiness, so an empty
/// vector would still mark the field, but only a populated one exercises
/// `OutgoingTxMetadata::write`.
fn row_10() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 1);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_bytes(&mut out, &TRANSPARENT_KEY);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_OUTGOING);
    out
}

/// Row 11, Defining Commit `b3ca226ff` ("Write chain name to wallet",
/// 2019-09-24), replicating `LightWallet::write` in `src/lightwallet/mod.rs`
/// and `utils::write_string` in `src/utils.rs`. The version word moves to 2
/// and the chain name is appended after the transaction map, framed by the
/// `write_string` pair this commit introduces: a u64 little-endian byte length
/// followed by the UTF-8 bytes.
///
/// The wallet is row 10's on mainnet, so the file now ends
/// `04 00 00 00 00 00 00 00 6D 61 69 6E`.
fn row_11() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 2);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_bytes(&mut out, &TRANSPARENT_KEY);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_OUTGOING);
    push_u64_string(&mut out, CHAIN_NAME);
    out
}

/// Row 12, Defining Commit `df12ccf31` ("Explicitly mark Txs as fully
/// scanned", 2019-09-25), replicating `WalletTx::write` in
/// `src/lightwallet/data.rs`. A `full_tx_scanned` u8 closes the transaction
/// record while the sub-version stands still at 2, so the reader's
/// `match version { 1 => false, _ => read_u8() }` cannot tell the two
/// version-2 layouts apart and a file written just before this commit
/// desynchronises. The wallet's own writer is untouched.
///
/// The wallet is row 11's, one byte longer inside the transaction.
fn row_12() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 2);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_bytes(&mut out, &TRANSPARENT_KEY);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_SCANNED);
    push_u64_string(&mut out, CHAIN_NAME);
    out
}

/// Row 13, Defining Commit `88a80f574` (a merge of `dev`, 2019-09-27, whose
/// authoring interior commit is `f78c3fa48`, "Add wallet birthday"),
/// replicating `LightWallet::write` in `src/lightwallet/mod.rs` as the merge's
/// own tree carries it. The birthday is appended after the chain name as a
/// u64, and the version word stays at 2.
///
/// The wallet is row 12's with a birthday of 1000000.
fn row_13() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 2);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_bytes(&mut out, &TRANSPARENT_KEY);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_SCANNED);
    push_u64_string(&mut out, CHAIN_NAME);
    push_u64_le(&mut out, BIRTHDAY);
    out
}

/// Row 14, Defining Commit `ba706ab7c` ("Write vector tkeys", 2019-10-01),
/// replicating `LightWallet::write` in `src/lightwallet/mod.rs`. The single
/// unprefixed 32-byte transparent key of rows 7 through 13 becomes a
/// `Vector<secp256k1::SecretKey>`, so a CompactSize count now precedes the key
/// material. The version word stays at 2.
///
/// The wallet is row 13's, its one transparent key now inside the vector,
/// which makes the file exactly one byte longer.
fn row_14() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 2);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_transparent_key_vector(&mut out);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_SCANNED);
    push_u64_string(&mut out, CHAIN_NAME);
    push_u64_le(&mut out, BIRTHDAY);
    out
}

/// Row 15, Defining Commit `e3f972508` ("Update serialization versions",
/// 2019-10-01), replicating `LightWallet::write` in `src/lightwallet/mod.rs`
/// and `WalletTx::write` in `src/lightwallet/data.rs`. The commit writes no
/// new bytes: `LightWallet::serialized_version()` moves from 2 to 3,
/// `WalletTx::serialized_version()` from 2 to 3, and the reader's version-1
/// branches are dropped.
///
/// The wallet is row 14's. The two fixtures differ at exactly two offsets, the
/// wallet's version word and the transaction's, which is the whole of this
/// row's discriminating evidence.
fn row_15() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 3);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_transparent_key_vector(&mut out);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_SCANNED_V3);
    push_u64_string(&mut out, CHAIN_NAME);
    push_u64_le(&mut out, BIRTHDAY);
    out
}

/// Row 16, Defining Commit `ebf3c7133` ("Add datetime to transactions",
/// 2019-10-18), replicating `WalletTx::write` in
/// `lib/src/lightwallet/data.rs`, the path the sub-records moved to when the
/// crate split into `lib` and `cli`. `WalletTx::serialized_version()` moves
/// from 3 to 4 and a u64 timestamp is inserted between the block height and
/// the txid. The wallet's own writer is untouched and its version word stays
/// at 3.
///
/// The wallet is row 15's, eight bytes longer inside the transaction.
fn row_16() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 3);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_transparent_key_vector(&mut out);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_DATETIME);
    push_u64_string(&mut out, CHAIN_NAME);
    push_u64_le(&mut out, BIRTHDAY);
    out
}

/// Row 17, Defining Commit `e3a0fd2de` ("Support for wallet encryption",
/// 2019-10-18), replicating `LightWallet::write` in `lib/src/lightwallet.rs`.
/// The version word moves to 4, a `locked` u8 is inserted immediately after
/// it, and a `Vector<ExtendedFullViewingKey>` is inserted after the spending
/// keys.
///
/// The wallet is unlocked, so the flag is zero, and it holds one all-zero full
/// viewing key.
fn row_17() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 4);
    push_u8(&mut out, 0);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_full_viewing_keys(&mut out);
    push_transparent_key_vector(&mut out);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_DATETIME);
    push_u64_string(&mut out, CHAIN_NAME);
    push_u64_le(&mut out, BIRTHDAY);
    out
}

/// Row 18, Defining Commit `fc15de568` ("Support mutable wallets",
/// 2019-10-19), replicating `LightWallet::write` in `lib/src/lightwallet.rs`.
/// A `Vector<String>` of transparent addresses is appended after the
/// transparent keys, and the version word stays at 4.
///
/// The wallet is row 17's plus the one address that its transparent key
/// controls.
fn row_18() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 4);
    push_u8(&mut out, 0);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_full_viewing_keys(&mut out);
    push_transparent_key_vector(&mut out);
    push_transparent_address_vector(&mut out);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_DATETIME);
    push_u64_string(&mut out, CHAIN_NAME);
    push_u64_le(&mut out, BIRTHDAY);
    out
}

/// Row 19, Defining Commit `72548e077` ("Add lock/unlock API", 2019-10-19),
/// replicating `LightWallet::write` in `lib/src/lightwallet.rs`. The encrypted
/// seed and its nonce are inserted between the `locked` flag and the plaintext
/// seed: `enc_seed` is the `[u8; 48]` field written raw with `write_all`, and
/// `nonce` is a `Vector<u8>`. The version word stays at 4.
///
/// The wallet is unlocked, which is what makes it minimal here. `enc_seed`
/// holds its 48 zero bytes and `nonce` is empty, exactly as an unencrypted
/// wallet writes them, and the plaintext seed still follows.
fn row_19() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 4);
    push_bytes(&mut out, &encryption_era_body());
    out
}

/// Row 20, Defining Commit `796663c97` ("Gzip the output", 2020-04-12),
/// replicating `LightWallet::write` in `lib/src/lightwallet.rs`. The version
/// word moves to 5 and is the last plaintext byte of the file: the writer
/// wraps the remaining output in `AutoFinishUnchecked::new(Encoder::new(out))`,
/// so the whole body becomes one compressed frame.
///
/// The body is byte for byte row 19's, because no grammar change landed on the
/// line between 2019-10-19 and 2020-04-12. The two rows differ only in the
/// version word and the compression.
///
/// DISCREPANCY: the issue table records a zstd frame here. The source says
/// gzip. The commit adds `libflate = "0.1"` to `lib/Cargo.toml` and imports
/// `libflate::{gzip::{Decoder, Encoder}, finish::AutoFinishUnchecked}`, so the
/// magic at offset 8 is `1F 8B`, not zstd's `28 B5 2F FD`. The commit message
/// is accurate and the table's note is not.
fn row_20() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 5);
    push_bytes(&mut out, ROW_20_GZIP_FRAME);
    out
}

/// Row 21, Defining Commit `cbffd69c6` ("Undo the gzip encoding",
/// 2020-05-09), replicating `LightWallet::write` in `lib/src/lightwallet.rs`.
/// The encoder is removed and the version word moves to 6. The reader keeps a
/// `version != 5` guard so that the one compressed generation stays readable.
///
/// The wallet is row 19's and row 20's, written in the clear, so this fixture
/// differs from row 19's only in the version word and holds exactly what row
/// 20's frame decompresses to.
fn row_21() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 6);
    push_bytes(&mut out, &encryption_era_body());
    out
}

/// Everything after the version word for rows 19, 20, and 21, whose writers
/// agree byte for byte: the `locked` flag (renamed `encrypted` by row 20), the
/// raw `enc_seed`, the nonce vector, the raw seed, the spending keys, the full
/// viewing keys, the transparent keys, the transparent addresses, the block
/// vector, the transaction map, the chain name, and the birthday.
fn encryption_era_body() -> Vec<u8> {
    let mut out = Vec::new();
    push_u8(&mut out, 0);
    push_bytes(&mut out, &ENC_SEED);
    push_compact_size(&mut out, 0);
    push_bytes(&mut out, &SEED);
    push_spending_keys(&mut out);
    push_full_viewing_keys(&mut out);
    push_transparent_key_vector(&mut out);
    push_transparent_address_vector(&mut out);
    push_compact_size(&mut out, 0);
    push_transaction_map(&mut out, TX_DATETIME);
    push_u64_string(&mut out, CHAIN_NAME);
    push_u64_le(&mut out, BIRTHDAY);
    out
}

/// The gzip frame that row 20 carries after its version word: the bytes of
/// [`encryption_era_body`] compressed once, ahead of time, and embedded here.
///
/// The workbench crate is deliberately std-only, so it cannot compress at
/// runtime. The frame was produced by piping the body through the system gzip,
/// with no intermediate file:
///
/// ```text
/// cargo test --lib wallet_grammars::era_inception::tests::dump_row_20_body \
///     -- --nocapture --ignored \
///   | rg '^ROW20BODY ' | cut -d' ' -f2 \
///   | python3 -c "import sys, subprocess; \
///       body = bytes.fromhex(sys.stdin.read().strip()); \
///       print(subprocess.run(['gzip', '-n', '-c'], input=body, \
///                            capture_output=True).stdout.hex())"
/// ```
///
/// The tool was GNU `gzip 1.14-modified` on Arch Linux, run on 2026-07-29 at
/// its default compression level, with `-n` so that neither a file name nor a
/// modification time enters the header.
///
// ASSUMPTION: the deflate bit stream is gzip's, not libflate 0.1's. Deflate
// output is not canonical, so two conforming compressors agree on a frame's
// framing but not on its interior, and libflate 0.1 is unavailable to a
// std-only crate. The header does match what libflate 0.1 emitted: its default
// `gzip::Header` carries modification time 0, an unknown compression level
// (XFL 0), and OS 3 (Unix), which is what `gzip -n` writes on this host, the
// ten bytes `1F 8B 08 00 00 00 00 00 00 03`. The trailer is bound to the
// derived body by `the_row_20_frame_wraps_the_derived_body`, which recomputes
// the CRC-32 and the input length gzip recorded. A recognizer keying on the
// magic, the header, or the trailer therefore sees what the historical writer
// produced; only the compressed interior is a stand-in.
const ROW_20_GZIP_FRAME: &[u8] = &[
    0x1F, 0x8B, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x63, 0x60, 0xA0, 0x36, 0x60, 0xA4,
    0xBA, 0x89, 0xB4, 0x02, 0xC3, 0xC8, 0xA5, 0x8C, 0xCA, 0x50, 0x46, 0x89, 0xA1, 0x47, 0x71, 0xB2,
    0xA1, 0x4F, 0x90, 0x45, 0xA5, 0x77, 0x5E, 0x52, 0x52, 0xAA, 0x71, 0x49, 0x79, 0x50, 0x81, 0x85,
    0x45, 0x81, 0x59, 0x99, 0x5B, 0x9A, 0xB3, 0x69, 0x89, 0xB9, 0x8B, 0x4F, 0x52, 0x31, 0x61, 0xD3,
    0x58, 0xA0, 0xB4, 0x83, 0x13, 0x3F, 0x43, 0x83, 0xCF, 0x94, 0x58, 0xA2, 0x9C, 0x38, 0x74, 0x42,
    0x13, 0x37, 0x08, 0x38, 0x4C, 0x3D, 0xB3, 0xE0, 0x01, 0x02, 0x8A, 0x1A, 0x62, 0xA2, 0x85, 0x48,
    0xB0, 0xA0, 0x0D, 0x62, 0x30, 0x28, 0x72, 0x24, 0xCB, 0x56, 0x8A, 0x60, 0x53, 0xD3, 0xB1, 0x06,
    0xD3, 0x39, 0x7E, 0x50, 0x46, 0x55, 0xB1, 0x61, 0x21, 0x35, 0x40, 0x41, 0xA1, 0x59, 0x8A, 0x45,
    0xFA, 0x8A, 0x44, 0x92, 0x02, 0x65, 0x14, 0x0C, 0x57, 0x00, 0x2B, 0x33, 0x72, 0x13, 0x33, 0xF3,
    0x40, 0x49, 0x13, 0x04, 0x00, 0x47, 0xDA, 0xA2, 0x50, 0x5D, 0x06, 0x00, 0x00,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The little-endian u64 at offset 0.
    fn version_word(bytes: &[u8]) -> u64 {
        u64::from_le_bytes(bytes[..8].try_into().expect("fixture has a version word"))
    }

    /// Whether `haystack` contains `needle` anywhere.
    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    /// The bytes of one `WalletTx` under `shape`, for length arithmetic.
    fn wallet_tx(shape: TransactionShape) -> Vec<u8> {
        let mut out = Vec::new();
        push_wallet_tx(&mut out, shape);
        out
    }

    /// CRC-32 as gzip computes it, over the reflected IEEE polynomial
    /// `0xEDB88320`, so a test can bind the embedded frame's trailer to the
    /// body this module derives.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// The issue table claims row 1 writes no version word. It does. This test
    /// records the finding rather than the claim: both rows open with the
    /// 8-byte little-endian 1, `c2e26fbbc` changed no byte, and so the two
    /// fixtures are separated only by their wallet contents.
    #[test]
    fn rows_01_and_02_are_one_grammar_and_both_open_with_version_one() {
        let (row_1, row_2) = (row_01(), row_02());
        assert_eq!(version_word(&row_1), 1);
        assert_eq!(version_word(&row_2), 1);
        assert_eq!(row_1.len(), row_2.len() + 47);
        assert_eq!(row_1[8], 1);
        assert_eq!(row_2[8], 0);
        assert_ne!(row_1, row_2);
    }

    /// Row 1's one block reaches disk: the block vector's count is 1, and the
    /// block closes with `BlockData::write`'s literal end tag 11. Everything
    /// after it is row 2's transaction map.
    #[test]
    fn row_01_carries_one_block_with_its_end_tag() {
        let bytes = row_01();
        assert_eq!(&bytes[9..13], &HEIGHT.to_le_bytes());
        assert_eq!(&bytes[45..48], &[0, 0, 0]);
        assert_eq!(&bytes[48..56], &11u64.to_le_bytes());
        assert_eq!(&bytes[56..], &row_02()[9..]);
    }

    /// Every row's version word is the one its Defining Commit's
    /// `LightWallet::serialized_version()` returns.
    #[test]
    fn version_words_match_the_defining_commits() {
        let expected = [
            (row_01(), 1),
            (row_02(), 1),
            (row_03(), 1),
            (row_04(), 1),
            (row_05(), 1),
            (row_06(), 1),
            (row_07(), 1),
            (row_08(), 1),
            (row_09(), 1),
            (row_10(), 1),
            (row_11(), 2),
            (row_12(), 2),
            (row_13(), 2),
            (row_14(), 2),
            (row_15(), 3),
            (row_16(), 3),
            (row_17(), 4),
            (row_18(), 4),
            (row_19(), 4),
            (row_20(), 5),
            (row_21(), 6),
        ];
        for (index, (bytes, version)) in expected.iter().enumerate() {
            assert_eq!(version_word(bytes), *version, "row {}", index + 1);
        }
    }

    /// Row 3's mark: the transaction record gains its own txid and a shielded
    /// total, and the note record gains `is_change`. The wallet's own bytes do
    /// not move, so the whole delta is 41 bytes inside the transaction.
    #[test]
    fn row_03_grows_the_transaction_and_note_records() {
        let (row_2, row_3) = (row_02(), row_03());
        assert_eq!(&row_3[..9], &row_2[..9]);
        assert_eq!(row_3.len(), row_2.len() + 32 + 8 + 1);
        assert_eq!(
            wallet_tx(TX_TXID).len(),
            wallet_tx(TX_BORN).len() + 32 + 8 + 1
        );
        let inner_txid = 8 + 1 + 32 + 8 + 4;
        assert_eq!(&row_3[inner_txid..inner_txid + 32], &[0u8; 32]);
    }

    /// Row 4's mark: the raw seed sits immediately after the version word, and
    /// the spending-key vector's single 169-byte element follows it.
    #[test]
    fn row_04_carries_the_seed_then_one_spending_key() {
        let (row_3, row_4) = (row_03(), row_04());
        assert_eq!(&row_4[8..40], &SEED);
        assert_eq!(row_4[40], 1);
        assert_eq!(&row_4[41..210], &EXTENDED_SPENDING_KEY);
        assert_eq!(row_4.len(), row_3.len() + 32 + 1 + 169);
    }

    /// Row 5's mark: a second value-spent total closes the transaction record,
    /// so the file is exactly eight bytes longer than row 4's.
    #[test]
    fn row_05_appends_the_transparent_value_spent_total() {
        let (row_4, row_5) = (row_04(), row_05());
        assert_eq!(row_5.len(), row_4.len() + 8);
        assert_eq!(
            wallet_tx(TX_TRANSPARENT_TOTAL).len(),
            wallet_tx(TX_TXID).len() + 8
        );
        assert_eq!(&row_5[row_5.len() - 16..], &[0u8; 16]);
    }

    /// Row 6's mark: a standalone UTXO vector holds one element, whose
    /// sub-record version is 1 and whose address is visible as ASCII.
    #[test]
    fn row_06_carries_one_standalone_utxo() {
        let (row_5, row_6) = (row_05(), row_06());
        let count = 8 + 32 + 1 + 169;
        assert_eq!(row_6[count], 1);
        assert_eq!(&row_6[count + 1..count + 9], &1u64.to_le_bytes());
        assert!(contains(&row_6, TRANSPARENT_ADDRESS.as_bytes()));
        assert!(!contains(&row_5, TRANSPARENT_ADDRESS.as_bytes()));
        assert_eq!(row_6.len(), row_5.len() + 1 + 127);
    }

    /// Row 7's mark: 32 unprefixed transparent-key bytes sit between the
    /// spending-key vector and the UTXO vector, making the file exactly 32
    /// bytes longer than row 6's.
    #[test]
    fn row_07_inserts_a_bare_transparent_key() {
        let (row_6, row_7) = (row_06(), row_07());
        let split = 8 + 32 + 1 + 169;
        assert_eq!(row_7.len(), row_6.len() + 32);
        assert_eq!(&row_7[..split], &row_6[..split]);
        assert_eq!(&row_7[split..split + 32], &TRANSPARENT_KEY);
        assert_eq!(&row_7[split + 32..], &row_6[split..]);
    }

    /// Row 8's mark: the standalone UTXO vector is gone from the wallet and
    /// the same 127-byte record reappears inside the transaction, so the file
    /// keeps its length while the address moves far to the right.
    #[test]
    fn row_08_moves_the_utxo_into_the_transaction() {
        let (row_7, row_8) = (row_07(), row_08());
        assert_eq!(row_8.len(), row_7.len());
        assert_ne!(row_8, row_7);
        assert!(contains(&row_8, TRANSPARENT_ADDRESS.as_bytes()));
        let utxo_vector_count = 8 + 32 + 1 + 169 + 32;
        assert_eq!(row_7[utxo_vector_count], 1);
        assert_eq!(row_8[utxo_vector_count], 0);
        assert_eq!(
            wallet_tx(TX_INLINE_UTXO).len(),
            wallet_tx(TX_TRANSPARENT_TOTAL).len() + 1 + 127
        );
    }

    /// Row 9's mark: the UTXO record loses its trailing unconfirmed-spent
    /// `Optional`, so the file is exactly one byte shorter than row 8's.
    #[test]
    fn row_09_trims_the_utxo_unconfirmed_spend_marker() {
        let (row_8, row_9) = (row_08(), row_09());
        assert_eq!(row_9.len(), row_8.len() - 1);
        let mut trimmed = Vec::new();
        push_utxo(&mut trimmed, false);
        let mut full = Vec::new();
        push_utxo(&mut full, true);
        assert_eq!(trimmed.len(), 126);
        assert_eq!(full.len(), 127);

        // The trimmed record is a prefix of the untrimmed one, so distinguish
        // them by the whole transaction map rather than by the UTXO alone.
        let mut map_with_full = Vec::new();
        push_transaction_map(&mut map_with_full, TX_INLINE_UTXO);
        let mut map_with_trimmed = Vec::new();
        push_transaction_map(&mut map_with_trimmed, TX_TRIMMED_UTXO);
        assert!(contains(&row_8, &map_with_full));
        assert!(contains(&row_9, &map_with_trimmed));
        assert!(!contains(&row_9, &map_with_full));
    }

    /// Row 10's marks: the transaction sub-version moves from 1 to 2 and a
    /// populated outgoing-metadata vector closes the record, carrying its
    /// Sapling recipient and a 512-byte memo.
    #[test]
    fn row_10_bumps_the_transaction_and_adds_outgoing_metadata() {
        let (row_9, row_10) = (row_09(), row_10());
        let mut metadata = Vec::new();
        push_outgoing_tx_metadata(&mut metadata);
        assert_eq!(metadata.len(), 8 + 78 + 8 + 512);
        assert!(contains(&row_10, SAPLING_ADDRESS.as_bytes()));
        assert!(!contains(&row_9, SAPLING_ADDRESS.as_bytes()));
        assert_eq!(row_10.len(), row_9.len() + 1 + metadata.len());
        let tx_version = 8 + 32 + 1 + 169 + 32 + 1 + 1 + 32;
        assert_eq!(&row_10[tx_version..tx_version + 8], &2u64.to_le_bytes());
        assert_eq!(&row_9[tx_version..tx_version + 8], &1u64.to_le_bytes());
    }

    /// Row 11's mark: the chain name closes the file in `write_string`
    /// discipline, a u64 length followed by ASCII, and everything before it is
    /// row 10's under the new version word.
    #[test]
    fn row_11_appends_the_chain_name_string() {
        let (row_10, row_11) = (row_10(), row_11());
        let body = row_11.len() - 12;
        assert_eq!(&row_11[body..body + 8], &4u64.to_le_bytes());
        assert_eq!(&row_11[body + 8..], b"main");
        assert_eq!(&row_11[8..body], &row_10[8..]);
    }

    /// Row 12's mark: a `full_tx_scanned` byte closes the transaction record
    /// while its sub-version stands still at 2, which is why a version-2 file
    /// from before this commit desynchronises its reader.
    #[test]
    fn row_12_appends_full_tx_scanned_without_a_sub_version_bump() {
        let (row_11, row_12) = (row_11(), row_12());
        assert_eq!(row_12.len(), row_11.len() + 1);
        assert_eq!(TX_SCANNED.version, TX_OUTGOING.version);
        assert_eq!(
            wallet_tx(TX_SCANNED).len(),
            wallet_tx(TX_OUTGOING).len() + 1
        );
    }

    /// Row 13's mark: the birthday closes the file as a u64, and everything
    /// before it is row 12's.
    #[test]
    fn row_13_appends_the_birthday() {
        let (row_12, row_13) = (row_12(), row_13());
        assert_eq!(row_13.len(), row_12.len() + 8);
        assert_eq!(&row_13[..row_12.len()], &row_12[..]);
        assert_eq!(&row_13[row_12.len()..], &BIRTHDAY.to_le_bytes());
    }

    /// Row 14's mark: a CompactSize count of 1 now precedes the transparent
    /// key material, which lengthens the file by exactly that one byte.
    #[test]
    fn row_14_prefixes_the_transparent_keys_with_a_count() {
        let (row_13, row_14) = (row_13(), row_14());
        let split = 8 + 32 + 1 + 169;
        assert_eq!(row_14.len(), row_13.len() + 1);
        assert_eq!(&row_14[..split], &row_13[..split]);
        assert_eq!(row_14[split], 1);
        assert_eq!(&row_14[split + 1..split + 33], &TRANSPARENT_KEY);
    }

    /// Row 15's mark is two version words and nothing else: the wallet's moves
    /// from 2 to 3 and the transaction's from 2 to 3, with every other byte
    /// unchanged from row 14's.
    #[test]
    fn row_15_changes_only_the_two_version_words() {
        let (row_14, row_15) = (row_14(), row_15());
        assert_eq!(row_15.len(), row_14.len());
        let differing: Vec<usize> = (0..row_15.len())
            .filter(|index| row_15[*index] != row_14[*index])
            .collect();
        let tx_version = 8 + 32 + 1 + 169 + 1 + 32 + 1 + 1 + 32;
        assert_eq!(differing, vec![0, tx_version]);
        assert_eq!(row_15[0], 3);
        assert_eq!(row_15[tx_version], 3);
    }

    /// Row 16's marks: the transaction sub-version moves from 3 to 4 and a u64
    /// timestamp is inserted between the block height and the txid.
    #[test]
    fn row_16_inserts_the_transaction_datetime() {
        let (row_15, row_16) = (row_15(), row_16());
        assert_eq!(row_16.len(), row_15.len() + 8);
        assert_eq!(TX_DATETIME.version, 4);
        let tx_version = 8 + 32 + 1 + 169 + 1 + 32 + 1 + 1 + 32;
        assert_eq!(&row_16[tx_version..tx_version + 8], &4u64.to_le_bytes());
        let datetime = tx_version + 8 + 4;
        assert_eq!(&row_16[datetime..datetime + 8], &DATETIME.to_le_bytes());
    }

    /// Row 17's marks: a `locked` flag directly after the version word, and a
    /// full-viewing-key vector after the spending keys.
    #[test]
    fn row_17_adds_the_locked_flag_and_the_viewing_keys() {
        let (row_16, row_17) = (row_16(), row_17());
        assert_eq!(row_17[8], 0);
        assert_eq!(&row_17[9..41], &SEED);
        let fvk_count = 9 + 32 + 1 + 169;
        assert_eq!(row_17[fvk_count], 1);
        assert_eq!(
            &row_17[fvk_count + 1..fvk_count + 170],
            &EXTENDED_FULL_VIEWING_KEY
        );
        assert_eq!(row_17.len(), row_16.len() + 1 + 1 + 169);
    }

    /// Row 18's mark: a vector of transparent-address strings, whose one
    /// element carries its own u64 length inside the CompactSize-counted
    /// vector.
    #[test]
    fn row_18_appends_the_transparent_address_vector() {
        let (row_17, row_18) = (row_17(), row_18());
        let mut vector = Vec::new();
        push_transparent_address_vector(&mut vector);
        assert_eq!(vector.len(), 44);
        assert!(contains(&row_18, &vector));
        assert_eq!(row_18.len(), row_17.len() + 44);
    }

    /// Row 19's mark: the 48-byte `enc_seed` and the nonce vector are inserted
    /// between the `locked` flag and the plaintext seed.
    #[test]
    fn row_19_inserts_the_encrypted_seed_and_nonce() {
        let (row_18, row_19) = (row_18(), row_19());
        assert_eq!(row_19[8], 0);
        assert_eq!(&row_19[9..57], &ENC_SEED);
        assert_eq!(row_19[57], 0);
        assert_eq!(&row_19[58..90], &SEED);
        assert_eq!(row_19.len(), row_18.len() + 49);
    }

    /// Row 20's mark: a gzip frame begins at offset 8, immediately after the
    /// plaintext version word 5. The table's zstd magic is nowhere in the
    /// file.
    #[test]
    fn row_20_starts_a_gzip_frame_at_offset_eight() {
        let bytes = row_20();
        assert_eq!(version_word(&bytes), 5);
        assert_eq!(&bytes[8..10], &[0x1F, 0x8B]);
        assert_eq!(
            &bytes[10..18],
            &[0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03]
        );
        assert!(!contains(&bytes, &[0x28, 0xB5, 0x2F, 0xFD]));
        assert!(bytes.len() < row_21().len());
    }

    /// The embedded frame really wraps the body this module derives: gzip's
    /// trailer records the uncompressed CRC-32 and length, and both match
    /// [`encryption_era_body`]. This is what ties the pre-computed constant
    /// back to the writer source, since a std-only crate cannot decompress it.
    #[test]
    fn the_row_20_frame_wraps_the_derived_body() {
        let body = encryption_era_body();
        let trailer = &ROW_20_GZIP_FRAME[ROW_20_GZIP_FRAME.len() - 8..];
        assert_eq!(
            u32::from_le_bytes(trailer[..4].try_into().expect("four CRC bytes")),
            crc32(&body)
        );
        assert_eq!(
            u32::from_le_bytes(trailer[4..].try_into().expect("four length bytes")),
            body.len() as u32
        );
    }

    /// Row 21's mark: the frame is gone and the body is plaintext again. The
    /// fixture is row 19's under a different version word, and its body is
    /// what row 20's frame decompresses to.
    #[test]
    fn row_21_restores_the_plaintext_body() {
        let bytes = row_21();
        assert_eq!(version_word(&bytes), 6);
        assert_ne!(&bytes[8..10], &[0x1F, 0x8B]);
        assert_eq!(&bytes[8..], &encryption_era_body()[..]);
        assert_eq!(&bytes[8..], &row_19()[8..]);
    }

    /// No two rows in this era collapse to the same bytes, which is the
    /// corpus-wide distinctness claim restricted to the rows this module owns.
    #[test]
    fn all_rows_are_pairwise_distinct() {
        let fixtures = fixtures();
        for (index, a) in fixtures.iter().enumerate() {
            for b in &fixtures[index + 1..] {
                assert_ne!(
                    a.bytes, b.bytes,
                    "rows {} and {} produced identical bytes",
                    a.row, b.row
                );
            }
        }
    }

    /// The era contributes rows 1 through 21 of the 77-row table, in order,
    /// each tagged with its Defining Commit and with `dev`.
    #[test]
    fn the_era_covers_rows_one_through_twenty_one() {
        let fixtures = fixtures();
        let rows: Vec<u8> = fixtures.iter().map(|f| f.row).collect();
        assert_eq!(rows, (1..=21).collect::<Vec<u8>>());
        assert!(fixtures.iter().all(|f| f.branch == "dev"));
        assert!(fixtures.iter().all(|f| f.defining_commit.len() == 9));
    }

    /// Prints the exact bytes row 20 compresses, so that the embedded frame
    /// can be regenerated by the pipeline documented on [`ROW_20_GZIP_FRAME`].
    /// It is ignored by default because it asserts nothing.
    #[test]
    #[ignore = "regeneration aid: prints the row 20 body for the gzip pipeline"]
    fn dump_row_20_body() {
        let hex: String = encryption_era_body()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        println!("ROW20BODY {hex}");
    }
}
