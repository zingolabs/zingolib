use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::{data_api::InputSource, ShieldedProtocol};
use zcash_primitives::zip32::AccountId;

use crate::{error::ZingoLibError, wallet::notes::NoteInterface};

use super::ZingoLedger;

impl InputSource for ZingoLedger {
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
        let transaction = self.current.get(txid);
        Ok(transaction
            .map(|transaction_record| match protocol {
                zcash_client_backend::ShieldedProtocol::Sapling => {
                    transaction_record.get_received_note::<SaplingDomain>(index)
                }
                zcash_client_backend::ShieldedProtocol::Orchard => {
                    transaction_record.get_received_note::<OrchardDomain>(index)
                }
            })
            .flatten())
    }

    fn select_spendable_notes(
        &self,
        account: zcash_primitives::zip32::AccountId,
        _target_value: zcash_primitives::transaction::components::Amount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        _anchor_height: zcash_primitives::consensus::BlockHeight,
        _exclude: &[Self::NoteRef],
    ) -> Result<
        Vec<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        >,
        Self::Error,
    > {
        if account != AccountId::ZERO {
            return Err(ZingoLibError::InvalidAccountId);
        }
        if sources.contains(&ShieldedProtocol::Sapling)
        //TODO: Genericize
        {
            let noteset = self.current.values().flat_map(|transaction_record| {
                transaction_record
                    .sapling_notes
                    .iter()
                    .filter(|sapnote| sapnote.is_spent_or_pending_spent())
            });
        }
        todo!()
    }

    fn get_unspent_transparent_output(
        &self,
        _outpoint: &zcash_primitives::transaction::components::OutPoint,
    ) -> Result<Option<zcash_client_backend::wallet::WalletTransparentOutput>, Self::Error> {
        todo!()
        // Ok(None)
    }

    fn get_unspent_transparent_outputs(
        &self,
        _address: &zcash_primitives::legacy::TransparentAddress,
        _max_height: zcash_primitives::consensus::BlockHeight,
        _exclude: &[zcash_primitives::transaction::components::OutPoint],
    ) -> Result<Vec<zcash_client_backend::wallet::WalletTransparentOutput>, Self::Error> {
        todo!()
        // Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    // #[test]
    // fn
}
