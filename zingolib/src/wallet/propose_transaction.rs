use zcash_client_backend::data_api::InputSource;

use super::transactions::TransactionMetadataSet;
use crate::error::ZingoLibError;

pub struct ZingoInputSource {
    metadata: TransactionMetadataSet,
}

impl InputSource for ZingoInputSource {
    type Error = ZingoLibError;

    // We can always change this later if we decide we need it
    type NoteRef = ();

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
        let transaction = self.metadata.current.get(txid);
        Ok(transaction.map(|transaction_record| match protocol {
            zcash_client_backend::ShieldedProtocol::Sapling => transaction_record
                .sapling_notes
                .iter()
                .find(|note| note.output_index == Some(index)),
            zcash_client_backend::ShieldedProtocol::Orchard => transaction_record
                .orchard_notes
                .iter()
                .find(|note| note.output_index == Some(index)),
        }))
    }

    fn select_spendable_notes(
        &self,
        account: zcash_primitives::zip32::AccountId,
        target_value: zcash_primitives::transaction::components::Amount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<
        Vec<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        >,
        Self::Error,
    > {
        todo!()
    }
}
