//! Format Census rows 56 through 65: the version-32 restructure and its
//! descendants.
//!
//! Row 56 rebuilds the Wallet File around the new sync engine. Everything
//! after the version word is new: the chain name, a seed `Vector`, a
//! four-byte birthday where eight bytes used to stand, the unified key
//! store, the unified- and transparent-address vectors, and the sync
//! engine's own structures. The nine rows that follow refine that layout
//! one delta at a time, and each delta is the row's grammar-unique mark.
//!
//! Every fixture here describes the same wallet: mainnet, one account, a
//! zeroed twenty-four-word seed, a spend-capable key store, one unified
//! address, one transparent address, no blocks and no transactions, one
//! outpoint-map entry, and one scan range. The outpoint entry is present in
//! every row because this era writes the output index as a `u16`; a later
//! era widens it to a `u32`, and the entry is what pins the width.

use super::util::{
    push_bytes, push_compact_size, push_optional_none, push_optional_some, push_u16_le,
    push_u32_le, push_u64_le, push_u64_string, push_u8,
};
use super::Fixture;

/// The chain name. This era writes it through `zingolib`'s historical
/// `utils::write_string`, so the framing is a u64 little-endian byte length
/// followed by the UTF-8 bytes, and `ChainType`'s `Display` renders mainnet
/// as `main`.
const CHAIN: &str = "main";

/// The wallet's seed entropy, zeroed. `bip0039::Mnemonic::into_entropy`
/// yields thirty-two bytes for a twenty-four-word mnemonic, which is what
/// this era's wallets carry.
const SEED_ENTROPY: [u8; 32] = [0u8; 32];

/// The single account these wallets hold. The zip32 account identifier is
/// written as a little-endian u32 wherever it appears.
const ACCOUNT_ID: u32 = 0;

/// The wallet birthday, a plausible mainnet height. Row 56 narrowed this
/// field from u64 to u32.
const BIRTHDAY: u32 = 2_500_000;

/// The address index of the one unified address and the one transparent
/// address.
const ADDRESS_INDEX: u32 = 0;

/// `TransparentScope::External`, written as its enum discriminant.
const SCOPE_EXTERNAL: u8 = 0;

/// The transaction identifier of the outpoint-map key.
const OUTPOINT_TXID: [u8; 32] = [0x11; 32];

/// The output index of the outpoint-map key. This era writes it as a u16.
const OUTPUT_INDEX: u16 = 1;

/// The block height of the outpoint-map value and of the one sync-state
/// scan target.
const LOCATOR_HEIGHT: u32 = 2_500_100;

/// The transaction identifier of the outpoint-map value and of the one
/// sync-state scan target.
const LOCATOR_TXID: [u8; 32] = [0x22; 32];

/// The one scan range's start height.
const SCAN_RANGE_START: u32 = BIRTHDAY;

/// The one scan range's end height.
const SCAN_RANGE_END: u32 = BIRTHDAY + 1_000;

/// `ScanPriority::Historic`, written as `priority as u8`. The discriminants
/// this era's reader accepts run `Ignored`, `Scanned`, `Historic`,
/// `OpenAdjacent`, `FoundNote`, `ChainTip`, `Verify`.
const SCAN_PRIORITY_HISTORIC: u8 = 2;

/// The gap limit of `TransparentAddressDiscovery::minimal()`, which is what
/// `LightWallet` initialises its `SyncConfig` with throughout this era.
const GAP_LIMIT: u8 = 1;

/// `TransparentAddressDiscoveryScopes::default()` packed into the writer's
/// bitmask: external set, internal clear, refund set.
const DISCOVERY_SCOPES: u8 = 0b101;

/// `PerformanceLevel::High`, the default, written as the second byte of the
/// `PerformanceLevel` record.
const PERFORMANCE_LEVEL_HIGH: u8 = 2;

/// The default `min_confirmations`, a `NonZeroU32` of one.
const MIN_CONFIRMATIONS: u32 = 1;

/// Which of the two `PriceList` grammars a row writes.
///
/// Both carry the same inner version byte of zero, which is what makes the
/// pair the census's third live misparse window: a file written in the
/// eight days between the two commits is read today under the later
/// grammar, and the missing `Optional` shifts every field after it.
#[derive(Clone, Copy, PartialEq)]
enum PriceListShape {
    /// The record as row 59 minted it, opening with an `Optional` holding
    /// the CoinCap API key.
    WithApiKey,
    /// The record from row 60 on, with that `Optional` gone.
    WithoutApiKey,
}

/// Which grammar each row's writer produces. The fields name the deltas the
/// census records for rows 56 through 65, so a row function reads as a
/// statement of what its Defining Commit changed.
struct Shape {
    /// The version word at offset zero.
    version: u64,
    /// Whether the mnemonic's account index follows the seed vector. Row 61
    /// dropped it.
    mnemonic_account_index: bool,
    /// Whether the key store is a `Vector<(account, UnifiedKeyStore)>`
    /// rather than a single bare record. Row 61 made the change.
    account_keyed_key_store: bool,
    /// The `ReceiverSelection` inner version byte. Row 62 moved it to two.
    receiver_version: u8,
    /// The `ReceiverSelection` bitmask. Row 62 retired the transparent bit.
    receiver_mask: u8,
    /// Whether the outpoint map's values and the sync state's scan targets
    /// are `ScanTarget` records rather than bare `(height, txid)` locators.
    /// Row 64 made the change.
    scan_target_values: bool,
    /// The `SyncState` inner version byte, which row 64 moved to one.
    sync_state_version: u8,
    /// The `SyncConfig` record's inner version, when the row writes the
    /// record at all. Row 58 appended it; row 65 moved it to one.
    sync_config_version: Option<u8>,
    /// Whether `min_confirmations` follows the sync config. Row 65 appended
    /// it.
    min_confirmations: bool,
    /// The `PriceList` record that closes the file, when the row writes one
    /// at all. Row 59 appended it; row 60 dropped its leading `Optional`.
    price_list: Option<PriceListShape>,
    /// Whether the vestigial `WalletOptions` and `WalletZecPriceInfo`
    /// records close the file. Only row 56 writes them; row 57 dropped them.
    vestigial_tail: bool,
}

/// The `UnifiedSpendingKey` blob, zeroed.
///
/// The wallet writer treats this blob as opaque: it writes a CompactSize
/// length and then the bytes `UnifiedSpendingKey::to_bytes(Era::Orchard)`
/// returned. Only the dependency parses the interior, so the fixture
/// reproduces the container framing and zeroes the key material.
fn unified_spending_key_blob() -> Vec<u8> {
    let mut out = Vec::new();
    // ASSUMPTION: `zcash_keys`' USK encoding writes the era identifier as a
    // little-endian u32 holding the NU5 consensus branch id, then one
    // (typecode, length, key) triple per pool with the typecode and the
    // length each a CompactSize. The pools appear in the order orchard,
    // sapling, transparent, and the typecodes are 3, 2 and 0. Read from the
    // pinned `zingolabs/librustzcash` checkout of `zcash_keys/src/keys.rs`,
    // whose `to_bytes` is unchanged across this era.
    push_bytes(&mut out, &0xc2d6_d0b4u32.to_le_bytes());
    // ASSUMPTION: the orchard spending key is thirty-two bytes.
    push_compact_size(&mut out, 3);
    push_compact_size(&mut out, 32);
    push_bytes(&mut out, &[0u8; 32]);
    // ASSUMPTION: `sapling_crypto`'s `ExtendedSpendingKey::to_bytes`
    // returns exactly 169 bytes (depth, parent tag, child index, chain
    // code, expanded spending key, diversifier key).
    push_compact_size(&mut out, 2);
    push_compact_size(&mut out, 169);
    push_bytes(&mut out, &[0u8; 169]);
    // ASSUMPTION: `AccountPrivKey::to_bytes` returns the BIP 32 extended
    // private key encoding without its four prefix bytes, so 74 of the 78
    // bytes.
    push_compact_size(&mut out, 0);
    push_compact_size(&mut out, 74);
    push_bytes(&mut out, &[0u8; 74]);
    out
}

/// Append one `UnifiedKeyStore` record holding a spending key: the trait's
/// version byte, the `KEY_TYPE_SPEND` tag, and the CompactSize-framed
/// spending-key blob.
fn push_unified_key_store(out: &mut Vec<u8>) {
    push_u8(out, 0);
    push_u8(out, 2);
    let blob = unified_spending_key_blob();
    push_compact_size(out, blob.len() as u64);
    push_bytes(out, &blob);
}

/// Append one `ScanTarget` record: its version byte, the block height, the
/// transaction identifier, and the narrow-scan-area flag row 64 introduced.
fn push_scan_target(out: &mut Vec<u8>) {
    push_u8(out, 0);
    push_u32_le(out, LOCATOR_HEIGHT);
    push_bytes(out, &LOCATOR_TXID);
    push_u8(out, 1);
}

/// Append the empty `NullifierMap`: its version byte and two empty vectors,
/// sapling then orchard.
fn push_nullifier_map(out: &mut Vec<u8>) {
    push_u8(out, 0);
    push_compact_size(out, 0);
    push_compact_size(out, 0);
}

/// Append the empty `ShardTrees`: its version byte, then the sapling and
/// orchard trees, each an empty shard vector, an empty checkpoint vector
/// and a cap.
fn push_shard_trees(out: &mut Vec<u8>) {
    push_u8(out, 0);
    for _ in 0..2 {
        push_compact_size(out, 0);
        push_compact_size(out, 0);
        // ASSUMPTION: `zcash_client_backend`'s `write_shard` writes a
        // serialization-version byte of 1 followed by the tree, and an
        // empty store's cap is a single `Nil` node whose tag is 0.
        push_u8(out, 1);
        push_u8(out, 0);
    }
}

/// Append the `SyncState`: its version byte, the scan ranges, the sapling
/// and orchard shard ranges, and finally the locators, which row 64 turned
/// into scan targets.
fn push_sync_state(out: &mut Vec<u8>, shape: &Shape) {
    push_u8(out, shape.sync_state_version);
    push_compact_size(out, 1);
    push_u32_le(out, SCAN_RANGE_START);
    push_u32_le(out, SCAN_RANGE_END);
    push_u8(out, SCAN_PRIORITY_HISTORIC);
    push_compact_size(out, 0);
    push_compact_size(out, 0);
    push_compact_size(out, 1);
    if shape.scan_target_values {
        push_scan_target(out);
    } else {
        push_u32_le(out, LOCATOR_HEIGHT);
        push_bytes(out, &LOCATOR_TXID);
    }
}

/// Append the `SyncConfig` record: its version byte, the transparent
/// address-discovery gap limit and scope bitmask, and, from version one on,
/// a `PerformanceLevel` record.
fn push_sync_config(out: &mut Vec<u8>, version: u8) {
    push_u8(out, version);
    push_u8(out, GAP_LIMIT);
    push_u8(out, DISCOVERY_SCOPES);
    if version >= 1 {
        push_u8(out, 0);
        push_u8(out, PERFORMANCE_LEVEL_HIGH);
    }
}

/// Append the `PriceList` record for a wallet that has never fetched a
/// price: its version byte, the API key `Optional` while the record still
/// carried one, then no update time, no current price and no daily prices.
fn push_price_list(out: &mut Vec<u8>, shape: PriceListShape) {
    push_u8(out, 0);
    if shape == PriceListShape::WithApiKey {
        push_optional_none(out);
    }
    push_optional_none(out);
    push_optional_none(out);
    push_compact_size(out, 0);
}

/// Append the vestigial `WalletOptions` record at its defaults: version
/// two, `MemoDownloadOption::WalletMemos`, and a transaction size filter of
/// five hundred.
fn push_wallet_options(out: &mut Vec<u8>) {
    push_u64_le(out, 2);
    push_u8(out, 1);
    push_optional_some(out);
    push_u32_le(out, 500);
}

/// Append the vestigial `WalletZecPriceInfo` record at its defaults:
/// version twenty, no fetch time, and a zero retry count.
fn push_zec_price_info(out: &mut Vec<u8>) {
    push_u64_le(out, 20);
    push_optional_none(out);
    push_u64_le(out, 0);
}

/// Build one Wallet File in the shape the given row's writer produces. The
/// field order follows `LightWallet::write` exactly: version word, chain
/// name, seed, birthday, key store, unified addresses, transparent
/// addresses, blocks, transactions, nullifier map, outpoint map, shard
/// trees, sync state, and then whichever tail records the row appends.
fn wallet(shape: &Shape) -> Vec<u8> {
    let mut out = Vec::new();

    push_u64_le(&mut out, shape.version);
    push_u64_string(&mut out, CHAIN);

    push_compact_size(&mut out, SEED_ENTROPY.len() as u64);
    push_bytes(&mut out, &SEED_ENTROPY);
    if shape.mnemonic_account_index {
        push_u32_le(&mut out, ACCOUNT_ID);
    }
    push_u32_le(&mut out, BIRTHDAY);

    if shape.account_keyed_key_store {
        push_compact_size(&mut out, 1);
        push_u32_le(&mut out, ACCOUNT_ID);
    }
    push_unified_key_store(&mut out);

    push_compact_size(&mut out, 1);
    push_u32_le(&mut out, ACCOUNT_ID);
    push_u32_le(&mut out, ADDRESS_INDEX);
    push_u8(&mut out, shape.receiver_version);
    push_u8(&mut out, shape.receiver_mask);

    push_compact_size(&mut out, 1);
    push_u32_le(&mut out, ACCOUNT_ID);
    push_u8(&mut out, SCOPE_EXTERNAL);
    push_u32_le(&mut out, ADDRESS_INDEX);

    push_compact_size(&mut out, 0);
    push_compact_size(&mut out, 0);

    push_nullifier_map(&mut out);

    push_compact_size(&mut out, 1);
    push_bytes(&mut out, &OUTPOINT_TXID);
    push_u16_le(&mut out, OUTPUT_INDEX);
    if shape.scan_target_values {
        push_scan_target(&mut out);
    } else {
        push_u32_le(&mut out, LOCATOR_HEIGHT);
        push_bytes(&mut out, &LOCATOR_TXID);
    }

    push_shard_trees(&mut out);
    push_sync_state(&mut out, shape);

    if let Some(version) = shape.sync_config_version {
        push_sync_config(&mut out, version);
    }
    if shape.min_confirmations {
        push_u32_le(&mut out, MIN_CONFIRMATIONS);
    }
    if let Some(price_list) = shape.price_list {
        push_price_list(&mut out, price_list);
    }
    if shape.vestigial_tail {
        push_wallet_options(&mut out);
        push_zec_price_info(&mut out);
    }

    out
}

/// This era's fixtures, census rows 56 through 65 in order.
pub fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            row: 56,
            defining_commit: "44e6271cb",
            branch: "dev",
            bytes: row_56(),
        },
        Fixture {
            row: 57,
            defining_commit: "8aaae992a",
            branch: "dev",
            bytes: row_57(),
        },
        Fixture {
            row: 58,
            defining_commit: "82c61c0d3",
            branch: "dev",
            bytes: row_58(),
        },
        Fixture {
            row: 59,
            defining_commit: "1ef03610b",
            branch: "dev",
            bytes: row_59(),
        },
        Fixture {
            row: 60,
            defining_commit: "44baa11b4",
            branch: "dev",
            bytes: row_60(),
        },
        Fixture {
            row: 61,
            defining_commit: "ccc1d681a",
            branch: "dev",
            bytes: row_61(),
        },
        Fixture {
            row: 62,
            defining_commit: "e5e4a349f",
            branch: "dev",
            bytes: row_62(),
        },
        Fixture {
            row: 63,
            defining_commit: "e6b02b0d8",
            branch: "dev",
            bytes: row_63(),
        },
        Fixture {
            row: 64,
            defining_commit: "eae34880e",
            branch: "dev",
            bytes: row_64(),
        },
        Fixture {
            row: 65,
            defining_commit: "ad6ded426",
            branch: "dev",
            bytes: row_65(),
        },
    ]
}

/// Row 56, Defining Commit `44e6271cb`, version 32.
///
/// Replicates `LightWallet::write` in `zingolib/src/wallet/disk.rs`,
/// together with `utils::write_string`, `UnifiedKeyStore::write` and
/// `ReceiverSelection::write` in `zingolib/src/wallet/keys/unified.rs`, the
/// `NullifierMap`, `ShardTrees` and `SyncState` writers in
/// `pepper-sync/src/wallet/serialization.rs`, and the `WalletOptions` and
/// `WalletZecPriceInfo` writers in `zingolib/src/wallet.rs` and
/// `zingolib/src/wallet/data.rs`.
///
/// The wallet is a mainnet wallet with a zeroed twenty-four-word seed whose
/// mnemonic account index follows the seed vector, a birthday of 2,500,000
/// written in four bytes rather than the eight the previous grammar used, a
/// single spend-capable key store, one unified address, one transparent
/// address, no blocks, no transactions, an empty nullifier map, one
/// outpoint-map entry, empty shard trees, and a sync state holding one
/// historic scan range and one locator. The vestigial `WalletOptions` and
/// `WalletZecPriceInfo` records close the file; the version-32 reader never
/// consumes them, and row 57 removes them.
fn row_56() -> Vec<u8> {
    wallet(&Shape {
        version: 32,
        mnemonic_account_index: true,
        account_keyed_key_store: false,
        receiver_version: 1,
        receiver_mask: 0b111,
        scan_target_values: false,
        sync_state_version: 0,
        sync_config_version: None,
        min_confirmations: false,
        price_list: None,
        vestigial_tail: true,
    })
}

/// Row 57, Defining Commit `8aaae992a`, version 32 unbumped.
///
/// Replicates the same writers as row 56. The wallet's contents are
/// identical; only the writer changed, dropping the trailing
/// `WalletOptions` and `WalletZecPriceInfo` records that the version-32
/// reader never consumed. Two files therefore claim version 32 and differ
/// by a thirty-one-byte tail, which is the row's mark.
fn row_57() -> Vec<u8> {
    wallet(&Shape {
        version: 32,
        mnemonic_account_index: true,
        account_keyed_key_store: false,
        receiver_version: 1,
        receiver_mask: 0b111,
        scan_target_values: false,
        sync_state_version: 0,
        sync_config_version: None,
        min_confirmations: false,
        price_list: None,
        vestigial_tail: false,
    })
}

/// Row 58, Defining Commit `82c61c0d3`, version 33.
///
/// Replicates row 57's writers plus `SyncConfig::write` in
/// `pepper-sync/src/sync.rs`. The wallet's contents are row 57's, and the
/// sync config is the one `LightWallet` initialises: version zero, the gap
/// limit of one that `TransparentAddressDiscovery::minimal` sets, and the
/// default scope bitmask with external and refund set.
fn row_58() -> Vec<u8> {
    wallet(&Shape {
        version: 33,
        mnemonic_account_index: true,
        account_keyed_key_store: false,
        receiver_version: 1,
        receiver_mask: 0b111,
        scan_target_values: false,
        sync_state_version: 0,
        sync_config_version: Some(0),
        min_confirmations: false,
        price_list: None,
        vestigial_tail: false,
    })
}

/// Row 59, Defining Commit `1ef03610b`, version 34.
///
/// Replicates row 58's writers plus `PriceList::write` in
/// `zingo-price/src/lib.rs`. The wallet's contents are row 58's, and the
/// price list is the one `PriceList::new` builds: version zero, then four
/// fields, of which the first is the `Optional` holding the CoinCap API key
/// that row 60 removes. All four are empty here, since the wallet has never
/// fetched a price and no key has been set.
fn row_59() -> Vec<u8> {
    wallet(&Shape {
        version: 34,
        mnemonic_account_index: true,
        account_keyed_key_store: false,
        receiver_version: 1,
        receiver_mask: 0b111,
        scan_target_values: false,
        sync_state_version: 0,
        sync_config_version: Some(0),
        min_confirmations: false,
        price_list: Some(PriceListShape::WithApiKey),
        vestigial_tail: false,
    })
}

/// Row 60, Defining Commit `44baa11b4`, version 34 unbumped.
///
/// Replicates row 59's writers with `PriceList::write` in
/// `zingo-price/src/lib.rs` rewritten for the Tor-fronted Gemini price
/// source, which needs no API key. The leading `Optional` disappears while
/// `PriceList::serialized_version` stands still at zero, so the record's
/// own version byte cannot tell the two grammars apart. Comparing
/// `zingolib/src/wallet/disk.rs` at `1ef03610b` and `44baa11b4` shows the
/// wallet writer itself untouched, so the fixture is row 59's bytes one
/// `Optional` shorter.
///
/// This is the third of the census's live misparse windows: a file written
/// in the eight days between the two commits is read today under this
/// grammar, and the missing byte shifts the update time, the current price
/// and the daily-price vector.
fn row_60() -> Vec<u8> {
    wallet(&Shape {
        version: 34,
        mnemonic_account_index: true,
        account_keyed_key_store: false,
        receiver_version: 1,
        receiver_mask: 0b111,
        scan_target_values: false,
        sync_state_version: 0,
        sync_config_version: Some(0),
        min_confirmations: false,
        price_list: Some(PriceListShape::WithoutApiKey),
        vestigial_tail: false,
    })
}

/// Row 61, Defining Commit `ccc1d681a`, version 35.
///
/// Replicates row 60's writers with `LightWallet::write` restructured for
/// multiple accounts. The single `UnifiedKeyStore` record becomes a
/// `Vector<(account, UnifiedKeyStore)>`, and the mnemonic account index
/// that used to follow the seed vector is gone. The wallet still holds one
/// account, so the vector carries one entry keyed by account zero, and the
/// birthday now follows the seed vector directly.
fn row_61() -> Vec<u8> {
    wallet(&Shape {
        version: 35,
        mnemonic_account_index: false,
        account_keyed_key_store: true,
        receiver_version: 1,
        receiver_mask: 0b111,
        scan_target_values: false,
        sync_state_version: 0,
        sync_config_version: Some(0),
        min_confirmations: false,
        price_list: Some(PriceListShape::WithoutApiKey),
        vestigial_tail: false,
    })
}

/// Row 62, Defining Commit `e5e4a349f`, version 35 unbumped.
///
/// Replicates row 61's writers with `ReceiverSelection` in
/// `zingolib/src/wallet/keys/unified.rs` moved to inner version two, which
/// retires the transparent bit from the receiver bitmask. The wallet's
/// contents are row 61's, so its one unified address now writes the version
/// byte two and a bitmask of orchard and sapling alone. Two files therefore
/// claim version 35 and differ inside the unified-address vector.
fn row_62() -> Vec<u8> {
    wallet(&Shape {
        version: 35,
        mnemonic_account_index: false,
        account_keyed_key_store: true,
        receiver_version: 2,
        receiver_mask: 0b11,
        scan_target_values: false,
        sync_state_version: 0,
        sync_config_version: Some(0),
        min_confirmations: false,
        price_list: Some(PriceListShape::WithoutApiKey),
        vestigial_tail: false,
    })
}

/// Row 63, Defining Commit `e6b02b0d8`, version 36.
///
/// Replicates row 62's writers unchanged. Comparing
/// `zingolib/src/wallet/disk.rs` at the two commits shows the writer
/// untouched: the commit moves the version word from 35 to 36 and teaches
/// the reader to regenerate addresses for anything older. The fixture is
/// therefore row 62's bytes with a different version word, and nothing
/// else distinguishes the two grammars.
fn row_63() -> Vec<u8> {
    wallet(&Shape {
        version: 36,
        mnemonic_account_index: false,
        account_keyed_key_store: true,
        receiver_version: 2,
        receiver_mask: 0b11,
        scan_target_values: false,
        sync_state_version: 0,
        sync_config_version: Some(0),
        min_confirmations: false,
        price_list: Some(PriceListShape::WithoutApiKey),
        vestigial_tail: false,
    })
}

/// Row 64, Defining Commit `eae34880e`, version 37.
///
/// Replicates row 63's writers with the locator re-encoded as a
/// `ScanTarget` record, whose writer joins `SyncState`'s in
/// `pepper-sync/src/wallet/serialization.rs`. The outpoint map's value and
/// the sync state's fourth vector both carried a bare block height and
/// transaction identifier; each now carries a versioned record that appends
/// a narrow-scan-area flag, and `SyncState`'s own inner version moves from
/// zero to one so its reader can tell the two encodings apart. The wallet's
/// contents are row 63's, and its one outpoint entry and one scan target
/// both show the new encoding with the flag set.
fn row_64() -> Vec<u8> {
    wallet(&Shape {
        version: 37,
        mnemonic_account_index: false,
        account_keyed_key_store: true,
        receiver_version: 2,
        receiver_mask: 0b11,
        scan_target_values: true,
        sync_state_version: 1,
        sync_config_version: Some(0),
        min_confirmations: false,
        price_list: Some(PriceListShape::WithoutApiKey),
        vestigial_tail: false,
    })
}

/// Row 65, Defining Commit `ad6ded426`, version 38.
///
/// Replicates row 64's writers with two additions. `LightWallet::write`
/// appends `min_confirmations` as a u32 between the sync config and the
/// price list, and `SyncConfig::write` in `pepper-sync/src/sync.rs` moves
/// to inner version one, which appends a `PerformanceLevel` record. The
/// wallet's contents are row 64's, with the default `min_confirmations` of
/// one and the default performance level of high.
fn row_65() -> Vec<u8> {
    wallet(&Shape {
        version: 38,
        mnemonic_account_index: false,
        account_keyed_key_store: true,
        receiver_version: 2,
        receiver_mask: 0b11,
        scan_target_values: true,
        sync_state_version: 1,
        sync_config_version: Some(1),
        min_confirmations: true,
        price_list: Some(PriceListShape::WithoutApiKey),
        vestigial_tail: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The version word each row's writer emits, in row order.
    const VERSION_WORDS: [u64; 10] = [32, 32, 33, 34, 34, 35, 35, 36, 37, 38];

    fn version_word(bytes: &[u8]) -> u64 {
        u64::from_le_bytes(bytes[0..8].try_into().expect("the header is eight bytes"))
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// The offset of the byte that follows the seed vector: the version
    /// word, the chain string, the seed's CompactSize count, and the seed.
    const AFTER_SEED: usize = 8 + 8 + CHAIN.len() + 1 + SEED_ENTROPY.len();

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("four bytes are in range"),
        )
    }

    #[test]
    fn every_row_writes_its_census_version_word_at_offset_zero() {
        for (fixture, expected) in fixtures().iter().zip(VERSION_WORDS) {
            assert_eq!(
                version_word(&fixture.bytes),
                expected,
                "row {} wrote the wrong version word",
                fixture.row
            );
        }
    }

    #[test]
    fn every_row_is_a_mainnet_wallet_whose_chain_name_is_a_u64_framed_string() {
        for fixture in fixtures() {
            assert_eq!(
                &fixture.bytes[8..20],
                &[4, 0, 0, 0, 0, 0, 0, 0, b'm', b'a', b'i', b'n'],
                "row {} framed the chain name differently",
                fixture.row
            );
        }
    }

    /// Row 56's headline mark: the birthday is four bytes, not the eight
    /// the preceding grammar wrote. In rows 56 through 60 the mnemonic
    /// account index stands between the seed and the birthday.
    #[test]
    fn row_56_writes_the_birthday_in_four_bytes_after_the_account_index() {
        let bytes = row_56();
        assert_eq!(read_u32(&bytes, AFTER_SEED), ACCOUNT_ID);
        assert_eq!(read_u32(&bytes, AFTER_SEED + 4), BIRTHDAY);
    }

    /// Row 57 drops the vestigial `WalletOptions` (fourteen bytes) and
    /// `WalletZecPriceInfo` (seventeen bytes) records, and changes nothing
    /// else.
    #[test]
    fn row_57_drops_the_vestigial_tail_and_leaves_the_rest_untouched() {
        let (long, short) = (row_56(), row_57());
        assert_eq!(long.len() - short.len(), 31);
        assert_eq!(&long[..short.len()], &short[..]);
    }

    /// Row 58 appends the three-byte `SyncConfig` record.
    #[test]
    fn row_58_appends_the_sync_config_record() {
        let (before, after) = (row_57(), row_58());
        assert_eq!(after.len() - before.len(), 3);
        assert_eq!(&after[after.len() - 3..], &[0, GAP_LIMIT, DISCOVERY_SCOPES]);
    }

    /// Row 59 appends the five-byte `PriceList` record of a wallet that has
    /// never fetched a price: a version byte, the API key `Optional`, the
    /// update-time and current-price `Optional`s, and an empty vector.
    #[test]
    fn row_59_appends_the_price_list_record_with_its_api_key_optional() {
        let (before, after) = (row_58(), row_59());
        assert_eq!(after.len() - before.len(), 5);
        assert_eq!(&after[after.len() - 5..], &[0, 0, 0, 0, 0]);
    }

    /// Row 60's mark: the price list loses its leading `Optional` while its
    /// inner version byte stands still at zero, so the two grammars differ
    /// by exactly one byte at the end of the file and the record's own
    /// version cannot separate them.
    #[test]
    fn row_60_drops_the_api_key_optional_without_bumping_the_inner_version() {
        let (before, after) = (row_59(), row_60());
        assert_eq!(before.len() - after.len(), 1);
        assert_eq!(version_word(&before), version_word(&after));
        // Everything up to the price list is common, and the price list's
        // own version byte is zero in both, so the record cannot be told
        // from its version.
        let shared = after.len() - 4;
        assert_eq!(&before[..shared], &after[..shared]);
        assert_eq!(after[shared], 0);
        assert_eq!(before[shared], 0);
    }

    /// Only row 59 writes the API key `Optional`. Each price-list row's
    /// file must end in exactly the records that follow the sync state, so
    /// the price list's length is pinned by what precedes it rather than
    /// counted in isolation: the sync config, then `min_confirmations` on
    /// row 65, then the price list.
    #[test]
    fn only_row_59_writes_the_api_key_optional() {
        const SYNC_CONFIG_V0: [u8; 3] = [0, GAP_LIMIT, DISCOVERY_SCOPES];
        const SYNC_CONFIG_V1: [u8; 5] = [1, GAP_LIMIT, DISCOVERY_SCOPES, 0, PERFORMANCE_LEVEL_HIGH];
        const MIN_CONFIRMATIONS_BYTES: [u8; 4] = [1, 0, 0, 0];
        const PRICE_LIST_WITH_API_KEY: [u8; 5] = [0, 0, 0, 0, 0];
        const PRICE_LIST_WITHOUT_API_KEY: [u8; 4] = [0, 0, 0, 0];

        for fixture in fixtures() {
            let mut expected_tail = Vec::new();
            match fixture.row {
                56..=58 => continue,
                59 => {
                    expected_tail.extend_from_slice(&SYNC_CONFIG_V0);
                    expected_tail.extend_from_slice(&PRICE_LIST_WITH_API_KEY);
                }
                65 => {
                    expected_tail.extend_from_slice(&SYNC_CONFIG_V1);
                    expected_tail.extend_from_slice(&MIN_CONFIRMATIONS_BYTES);
                    expected_tail.extend_from_slice(&PRICE_LIST_WITHOUT_API_KEY);
                }
                _ => {
                    expected_tail.extend_from_slice(&SYNC_CONFIG_V0);
                    expected_tail.extend_from_slice(&PRICE_LIST_WITHOUT_API_KEY);
                }
            }
            assert_eq!(
                &fixture.bytes[fixture.bytes.len() - expected_tail.len()..],
                &expected_tail[..],
                "row {} wrote the wrong price list tail",
                fixture.row
            );
        }
    }

    /// Row 61 drops the four-byte mnemonic account index and keys the key
    /// store by account, so the birthday now follows the seed directly.
    #[test]
    fn row_61_keys_the_key_store_by_account_and_drops_the_mnemonic_index() {
        let bytes = row_61();
        assert_eq!(read_u32(&bytes, AFTER_SEED), BIRTHDAY);
        // The vector count of one, then the account identifier, then the
        // key store's own version and spend-type bytes.
        assert_eq!(
            &bytes[AFTER_SEED + 4..AFTER_SEED + 11],
            &[1, 0, 0, 0, 0, 0, 2]
        );
    }

    /// Row 62 moves the `ReceiverSelection` inner version byte to two and
    /// retires the transparent bit, while row 61 still writes version one
    /// with all three bits set.
    #[test]
    fn row_62_writes_receiver_selection_version_two_without_the_transparent_bit() {
        let unified_address = [0u8; 8];
        let mut row_61_record = unified_address.to_vec();
        row_61_record.extend_from_slice(&[1, 0b111]);
        let mut row_62_record = unified_address.to_vec();
        row_62_record.extend_from_slice(&[2, 0b11]);

        assert!(contains(&row_61(), &row_61_record));
        assert!(!contains(&row_61(), &row_62_record));
        assert!(contains(&row_62(), &row_62_record));
        assert!(!contains(&row_62(), &row_61_record));
    }

    /// Rows 62 and 63 differ only in the version word: comparing
    /// `zingolib/src/wallet/disk.rs` at `e5e4a349f` and `e6b02b0d8` shows
    /// the writer untouched.
    #[test]
    fn rows_62_and_63_differ_only_in_the_version_word() {
        let (older, newer) = (row_62(), row_63());
        assert_eq!(version_word(&older), 35);
        assert_eq!(version_word(&newer), 36);
        assert_eq!(&older[8..], &newer[8..]);
    }

    /// Row 64 re-encodes the outpoint map's value and the sync state's
    /// fourth vector as `ScanTarget` records, each of which adds a version
    /// byte and a narrow-scan-area flag to the two bytes the bare locator
    /// wrote, and moves `SyncState`'s inner version to one.
    #[test]
    fn row_64_re_encodes_locators_as_scan_targets() {
        let (before, after) = (row_63(), row_64());
        assert_eq!(after.len() - before.len(), 4);

        let mut scan_target = vec![0u8];
        scan_target.extend_from_slice(&LOCATOR_HEIGHT.to_le_bytes());
        scan_target.extend_from_slice(&LOCATOR_TXID);
        scan_target.push(1);
        assert!(contains(&after, &scan_target));
        assert!(!contains(&before, &scan_target));
    }

    /// The whole era writes the outpoint map's output index as a u16, so
    /// every fixture holds the outpoint key's transaction identifier
    /// followed by exactly two index bytes.
    #[test]
    fn every_row_writes_the_outpoint_output_index_as_a_u16() {
        let mut key = OUTPOINT_TXID.to_vec();
        key.extend_from_slice(&OUTPUT_INDEX.to_le_bytes());
        // The byte that follows the index starts the value: a locator's
        // block height in rows 56 through 63, a `ScanTarget` version byte
        // in rows 64 and 65. A u32 index would put a zero byte there in
        // both cases, so pin the value's first byte too.
        let mut locator_value = key.clone();
        locator_value.extend_from_slice(&LOCATOR_HEIGHT.to_le_bytes()[..1]);
        let mut scan_target_value = key.clone();
        scan_target_value.push(0);

        for fixture in fixtures() {
            assert!(
                contains(&fixture.bytes, &locator_value)
                    || contains(&fixture.bytes, &scan_target_value),
                "row {} did not write a u16 output index",
                fixture.row
            );
        }
    }

    /// Row 65 appends the four-byte `min_confirmations` between the sync
    /// config and the price list, and grows the sync config by the two-byte
    /// `PerformanceLevel` record.
    #[test]
    fn row_65_appends_min_confirmations_and_a_performance_level() {
        let (before, after) = (row_64(), row_65());
        assert_eq!(after.len() - before.len(), 6);

        let sync_config_and_confirmations = [
            1,
            GAP_LIMIT,
            DISCOVERY_SCOPES,
            0,
            PERFORMANCE_LEVEL_HIGH,
            MIN_CONFIRMATIONS as u8,
            0,
            0,
            0,
        ];
        assert!(contains(&after, &sync_config_and_confirmations));
    }

    /// Neighbouring rows are the pairs a recognizer is most likely to
    /// confuse, so each adjacent pair must differ.
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

    /// The era contributes rows 56 through 65 in order, all minted on dev.
    #[test]
    fn the_era_covers_rows_56_through_65_on_dev() {
        let fixtures = fixtures();
        let rows: Vec<u8> = fixtures.iter().map(|f| f.row).collect();
        assert_eq!(rows, (56..=65).collect::<Vec<u8>>());
        assert!(fixtures.iter().all(|f| f.branch == "dev"));
    }
}
