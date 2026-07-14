//! Assembles offline wallets with fabricated, spendable-and-proposable
//! funds — no network, no chain, no fixture files.
//!
//! Proposal creation gates on: a known chain height (fully scanned scan
//! ranges), anchor checkpoints in BOTH shard-tree stores, and notes that
//! are confirmed at or below the anchor carrying a position, a nullifier,
//! and no spend. This builder fabricates exactly those invariants.
//!
//! Beyond proposing, the wallets can BUILD transactions offline
//! (`LightWallet::calculate_transactions` — the build-without-broadcast
//! seam of the protection audit's gap remediation plan): each fabricated
//! note is addressed to the wallet's own keys and its commitment is
//! appended to the corresponding shard tree at the note's claimed
//! position, so witness computation and spend proving work without a
//! chain. Sapling proving parameters are embedded in the crate.
//!
//! Example wallet fixture files cannot serve this purpose: they
//! deserialize with keys and tree frontiers but no confirmed transaction
//! state (the root cause of the historically-ignored offline propose
//! test).

use incrementalmerkletree::{Position, Retention};
use orchard::tree::MerkleHashOrchard;
use orchard::value::NoteValue;
use shardtree::store::{Checkpoint, ShardStore as _};
use zcash_keys::keys::UnifiedSpendingKey;

use pepper_sync::sync::{ScanPriority, ScanRange};
use pepper_sync::wallet::{
    IronwoodNote, OrchardNote, OutputId, SaplingNote, SyncState, TransparentCoin, WalletTransaction,
};
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::consensus::Parameters as _;
use zcash_protocol::memo::Memo;
use zcash_protocol::value::Zatoshis;
use zingo_common_components::protocol::ActivationHeights;
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::config::{ChainType, WalletConfig};
use crate::mocks::SaplingCryptoNoteBuilder;
use crate::mocks::nullifier::{OrchardNullifierBuilder, SaplingNullifierBuilder};
use crate::mocks::orchard_note::OrchardCryptoNoteBuilder;
use crate::testutils::default_test_wallet_settings;
use crate::wallet::LightWallet;

/// Builds an offline [`LightWallet`] whose fabricated orchard funds pass
/// every spendability gate, so `propose_send` (and balance/summary logic)
/// work without a network.
pub struct SyntheticWalletBuilder {
    mnemonic: String,
    tip: u32,
    activation_heights: ActivationHeights,
    ironwood_note_values: Vec<u64>,
    orchard_note_values: Vec<u64>,
    sapling_note_values: Vec<u64>,
    transparent_coin_values: Vec<u64>,
}

impl SyntheticWalletBuilder {
    /// A regtest wallet for `mnemonic` with birthday 1. The chain is
    /// considered fully scanned through [`Self::tip`] (default 20).
    pub fn new(mnemonic: &str) -> Self {
        Self {
            mnemonic: mnemonic.to_string(),
            tip: 20,
            activation_heights: ActivationHeights::default(),
            ironwood_note_values: Vec::new(),
            orchard_note_values: Vec::new(),
            sapling_note_values: Vec::new(),
            transparent_coin_values: Vec::new(),
        }
    }

    /// Sets the fully-scanned chain tip. Notes are confirmed at heights
    /// 2, 3, … so the tip must exceed the number of notes by at least 2.
    pub fn tip(mut self, tip: u32) -> Self {
        self.tip = tip;
        self
    }

    /// Overrides the regtest activation-height schedule (default: every
    /// expressible upgrade active at height 1). The gap-4 boundary cells
    /// use this to position an activation just above the synced tip.
    pub fn activation_heights(mut self, heights: ActivationHeights) -> Self {
        self.activation_heights = heights;
        self
    }

    /// Adds a confirmed, unspent, spendable orchard note of `value`.
    pub fn ironwood_note(mut self, value: u64) -> Self {
        self.ironwood_note_values.push(value);
        self
    }

    /// Adds a confirmed, unspent, spendable orchard note of `value`.
    pub fn orchard_note(mut self, value: u64) -> Self {
        self.orchard_note_values.push(value);
        self
    }

    /// Adds a confirmed, unspent, spendable sapling note of `value`.
    pub fn sapling_note(mut self, value: u64) -> Self {
        self.sapling_note_values.push(value);
        self
    }

    /// Adds a confirmed, unspent transparent coin of `value`, received on
    /// the wallet's own first transparent address.
    pub fn transparent_coin(mut self, value: u64) -> Self {
        self.transparent_coin_values.push(value);
        self
    }

    /// Assembles the wallet.
    pub fn build(&self) -> LightWallet {
        assert!(
            self.tip as usize
                > self.ironwood_note_values.len()
                    + self.orchard_note_values.len()
                    + self.sapling_note_values.len()
                    + self.transparent_coin_values.len()
                    + 1,
            "tip must exceed the highest note confirmation height"
        );
        let mut wallet = LightWallet::new(
            ChainType::Regtest(self.activation_heights),
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: self.mnemonic.clone(),
                no_of_accounts: 1.try_into().expect("hard-coded non-zero"),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
        )
        .expect("synthetic wallet construction from mnemonic succeeds");

        // Chain fully scanned from birthday through the tip.
        wallet.sync_state = SyncState::new_for_test(vec![ScanRange::from_parts(
            BlockHeight::from_u32(1)..BlockHeight::from_u32(self.tip + 1),
            ScanPriority::Scanned,
        )]);

        // Notes must be addressed to the wallet's own keys: proposing
        // never checks ownership, but the spend builders refuse (orchard)
        // or mis-prove (sapling) notes whose recipient the spending key
        // does not derive.
        let unified_spending_key: UnifiedSpendingKey = wallet
            .unified_key_store
            .get(&zip32::AccountId::ZERO)
            .expect("a fresh mnemonic wallet carries account zero")
            .try_into()
            .expect("mnemonic wallets carry spend capability");
        let unified_full_viewing_key = unified_spending_key.to_unified_full_viewing_key();

        let mut nullifiers = OrchardNullifierBuilder::new();
        let orchard_recipient = unified_full_viewing_key
            .orchard()
            .expect("unified key carries an orchard fvk")
            .address_at(0u32, zip32::Scope::External);

        for (index, value) in self.ironwood_note_values.iter().enumerate() {
            let txid = TxId::from_bytes([u8::try_from(index).unwrap() + 1; 32]);
            let crypto_note = OrchardCryptoNoteBuilder::default()
                .recipient(orchard_recipient)
                .value(NoteValue::from_raw(*value))
                .build();
            // Give the note's claimed position a real leaf, so witness
            // computation (and thus transaction building) works offline.
            wallet
                .shard_trees
                .ironwood
                .append(
                    MerkleHashOrchard::from_cmx(&crypto_note.commitment().into()),
                    Retention::Marked,
                )
                .expect("appending to the in-memory orchard tree succeeds");
            let note = IronwoodNote::new_for_test(
                OutputId::new(txid, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                crypto_note,
                Memo::Empty,
                Some(Position::from(index as u64)),
            )
            .with_nullifier_for_test(nullifiers.assign_unique_nullifier().build());
            wallet.wallet_transactions.insert(
                txid,
                WalletTransaction::new_for_test_with_ironwood_notes(
                    txid,
                    ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                        2 + u32::try_from(index).unwrap(),
                    )),
                    vec![note],
                    vec![],
                ),
            );
        }
        let mut note_count = self.ironwood_note_values.len();
        for (index, value) in self.orchard_note_values.iter().enumerate() {
            // Txid bytes offset past the ironwood range (which starts at 1)
            // so the two note families never collide in wallet_transactions.
            let txid = TxId::from_bytes([0x40 + u8::try_from(index).unwrap(); 32]);
            let crypto_note = OrchardCryptoNoteBuilder::default()
                .recipient(orchard_recipient)
                .value(NoteValue::from_raw(*value))
                .note_version(orchard::NoteVersion::V2)
                .build();
            // Give the note's claimed position a real leaf, so witness
            // computation (and thus transaction building) works offline.
            wallet
                .shard_trees
                .orchard
                .append(
                    MerkleHashOrchard::from_cmx(&crypto_note.commitment().into()),
                    Retention::Marked,
                )
                .expect("appending to the in-memory orchard tree succeeds");
            let note = OrchardNote::new_for_test(
                OutputId::new(txid, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                crypto_note,
                Memo::Empty,
                Some(Position::from(index as u64)),
            )
            .with_nullifier_for_test(nullifiers.assign_unique_nullifier().build());
            wallet.wallet_transactions.insert(
                txid,
                WalletTransaction::new_for_test_with_orchard_notes(
                    txid,
                    ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                        2 + u32::try_from(note_count + index).unwrap(),
                    )),
                    vec![note],
                    vec![],
                ),
            );
        }

        let mut sapling_nullifiers = SaplingNullifierBuilder::new();
        note_count += self.orchard_note_values.len();
        let sapling_recipient = unified_full_viewing_key
            .sapling()
            .expect("unified key carries a sapling dfvk")
            .default_address()
            .1;
        for (index, value) in self.sapling_note_values.iter().enumerate() {
            // Txid bytes offset past the orchard range so the two note
            // families never collide.
            let txid = TxId::from_bytes([0x80 + u8::try_from(index).unwrap(); 32]);
            let mut crypto_note = SaplingCryptoNoteBuilder::default();
            crypto_note
                .recipient(sapling_recipient)
                .value(sapling_crypto::value::NoteValue::from_raw(*value));
            let crypto_note = crypto_note.build();
            wallet
                .shard_trees
                .sapling
                .append(
                    sapling_crypto::Node::from_cmu(&crypto_note.cmu()),
                    Retention::Marked,
                )
                .expect("appending to the in-memory sapling tree succeeds");
            let note = SaplingNote::new_for_test(
                OutputId::new(txid, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                crypto_note,
                Memo::Empty,
                Some(Position::from(index as u64)),
            )
            .with_nullifier_for_test(sapling_nullifiers.assign_unique_nullifier().build());
            wallet.wallet_transactions.insert(
                txid,
                WalletTransaction::new_for_test(
                    txid,
                    ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                        2 + u32::try_from(note_count + index).unwrap(),
                    )),
                )
                .with_sapling_notes_for_test(vec![note]),
            );
        }

        // The anchor is capped by the sapling tree's newest checkpoint,
        // and note selection requires a checkpoint at the anchor in BOTH
        // stores. Each checkpoint must cover the leaves appended above,
        // or witnesses would come out empty at the anchor.
        let tip = BlockHeight::from_u32(self.tip);
        let checkpoint_covering = |leaf_count: usize| {
            if leaf_count == 0 {
                Checkpoint::tree_empty()
            } else {
                Checkpoint::at_position(Position::from(leaf_count as u64 - 1))
            }
        };
        wallet
            .shard_trees
            .sapling
            .store_mut()
            .add_checkpoint(tip, checkpoint_covering(self.sapling_note_values.len()))
            .expect("infallible on the memory store");
        wallet
            .shard_trees
            .orchard
            .store_mut()
            .add_checkpoint(tip, checkpoint_covering(self.orchard_note_values.len()))
            .expect("infallible on the memory store");
        wallet
            .shard_trees
            .ironwood
            .store_mut()
            .add_checkpoint(tip, checkpoint_covering(self.ironwood_note_values.len()))
            .expect("infallible on the memory store");

        let shielded_note_count = self.ironwood_note_values.len()
            + self.orchard_note_values.len()
            + self.sapling_note_values.len();
        for (index, value) in self.transparent_coin_values.iter().enumerate() {
            // Txid bytes offset past both shielded ranges.
            let txid = TxId::from_bytes([0xC0 + u8::try_from(index).unwrap(); 32]);
            let (key_id, address) = wallet
                .transparent_addresses()
                .iter()
                .next()
                .map(|(key_id, address)| (*key_id, address.clone()))
                .expect("a fresh wallet carries one transparent address");
            let script = zcash_address::ZcashAddress::try_from_encoded(&address)
                .expect("wallet-generated address encodes validly")
                .convert_if_network::<zcash_transparent::address::TransparentAddress>(
                    wallet.chain_type().network_type(),
                )
                .expect("wallet-generated address matches the wallet network")
                .script()
                .into();
            let coin = TransparentCoin::new_for_test(
                OutputId::new(txid, 0),
                key_id,
                address,
                script,
                Zatoshis::from_u64(*value).expect("coin values are valid zatoshis"),
            );
            wallet.wallet_transactions.insert(
                txid,
                WalletTransaction::new_for_test(
                    txid,
                    ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                        2 + u32::try_from(shielded_note_count + index).unwrap(),
                    )),
                )
                .with_transparent_coins_for_test(vec![coin]),
            );
        }

        wallet
    }
}
