use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::{data_api::InputSource, ShieldedProtocol};
use zcash_primitives::zip32::AccountId;

use crate::{error::ZingoLibError, wallet::data::PoolNullifier};

use super::ZingoLedger;

impl InputSource for &ZingoLedger {
    type Error = ZingoLibError;
    type AccountId = zcash_primitives::zip32::AccountId;

    // TODO pick a real type for this. perhaps PoolNullifier
    type NoteRef = u32;

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
        let mut noteset: Vec<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        > = Vec::new();
        for transaction_record in self.current.values() {
            if sources.contains(&ShieldedProtocol::Sapling) {
                noteset.extend(transaction_record.select_unspent_domain_notes::<SaplingDomain>());
            }
            if sources.contains(&ShieldedProtocol::Orchard) {
                noteset.extend(transaction_record.select_unspent_domain_notes::<OrchardDomain>());
            }
        }
        Ok(noteset)
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
