use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::{data_api::InputSource, ShieldedProtocol};
use zcash_primitives::zip32::AccountId;

use crate::error::ZingoLibError;

use super::SpendKit;

impl InputSource for SpendKit<'_> {
    type Error = ZingoLibError;
    type AccountId = zcash_primitives::zip32::AccountId;
    type NoteRef = u32; //should this be a nullifier? or a Rho?

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
        let transaction = self.record_book.all_transactions.get(txid);
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
        account: Self::AccountId,
        target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
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
        if account != AccountId::ZERO {
            return Err(ZingoLibError::UnknownError);
        }
        let mut noteset: Vec<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        > = Vec::new();
        for transaction_record in self.record_book.all_transactions.values() {
            if sources.contains(&ShieldedProtocol::Sapling) {
                noteset.extend(transaction_record.select_unspent_domain_notes::<SaplingDomain>());
            }
            match sources.contains(&ShieldedProtocol::Orchard) {
                true => {
                    noteset
                        .extend(transaction_record.select_unspent_domain_notes::<OrchardDomain>());
                }
                false => (),
            }
        }
        Ok(noteset) //review! this is incorrect because it selects ALL the unspent notes, not just enough for the target value.
    }
}
