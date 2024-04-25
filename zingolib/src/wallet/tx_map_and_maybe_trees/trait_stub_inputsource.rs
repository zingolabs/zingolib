//! this mod brings input source functionality from transaction_records_by_id

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::{
    data_api::{InputSource, SpendableNotes},
    wallet::{ReceivedNote, WalletTransparentOutput},
    ShieldedProtocol,
};
use zcash_primitives::{
    legacy::Script,
    transaction::components::{amount::NonNegativeAmount, TxOut},
};
use zip32::AccountId;

use crate::{
    error::{ZingoLibError, ZingoLibResult},
    wallet::{
        notes::{query::OutputSpendStatusQuery, OutputInterface, ShNoteId},
        transaction_records_by_id::TransactionRecordsById,
    },
};

use super::TxMapAndMaybeTrees;

/// A trait representing the capability to query a data store for unspent transaction outputs belonging to a wallet.
/// combining this with WalletRead unlocks propose_transaction
/// all implementations in this file redirect to transaction_records_by_id
impl InputSource for TxMapAndMaybeTrees {
    type Error = ZingoLibError;
    type AccountId = zcash_primitives::zip32::AccountId;
    type NoteRef = ShNoteId;

    fn get_spendable_note(
        &self,
        txid: &zcash_primitives::transaction::TxId,
        protocol: zcash_client_backend::ShieldedProtocol,
        index: u32,
    ) -> Result<
        Option<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        >,
        Self::Error,
    > {
        self.transaction_records_by_id
            .get_spendable_note(txid, protocol, index)
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<SpendableNotes<ShNoteId>, ZingoLibError> {
        self.transaction_records_by_id.select_spendable_notes(
            account,
            target_value,
            sources,
            anchor_height,
            exclude,
        )
    }

    fn get_unspent_transparent_output(
        &self,
        outpoint: &zcash_primitives::transaction::components::OutPoint,
    ) -> Result<Option<zcash_client_backend::wallet::WalletTransparentOutput>, Self::Error> {
        self.transaction_records_by_id
            .get_unspent_transparent_output(outpoint)
    }

    fn get_unspent_transparent_outputs(
        &self,
        address: &zcash_primitives::legacy::TransparentAddress,
        max_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[zcash_primitives::transaction::components::OutPoint],
    ) -> Result<Vec<zcash_client_backend::wallet::WalletTransparentOutput>, Self::Error> {
        self.transaction_records_by_id
            .get_unspent_transparent_outputs(address, max_height, exclude)
    }
}
