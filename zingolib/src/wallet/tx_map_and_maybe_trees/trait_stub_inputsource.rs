//! this mod brings input source functionality from transaction_records_by_id

use zcash_client_backend::{
    data_api::{InputSource, SpendableNotes},
    wallet::NoteId,
};

use super::{TxMapAndMaybeTrees, TxMapAndMaybeTreesTraitError};

/// A trait representing the capability to query a data store for unspent transaction outputs belonging to a wallet.
/// combining this with WalletRead unlocks propose_transaction
/// all implementations in this file redirect to transaction_records_by_id
impl InputSource for TxMapAndMaybeTrees {
    type Error = TxMapAndMaybeTreesTraitError;
    type AccountId = zcash_primitives::zip32::AccountId;
    type NoteRef = NoteId;

    fn get_spendable_note(
        &self,
        _txid: &zcash_primitives::transaction::TxId,
        _protocol: zcash_client_backend::ShieldedProtocol,
        _index: u32,
    ) -> Result<
        Option<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        >,
        Self::Error,
    > {
        unimplemented!()
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<SpendableNotes<Self::NoteRef>, Self::Error> {
        self.transaction_records_by_id
            .select_spendable_notes(account, target_value, sources, anchor_height, exclude)
            .map_err(TxMapAndMaybeTreesTraitError::InputSource)
    }

    fn get_unspent_transparent_output(
        &self,
        _outpoint: &zcash_primitives::transaction::components::OutPoint,
    ) -> Result<Option<zcash_client_backend::wallet::WalletTransparentOutput>, Self::Error> {
        unimplemented!()
    }

    fn get_unspent_transparent_outputs(
        &self,
        address: &zcash_primitives::legacy::TransparentAddress,
        max_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[zcash_primitives::transaction::components::OutPoint],
    ) -> Result<Vec<zcash_client_backend::wallet::WalletTransparentOutput>, Self::Error> {
        self.transaction_records_by_id
            .get_unspent_transparent_outputs(address, max_height, exclude)
            .map_err(TxMapAndMaybeTreesTraitError::InputSource)
    }
}
