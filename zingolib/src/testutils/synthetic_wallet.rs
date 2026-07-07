//! Assembles offline wallets with fabricated, spendable-and-proposable
//! funds — no network, no chain, no fixture files.
//!
//! Proposal creation gates on: a known chain height (fully scanned scan
//! ranges), anchor checkpoints in BOTH shard-tree stores, and notes that
//! are confirmed at or below the anchor carrying a position, a nullifier,
//! and no spend. This builder fabricates exactly those invariants.
//! Witnesses are never computed at proposal time, so the shard trees can
//! stay empty — only their checkpoint stores matter.
//!
//! Example wallet fixture files cannot serve this purpose: they
//! deserialize with keys and tree frontiers but no confirmed transaction
//! state (the root cause of the historically-ignored offline propose
//! test).

use incrementalmerkletree::Position;
use orchard::value::NoteValue;
use shardtree::store::{Checkpoint, ShardStore as _};

use pepper_sync::sync::{ScanPriority, ScanRange};
use pepper_sync::wallet::{
    OrchardNote, OutputId, SaplingNote, SyncState, TransparentCoin, WalletTransaction,
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
                > self.orchard_note_values.len()
                    + self.sapling_note_values.len()
                    + self.transparent_coin_values.len()
                    + 1,
            "tip must exceed the highest note confirmation height"
        );
        let mut wallet = LightWallet::new(
            ChainType::Regtest(ActivationHeights::default()),
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

        // The anchor is capped by the sapling tree's newest checkpoint,
        // and note selection requires a checkpoint at the anchor in BOTH
        // stores.
        let tip = BlockHeight::from_u32(self.tip);
        wallet
            .shard_trees
            .sapling
            .store_mut()
            .add_checkpoint(tip, Checkpoint::tree_empty())
            .expect("infallible on the memory store");
        wallet
            .shard_trees
            .orchard
            .store_mut()
            .add_checkpoint(tip, Checkpoint::tree_empty())
            .expect("infallible on the memory store");

        let mut nullifiers = OrchardNullifierBuilder::new();
        for (index, value) in self.orchard_note_values.iter().enumerate() {
            let txid = TxId::from_bytes([u8::try_from(index).unwrap() + 1; 32]);
            let note = OrchardNote::new_for_test(
                OutputId::new(txid, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                OrchardCryptoNoteBuilder::default()
                    .value(NoteValue::from_raw(*value))
                    .build(),
                Memo::Empty,
                Some(Position::from(index as u64)),
            )
            .with_nullifier_for_test(nullifiers.assign_unique_nullifier().build());
            wallet.wallet_transactions.insert(
                txid,
                WalletTransaction::new_for_test_with_orchard_notes(
                    txid,
                    ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                        2 + u32::try_from(index).unwrap(),
                    )),
                    vec![note],
                    vec![],
                ),
            );
        }

        let mut sapling_nullifiers = SaplingNullifierBuilder::new();
        let orchard_note_count = self.orchard_note_values.len();
        for (index, value) in self.sapling_note_values.iter().enumerate() {
            // Txid bytes offset past the orchard range so the two note
            // families never collide.
            let txid = TxId::from_bytes([0x80 + u8::try_from(index).unwrap(); 32]);
            let mut crypto_note = SaplingCryptoNoteBuilder::default();
            crypto_note.value(sapling_crypto::value::NoteValue::from_raw(*value));
            let note = SaplingNote::new_for_test(
                OutputId::new(txid, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                crypto_note.build(),
                Memo::Empty,
                Some(Position::from(index as u64)),
            )
            .with_nullifier_for_test(sapling_nullifiers.assign_unique_nullifier().build());
            wallet.wallet_transactions.insert(
                txid,
                WalletTransaction::new_for_test(
                    txid,
                    ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                        2 + u32::try_from(orchard_note_count + index).unwrap(),
                    )),
                )
                .with_sapling_notes_for_test(vec![note]),
            );
        }

        let shielded_note_count = self.orchard_note_values.len() + self.sapling_note_values.len();
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
