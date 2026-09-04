//! Census rows 29 through 37: the WalletCapability era, wallet versions 25
//! through 31.
//!
//! This era opens with the last two grammars whose key store is still a loose
//! record (row 29's legacy `Keys`, row 30's `UnifiedSpendCapability`) and
//! closes with the two version-31 layouts that the sync integration left
//! behind. Across the nine rows the key store is rewritten four times — the
//! `Capability` triple at row 32, the `UnifiedKeyStore` at row 34, the
//! ephemeral-address count at row 35, and its wholesale removal at row 37 —
//! so the sub-version byte the key store writes just after the file's
//! version word is the era's most useful discriminator.
//!
//! Every fixture is a minimal spend-capable wallet: no blocks, no
//! transactions, no witness trees, one unified address bearing all three
//! receivers, a 32-byte all-zero mnemonic entropy, mainnet, and a birthday of
//! 2,000,000. Fixed-width key material is zeroed except where the historical
//! reader would reject a zero — a secp256k1 secret key must be non-zero, so
//! the transparent key bytes are all ones.

use super::util::{
    push_bytes, push_compact_size, push_compact_vec_u8, push_optional_none, push_optional_some,
    push_u32_le, push_u64_le, push_u64_string, push_u8,
};
use super::Fixture;

/// The chain name every fixture carries. `ChainType`/`Network` rendered
/// `Mainnet` as `"main"` at all nine Defining Commits.
const CHAIN_NAME: &str = "main";

/// The wallet birthday, a plausible mainnet height for this era.
const BIRTHDAY: u64 = 2_000_000;

/// `MemoDownloadOption::WalletMemos`, the `WalletOptions` default, whose
/// discriminant was 1 throughout the era.
const MEMO_DOWNLOAD_WALLET_MEMOS: u8 = 1;

/// `MAX_TRANSACTION_SIZE_DEFAULT`, the `WalletOptions` transaction-size
/// filter default.
const TRANSACTION_SIZE_FILTER: u32 = 500;

/// The mnemonic entropy every fixture carries: 32 zero bytes, the entropy of
/// a 24-word seed phrase.
const SEED_ENTROPY: [u8; 32] = [0u8; 32];

/// The mnemonic account index appended from row 33 onward.
const MNEMONIC_ACCOUNT_INDEX: u32 = 0;

/// The height paired with row 29's single Orchard anchor.
const ANCHOR_HEIGHT: u32 = 1_950_000;

/// Stand-in bytes for a secp256k1 secret key. Zero is not a valid secret key,
/// so `SecretKey::from_slice` would reject an all-zero blob; every other
/// 32-byte value in range is accepted.
const DUMMY_SECP_SECRET_KEY: [u8; 32] = [1u8; 32];

/// The receiver bitmask for a unified address holding an Orchard, a Sapling,
/// and a transparent receiver. `ReceiverSelection` packed the three receivers
/// into bits 0, 1, and 2 throughout the era.
const ALL_RECEIVERS: u8 = 0b111;

/// `Era::Orchard`'s identifier in `zcash_keys`: the NU5 consensus branch id,
/// written as a little-endian u32 at the head of a serialized
/// `UnifiedSpendingKey`.
const ERA_ORCHARD_ID: u32 = 0xc2d6_d0b4;

/// Every fixture this era contributes, in census order.
pub fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            row: 29,
            defining_commit: "6b6ed912e",
            branch: "dev",
            bytes: row_29(),
        },
        Fixture {
            row: 30,
            defining_commit: "cc78c2358",
            branch: "dev",
            bytes: row_30(),
        },
        Fixture {
            row: 31,
            defining_commit: "18014a7ee",
            branch: "dev",
            bytes: row_31(),
        },
        Fixture {
            row: 32,
            defining_commit: "939ef32b1",
            branch: "dev",
            bytes: row_32(),
        },
        Fixture {
            row: 33,
            defining_commit: "33daec1d1",
            branch: "dev",
            bytes: row_33(),
        },
        Fixture {
            row: 34,
            defining_commit: "fd86965ea",
            branch: "dev",
            bytes: row_34(),
        },
        Fixture {
            row: 35,
            defining_commit: "eb2210e79",
            branch: "dev",
            bytes: row_35(),
        },
        Fixture {
            row: 36,
            defining_commit: "b82fbe17b",
            branch: "dev",
            bytes: row_36(),
        },
        Fixture {
            row: 37,
            defining_commit: "db3f7f716",
            branch: "dev",
            bytes: row_37(),
        },
    ]
}

/// Row 29, Defining Commit `6b6ed912e` (merge of PR #131, authored in
/// `77c570f58`), wallet version 25.
///
/// Replicates `LightWallet::write` in `lib/src/wallet.rs`, together with
/// `Keys::write` in `lib/src/wallet/keys.rs`, `TransactionMetadataSet::write`
/// in `lib/src/wallet/transactions.rs`, `WalletZecPriceInfo::write` in
/// `lib/src/wallet/data.rs`, and `utils::write_string`.
///
/// The wallet holds the legacy `Keys` record at its own version 22 — this row
/// predates the `WalletCapability` rewrite despite opening the era module —
/// with no Sapling, Orchard, or transparent keys, no blocks, and no
/// transactions. The grammar's unique mark is the trailing vector of
/// `(Orchard anchor, height)` pairs, so it holds one entry: a zeroed 32-byte
/// anchor at height 1,950,000.
fn row_29() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 25);

    // Keys::write, serialized version 22.
    push_u64_le(&mut out, 22);
    push_u8(&mut out, 0); // not encrypted
    push_bytes(&mut out, &[0u8; 48]); // enc_seed, written raw and unprefixed
    push_compact_vec_u8(&mut out, &[]); // nonce
    push_bytes(&mut out, &SEED_ENTROPY); // seed, written raw and unprefixed
    push_compact_size(&mut out, 0); // Sapling keys
    push_compact_size(&mut out, 0); // Orchard keys
    push_compact_size(&mut out, 0); // transparent keys

    push_compact_size(&mut out, 0); // blocks

    // TransactionMetadataSet::write, serialized version 21.
    push_u64_le(&mut out, 21);
    push_compact_size(&mut out, 0);

    push_u64_string(&mut out, CHAIN_NAME);
    push_wallet_options(&mut out);
    push_u64_le(&mut out, BIRTHDAY);
    push_optional_none(&mut out); // verified_tree
    push_price_info(&mut out);

    // The grammar's mark: Vector<(orchard anchor, height)>, holding one pair.
    push_compact_size(&mut out, 1);
    // ASSUMPTION: `orchard::Anchor::to_bytes` yields the 32-byte
    // little-endian canonical encoding of a Pallas base field element, and
    // zero is a valid element.
    push_bytes(&mut out, &[0u8; 32]);
    push_u32_le(&mut out, ANCHOR_HEIGHT);

    out
}

/// Row 30, Defining Commit `cc78c2358` (merge of PR #93, authored in
/// `533b44a65`), wallet version 25 unbumped.
///
/// Replicates `LightWallet::write` in `lib/src/wallet.rs` and
/// `<UnifiedSpendCapability as ReadableWriteable<()>>::write` in
/// `lib/src/wallet/keys/unified.rs`, whose `VERSION` is 1.
///
/// The legacy `Keys` record is gone: the key store is now a single
/// `UnifiedSpendCapability` holding an Orchard spending key, a Sapling
/// extended spending key, a transparent extended private key, one unified
/// address, and a trailing `encrypted` flag. The grammar's unique mark is the
/// mnemonic-entropy vector appended after the anchor vector, so the fixture
/// carries both.
fn row_30() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 25);

    // UnifiedSpendCapability::write, VERSION 1.
    push_u8(&mut out, 1);
    push_orchard_spending_key(&mut out);
    push_sapling_extended_spending_key(&mut out);
    push_legacy_extended_priv_key(&mut out);
    push_receiver_selection_vector(&mut out);
    push_u8(&mut out, 0); // encrypted, written only at this Defining Commit

    push_compact_size(&mut out, 0); // blocks
    push_u64_le(&mut out, 21); // TransactionMetadataSet, version 21
    push_compact_size(&mut out, 0);

    push_u64_string(&mut out, CHAIN_NAME);
    push_wallet_options(&mut out);
    push_u64_le(&mut out, BIRTHDAY);
    push_optional_none(&mut out); // verified_tree
    push_price_info(&mut out);

    push_compact_size(&mut out, 1); // orchard anchors
    push_bytes(&mut out, &[0u8; 32]);
    push_u32_le(&mut out, ANCHOR_HEIGHT);

    // The grammar's mark: the mnemonic entropy, appended as a byte vector.
    push_compact_vec_u8(&mut out, &SEED_ENTROPY);

    out
}

/// Row 31, Defining Commit `18014a7ee` (merge of PR #182, authored in
/// `c684b76f1`), wallet version 26.
///
/// Replicates `LightWallet::write` in `lib/src/wallet.rs` and
/// `<UnifiedSpendCapability as ReadableWriteable<()>>::write` in
/// `lib/src/wallet/keys/unified.rs`, still at `VERSION` 1.
///
/// The wallet contents match row 30's, and the grammar differs in two places
/// rather than the one the census records. The named delta is the removal of
/// the `(Orchard anchor, height)` vector. The unnamed one is inside the
/// capability: this commit's `write` stops emitting the trailing `encrypted`
/// byte, so the capability record is one byte shorter than row 30's.
fn row_31() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 26);

    // UnifiedSpendCapability::write, VERSION 1, without the encrypted flag.
    push_u8(&mut out, 1);
    push_orchard_spending_key(&mut out);
    push_sapling_extended_spending_key(&mut out);
    push_legacy_extended_priv_key(&mut out);
    push_receiver_selection_vector(&mut out);

    push_compact_size(&mut out, 0); // blocks
    push_u64_le(&mut out, 21); // TransactionMetadataSet, version 21
    push_compact_size(&mut out, 0);

    push_u64_string(&mut out, CHAIN_NAME);
    push_wallet_options(&mut out);
    push_u64_le(&mut out, BIRTHDAY);
    push_optional_none(&mut out); // verified_tree
    push_price_info(&mut out);
    push_compact_vec_u8(&mut out, &SEED_ENTROPY);

    out
}

/// Row 32, Defining Commit `939ef32b1` (merge of PR #262, authored in
/// `ffd2c4023`), wallet version 27.
///
/// Replicates `LightWallet::write` in `zingolib/src/wallet.rs` — the writer
/// has moved from `lib/` to `zingolib/` — together with
/// `<WalletCapability as ReadableWriteable<()>>::write` and the generic
/// `<Capability<V, S> as ReadableWriteable<()>>::write` in
/// `zingolib/src/wallet/keys/unified.rs`.
///
/// The key store is now the `WalletCapability` triple, and its `VERSION` byte
/// reads 2 — the grammar's cheapest mark, sitting at offset 8. Each of the
/// three capabilities writes its own version byte, a variant tag, and its key.
/// The fixture chooses `Capability::Spend` for all three, which is what a
/// seed-derived wallet writes; the row's other delta, Sapling's extended full
/// viewing key giving way to a diversifiable one, surfaces only in the
/// `Capability::View` variant and so leaves no trace here. The seed vector,
/// which this commit permits to be empty when no mnemonic exists, carries the
/// 32-byte entropy.
fn row_32() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 27);
    push_wallet_capability_v2(&mut out);

    push_compact_size(&mut out, 0); // blocks
    push_u64_le(&mut out, 21); // TransactionMetadataSet, version 21
    push_compact_size(&mut out, 0);

    push_u64_string(&mut out, CHAIN_NAME);
    push_wallet_options(&mut out);
    push_u64_le(&mut out, BIRTHDAY);
    push_optional_none(&mut out); // verified_tree
    push_price_info(&mut out);
    push_compact_vec_u8(&mut out, &SEED_ENTROPY);

    out
}

/// Row 33, Defining Commit `33daec1d1` (merge of PR #474, authored in
/// `8f9eb1c74`), wallet version 28.
///
/// Replicates `LightWallet::write` in `zingolib/src/wallet.rs`,
/// `WalletCapability::write` in `zingolib/src/wallet/keys/unified.rs` (still
/// `VERSION` 2), and `TransactionMetadataSet::write` in
/// `zingolib/src/wallet/transactions.rs`.
///
/// The census names one delta, the u32 mnemonic account index appended after
/// the seed vector, and the fixture writes it as 0. A second delta rides
/// along: `TransactionMetadataSet` has bumped from 21 to 22 and now appends an
/// `Optional<WitnessTrees>`, which the fixture writes as `None` to keep the
/// transaction section minimal.
fn row_33() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 28);
    push_wallet_capability_v2(&mut out);

    push_compact_size(&mut out, 0); // blocks
    push_u64_le(&mut out, 22); // TransactionMetadataSet, version 22
    push_compact_size(&mut out, 0);
    push_optional_none(&mut out); // witness trees

    push_u64_string(&mut out, CHAIN_NAME);
    push_wallet_options(&mut out);
    push_u64_le(&mut out, BIRTHDAY);
    push_optional_none(&mut out); // verified_tree
    push_price_info(&mut out);
    push_compact_vec_u8(&mut out, &SEED_ENTROPY);

    // The grammar's mark: the mnemonic account index.
    push_u32_le(&mut out, MNEMONIC_ACCOUNT_INDEX);

    out
}

/// Row 34, Defining Commit `fd86965ea` (merge of PR #1414, authored in
/// `173ea7f32`), wallet version 29.
///
/// Replicates `LightWallet::write` in `zingolib/src/wallet/disk.rs` — the
/// writer has moved out of `wallet.rs` into its own module — together with
/// `<WalletCapability as ReadableWriteable<ChainType, ChainType>>::write`,
/// `UnifiedKeyStore::write`, and `<UnifiedSpendingKey as
/// ReadableWriteable>::write` in `zingolib/src/wallet/keys/unified.rs`, and
/// `TxMap::write` in `zingolib/src/wallet/tx_map/read_write.rs`.
///
/// The `Capability` triple is gone. `WalletCapability`'s `VERSION` byte reads
/// 3 and is followed by a `UnifiedKeyStore`, which writes its own version byte
/// (0), a key-type byte (2 for `Spend`), and then the unified spending key as
/// a CompactSize-prefixed opaque blob. The blob is not opaque to the reader,
/// which parses it with `UnifiedSpendingKey::from_bytes(Era::Orchard, ..)`, so
/// the fixture builds a structurally valid one.
fn row_34() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 29);

    // WalletCapability::write, VERSION 3.
    push_u8(&mut out, 3);
    push_unified_key_store_spend(&mut out);
    push_receiver_selection_vector(&mut out);

    push_compact_size(&mut out, 0); // blocks
    push_tx_map(&mut out);

    push_u64_string(&mut out, CHAIN_NAME);
    push_wallet_options(&mut out);
    push_u64_le(&mut out, BIRTHDAY);
    push_optional_none(&mut out); // verified_tree
    push_price_info(&mut out);
    push_compact_vec_u8(&mut out, &SEED_ENTROPY);
    push_u32_le(&mut out, MNEMONIC_ACCOUNT_INDEX);

    out
}

/// Row 35, Defining Commit `eb2210e79` (merge of PR #1445, authored in
/// `285a73cbb`), wallet version 30.
///
/// Replicates `LightWallet::write` in `zingolib/src/wallet/disk.rs` and
/// `WalletCapability::write` in `zingolib/src/wallet/keys/unified.rs`, whose
/// `VERSION` is now 4.
///
/// The sole delta is inside the capability: a u32 count of ephemeral
/// transparent addresses now precedes the key store. The fixture writes 0,
/// because a wallet that has never sent to a TEX address has derived none, so
/// the grammar shows up as the version byte 4 followed by four zero bytes
/// where row 34 put the key store's version byte immediately.
fn row_35() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 30);
    push_wallet_capability_v4(&mut out);

    push_compact_size(&mut out, 0); // blocks
    push_tx_map(&mut out);

    push_u64_string(&mut out, CHAIN_NAME);
    push_wallet_options(&mut out);
    push_u64_le(&mut out, BIRTHDAY);
    push_optional_none(&mut out); // verified_tree
    push_price_info(&mut out);
    push_compact_vec_u8(&mut out, &SEED_ENTROPY);
    push_u32_le(&mut out, MNEMONIC_ACCOUNT_INDEX);

    out
}

/// Row 36, Defining Commit `b82fbe17b` (merge of PR #1630, authored in
/// `0ef038132`), wallet version 31, the first of the two version-31 layouts.
///
/// Replicates `LightWallet::write` in `zingolib/src/wallet/disk.rs`,
/// `WalletCapability::write` in `zingolib/src/wallet/keys/unified.rs` (still
/// `VERSION` 4, its u32 count now naming rejection addresses rather than
/// ephemeral ones), and `TxMap::write` in
/// `zingolib/src/wallet/tx_map/read_write.rs`.
///
/// The sync integration has removed the last-100-blocks vector and the
/// `Optional<TreeState>`, and the birthday is now written from the wallet's
/// own field rather than recomputed. The key store and the transaction set are
/// still written — that is precisely what row 37 drops.
fn row_36() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 31);
    push_wallet_capability_v4(&mut out);
    push_tx_map(&mut out);

    push_u64_string(&mut out, CHAIN_NAME);
    push_wallet_options(&mut out);
    push_u64_le(&mut out, BIRTHDAY);
    push_price_info(&mut out);
    push_compact_vec_u8(&mut out, &SEED_ENTROPY);
    push_u32_le(&mut out, MNEMONIC_ACCOUNT_INDEX);

    out
}

/// Row 37, Defining Commit `db3f7f716` (merge of PR #1648, authored in the
/// `f112f3167` series), wallet version 31 unbumped, the second version-31
/// layout.
///
/// Replicates `LightWallet::write` in `zingolib/src/wallet/disk.rs`, where the
/// key-store and transaction-set calls survive only as commented-out code and
/// the chain name is taken from `self.network`.
///
/// The two version-31 grammars therefore share a version word and nothing
/// else after it: this file's ninth byte begins the chain-name string's u64
/// length, where row 36 puts the capability's version byte. For a real wallet
/// the gap runs to kilobytes; between these two minimal fixtures it is the
/// capability record and the empty transaction set, a few hundred bytes.
fn row_37() -> Vec<u8> {
    let mut out = Vec::new();
    push_u64_le(&mut out, 31);

    push_u64_string(&mut out, CHAIN_NAME);
    push_wallet_options(&mut out);
    push_u64_le(&mut out, BIRTHDAY);
    push_price_info(&mut out);
    push_compact_vec_u8(&mut out, &SEED_ENTROPY);
    push_u32_le(&mut out, MNEMONIC_ACCOUNT_INDEX);

    out
}

/// Append `WalletOptions::write`, unchanged across the era: serialized
/// version 2, the memo-download discriminant, then an `Optional<u32>`
/// transaction-size filter.
fn push_wallet_options(out: &mut Vec<u8>) {
    push_u64_le(out, 2);
    push_u8(out, MEMO_DOWNLOAD_WALLET_MEMOS);
    push_optional_some(out);
    push_u32_le(out, TRANSACTION_SIZE_FILTER);
}

/// Append `WalletZecPriceInfo::write`, unchanged across the era: serialized
/// version 20, an `Optional<u64>` fetch timestamp, then a u64 retry count.
/// Neither the spot price nor the currency is persisted.
fn push_price_info(out: &mut Vec<u8>) {
    push_u64_le(out, 20);
    push_optional_none(out);
    push_u64_le(out, 0);
}

/// Append `TxMap::write` as rows 34 through 36 wrote it: serialized version
/// 22, an empty transaction vector, then `Optional<WitnessTrees>` as `None`.
fn push_tx_map(out: &mut Vec<u8>) {
    push_u64_le(out, 22);
    push_compact_size(out, 0);
    push_optional_none(out);
}

/// Append an Orchard spending key as the era's writers wrote it: 32 raw bytes,
/// with no length prefix and no version byte.
// ASSUMPTION: `orchard::keys::SpendingKey::to_bytes` returns the 32 bytes of
// raw entropy, and `from_bytes` accepts an all-zero blob, rejecting only the
// negligible case where the derived spend authorizing key is zero.
fn push_orchard_spending_key(out: &mut Vec<u8>) {
    push_bytes(out, &[0u8; 32]);
}

/// Append a Sapling extended spending key in the ZIP-32 serialization: depth,
/// parent full-viewing-key tag, child index, chain code, expanded spending
/// key, and diversifier key, 169 bytes in all.
// ASSUMPTION: this 169-byte layout is what
// `zcash_primitives::zip32::ExtendedSpendingKey::write` produced at rows 30
// through 33 and what `sapling_crypto::zip32::ExtendedSpendingKey::to_bytes`
// produces at rows 34 through 36; the two crates agree, and the field order is
// fixed by ZIP 32. The depth of 3 and hardened child index of 0 name the
// account key at `m/32'/133'/0'`.
fn push_sapling_extended_spending_key(out: &mut Vec<u8>) {
    push_u8(out, 3); // depth
    push_bytes(out, &[0u8; 4]); // parent full-viewing-key tag
    push_u32_le(out, 0x8000_0000); // hardened child index 0
    push_bytes(out, &[0u8; 32]); // chain code
    push_bytes(out, &[0u8; 96]); // expanded spending key: ask, nsk, ovk
    push_bytes(out, &[0u8; 32]); // diversifier key
}

/// Append the wallet's own `ExtendedPrivKey`, the pre-`UnifiedKeyStore`
/// transparent key record: a version byte, the raw 32-byte secp256k1 secret,
/// then the chain code as a length-prefixed byte vector.
fn push_legacy_extended_priv_key(out: &mut Vec<u8>) {
    push_u8(out, 1); // ExtendedPrivKey VERSION
    push_bytes(out, &DUMMY_SECP_SECRET_KEY);
    push_compact_vec_u8(out, &[0u8; 32]); // chain code
}

/// Append the capability's trailing vector of per-address receiver
/// selections, holding one address with all three receivers. Each element is
/// a `ReceiverSelection` version byte followed by the receiver bitmask.
fn push_receiver_selection_vector(out: &mut Vec<u8>) {
    push_compact_size(out, 1);
    push_u8(out, 1); // ReceiverSelection VERSION
    push_u8(out, ALL_RECEIVERS);
}

/// Append `WalletCapability::write` at `VERSION` 2, the shape rows 32 and 33
/// share: the version byte, three `Capability` records each written as its own
/// version byte, a variant tag, and a key, then the receiver-selection vector.
/// The variant tag 2 is `Spend`.
fn push_wallet_capability_v2(out: &mut Vec<u8>) {
    push_u8(out, 2); // WalletCapability VERSION

    push_u8(out, 1); // Capability VERSION
    push_u8(out, 2); // Spend
    push_orchard_spending_key(out);

    push_u8(out, 1);
    push_u8(out, 2);
    push_sapling_extended_spending_key(out);

    push_u8(out, 1);
    push_u8(out, 2);
    push_legacy_extended_priv_key(out);

    push_receiver_selection_vector(out);
}

/// Append `WalletCapability::write` at `VERSION` 4, the shape rows 35 and 36
/// share: the version byte, a u32 count of ephemeral (later rejection)
/// transparent addresses, the unified key store, then the receiver-selection
/// vector.
fn push_wallet_capability_v4(out: &mut Vec<u8>) {
    push_u8(out, 4); // WalletCapability VERSION
    push_u32_le(out, 0); // ephemeral transparent addresses derived
    push_unified_key_store_spend(out);
    push_receiver_selection_vector(out);
}

/// Append `UnifiedKeyStore::write` for the `Spend` variant: the store's own
/// version byte (0), the key-type byte (2), then the unified spending key as a
/// CompactSize-prefixed blob.
fn push_unified_key_store_spend(out: &mut Vec<u8>) {
    push_u8(out, 0); // UnifiedKeyStore VERSION
    push_u8(out, 2); // KEY_TYPE_SPEND
    let usk = unified_spending_key_bytes();
    push_compact_size(out, usk.len() as u64);
    push_bytes(out, &usk);
}

/// Build the body of a `UnifiedSpendingKey` as
/// `UnifiedSpendingKey::to_bytes(Era::Orchard)` writes it: the era identifier,
/// then one CompactSize-tagged, CompactSize-framed component per pool in
/// Orchard, Sapling, transparent order.
// ASSUMPTION: the typecodes are ZIP-316's — 3 for Orchard, 2 for Sapling, 0
// for P2PKH — and the era identifier is the NU5 branch id, both read from
// `zcash_keys`'s `UnifiedSpendingKey::to_bytes`.
fn unified_spending_key_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    push_u32_le(&mut out, ERA_ORCHARD_ID);

    push_compact_size(&mut out, 3); // Typecode::Orchard
    push_compact_size(&mut out, 32);
    push_orchard_spending_key(&mut out);

    push_compact_size(&mut out, 2); // Typecode::Sapling
    push_compact_size(&mut out, 169);
    push_sapling_extended_spending_key(&mut out);

    push_compact_size(&mut out, 0); // Typecode::P2pkh
    push_compact_size(&mut out, 74);
    push_account_priv_key(&mut out);

    out
}

/// Append a transparent `AccountPrivKey` in the form `to_bytes` produced at
/// rows 34 through 36: a BIP-32 extended private key with its four-byte
/// version prefix stripped, 74 bytes in all.
// ASSUMPTION: `zcash_primitives` 0.16 and 0.19 both back `AccountPrivKey` with
// the `bip32` crate and serialize it by base58-decoding the `xprv` string and
// dropping the prefix, which yields depth, parent fingerprint, big-endian
// child number, chain code, and a 33-byte key field whose leading zero marks
// it private. The 32 secret bytes must be a valid secp256k1 scalar, so they
// are all ones rather than zeros; depth 3 and hardened child number 0 name the
// account key at `m/44'/133'/0'`.
fn push_account_priv_key(out: &mut Vec<u8>) {
    push_u8(out, 3); // depth
    push_bytes(out, &[0u8; 4]); // parent fingerprint
    push_bytes(out, &0x8000_0000u32.to_be_bytes()); // hardened child number 0
    push_bytes(out, &[0u8; 32]); // chain code
    push_u8(out, 0); // private-key marker
    push_bytes(out, &DUMMY_SECP_SECRET_KEY);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read the file's version word, the u64 every fixture in this era opens
    /// with.
    fn version_word(bytes: &[u8]) -> u64 {
        u64::from_le_bytes(bytes[..8].try_into().expect("fixture has a version word"))
    }

    /// The nine fixtures carry the census's version words, including the two
    /// unbumped reuses: rows 29 and 30 both write 25, and rows 36 and 37 both
    /// write 31.
    #[test]
    fn version_words_match_the_census() {
        let expected = vec![25u64, 25, 26, 27, 28, 29, 30, 31, 31];
        let actual: Vec<u64> = fixtures()
            .iter()
            .map(|fixture| version_word(&fixture.bytes))
            .collect();
        assert_eq!(actual, expected);
    }

    /// The rows arrive in census order under their Defining Commits.
    #[test]
    fn rows_are_ordered_and_labelled() {
        let fixtures = fixtures();
        let rows: Vec<u8> = fixtures.iter().map(|fixture| fixture.row).collect();
        assert_eq!(rows, (29..=37).collect::<Vec<u8>>());
        assert!(fixtures.iter().all(|fixture| fixture.branch == "dev"));
        assert_eq!(fixtures[0].defining_commit, "6b6ed912e");
        assert_eq!(fixtures[8].defining_commit, "db3f7f716");
    }

    /// Row 29's key store is still the legacy `Keys` record at version 22,
    /// which the file writes as a u64 immediately after the version word.
    /// Rows 30 through 36 put a single-byte key-store version there instead.
    #[test]
    fn row_29_writes_the_legacy_keys_version_word() {
        let bytes = row_29();
        assert_eq!(
            u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            22,
            "the legacy Keys record's serialized version"
        );
    }

    /// Row 29's mark is the trailing `(Orchard anchor, height)` vector: a
    /// CompactSize count of one, a zeroed anchor, and the height.
    #[test]
    fn row_29_ends_with_one_orchard_anchor() {
        let bytes = row_29();
        let tail = &bytes[bytes.len() - 37..];
        assert_eq!(tail[0], 1, "one anchor pair");
        assert_eq!(&tail[1..33], &[0u8; 32], "the zeroed anchor");
        assert_eq!(
            u32::from_le_bytes(tail[33..].try_into().unwrap()),
            ANCHOR_HEIGHT
        );
    }

    /// The capability's sub-version byte sits at offset 8 for rows 30 through
    /// 36 and is the era's sharpest discriminator: 1 for the two
    /// `UnifiedSpendCapability` rows, 2 once the `Capability` triple arrives,
    /// 3 once the `UnifiedKeyStore` replaces it, and 4 once the ephemeral
    /// address count precedes it.
    #[test]
    fn capability_sub_version_bytes_track_the_key_store_rewrites() {
        assert_eq!(row_30()[8], 1);
        assert_eq!(row_31()[8], 1);
        assert_eq!(row_32()[8], 2);
        assert_eq!(row_33()[8], 2);
        assert_eq!(row_34()[8], 3);
        assert_eq!(row_35()[8], 4);
        assert_eq!(row_36()[8], 4);
    }

    /// Row 30 alone closes its capability with an `encrypted` flag, so its
    /// capability record is one byte longer than row 31's. Every earlier byte
    /// of the capability is identical, which the shared prefix witnesses.
    #[test]
    fn row_30_capability_carries_the_encrypted_flag_row_31_drops() {
        let thirty = row_30();
        let thirty_one = row_31();
        // Version byte, Orchard key, Sapling key, transparent key, and the
        // receiver-selection vector, all measured from the file version word.
        let capability_end = 8 + 1 + 32 + 169 + 66 + 3;
        assert_eq!(&thirty[8..capability_end], &thirty_one[8..capability_end]);
        assert_eq!(thirty[capability_end], 0, "row 30's encrypted flag");
    }

    /// Row 33's mark is the u32 mnemonic account index appended after the seed
    /// vector. Its file runs five bytes longer than row 32's: four for the
    /// index and one for the `Optional<WitnessTrees>` its transaction set
    /// gained.
    #[test]
    fn row_33_appends_the_mnemonic_account_index() {
        let bytes = row_33();
        assert_eq!(
            u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap()),
            MNEMONIC_ACCOUNT_INDEX
        );
        assert_eq!(bytes.len(), row_32().len() + 5);
    }

    /// Row 35 inserts a u32 ephemeral-address count between the capability's
    /// version byte and the key store, so its file runs four bytes longer than
    /// row 34's and the key store's own version byte moves from offset 9 to
    /// offset 13.
    #[test]
    fn row_35_inserts_the_ephemeral_address_count() {
        let bytes = row_35();
        assert_eq!(
            u32::from_le_bytes(bytes[9..13].try_into().unwrap()),
            0,
            "no ephemeral addresses derived"
        );
        assert_eq!(bytes[13], 0, "the UnifiedKeyStore version byte");
        assert_eq!(row_34()[9], 0, "the UnifiedKeyStore version byte at row 34");
        assert_eq!(bytes.len(), row_34().len() + 4);
    }

    /// The two version-31 grammars share their first eight bytes and diverge
    /// at the ninth: row 36 writes the capability, row 37 writes the chain
    /// name's u64 length. The size gap is the capability plus the empty
    /// transaction set.
    #[test]
    fn the_two_version_31_layouts_differ_by_the_key_store_and_transactions() {
        let thirty_six = row_36();
        let thirty_seven = row_37();
        assert_eq!(&thirty_six[..8], &thirty_seven[..8]);
        assert_eq!(thirty_six[8], 4, "row 36's WalletCapability version");
        assert_eq!(
            u64::from_le_bytes(thirty_seven[8..16].try_into().unwrap()),
            CHAIN_NAME.len() as u64,
            "row 37 begins the chain name where row 36 begins the key store"
        );
        let gap = thirty_six.len() - thirty_seven.len();
        assert!(
            gap > 250,
            "the key store and transaction set account for {gap} bytes"
        );
    }

    /// No two neighbouring rows produce the same bytes, which is the census's
    /// claim that each row is a distinguishable grammar.
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
