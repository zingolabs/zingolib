//! The apply layer of the in-tree spend pipeline (ADR 0010): the
//! pipeline's single mutation site.
//!
//! [`LightWallet::apply_calculated`] takes the pure plan and the
//! wallet-pure build's output and performs every wallet effect of a
//! calculation in one place: it reserves the TEX flow's Refund Address
//! (reservation happens exactly when a transaction bearing the address
//! comes into existence, so an abandoned proposal never burns an index),
//! records each built transaction with `Calculated` status, and marks the
//! wallet dirty.

use nonempty::NonEmpty;

use zcash_protocol::TxId;

use pepper_sync::error::SyncError;
use pepper_sync::wallet::traits::SyncWallet;
use zingo_status::confirmation_status::ConfirmationStatus;

use super::build::BuiltStep;
use super::proposal::Proposal;
use crate::wallet::LightWallet;
use crate::wallet::error::WalletError;

impl LightWallet {
    /// Applies a calculation's results to the wallet — the pipeline's
    /// single mutation site. Returns the calculated txids in step order.
    ///
    /// The Refund Address reserved here for a TEX flow is necessarily
    /// the one the build derived: derivation and reservation both take
    /// the lowest unreserved index, and the wallet write lock is held
    /// across materials, build, and apply.
    ///
    /// # Errors
    ///
    /// Returns [`WalletError`] if address reservation or transaction
    /// recording fails.
    pub fn apply_calculated(
        &mut self,
        proposal: &Proposal,
        built: NonEmpty<BuiltStep>,
    ) -> Result<NonEmpty<TxId>, WalletError> {
        if let Proposal::TexTransfer(_) = proposal {
            self.generate_refund_addresses(1, proposal.account_id())?;
        }

        let chain_type = self.chain_type;
        let unified_full_viewing_keys = SyncWallet::get_unified_full_viewing_keys(self)?;
        let timestamp = u32::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("now is after the epoch")
                .as_secs(),
        )
        .expect("the current time fits u32");
        let status = ConfirmationStatus::Calculated(proposal.target_height());

        let mut txids = Vec::with_capacity(built.len());
        for step in built {
            let txid = step.transaction.txid();
            match pepper_sync::scan_pending_transaction(
                &chain_type,
                &unified_full_viewing_keys,
                self,
                step.transaction,
                status,
                timestamp,
            ) {
                Ok(()) => (),
                Err(SyncError::ScanError(e)) => return Err(e.into()),
                Err(SyncError::WalletError(e)) => return Err(e),
                Err(_) => {
                    panic!("`scan_pending_transaction` returns only scan or wallet errors")
                }
            }
            txids.push(txid);
        }
        self.save_required = true;

        Ok(NonEmpty::from_vec(txids).expect("built steps are nonempty"))
    }
}

#[cfg(test)]
mod tests {
    //! The reserve-at-apply acceptance gate (ADR 0010): a Refund Address
    //! is reserved exactly when a transaction bearing it comes into
    //! existence — an abandoned TEX proposal leaves no trace, where the
    //! old path burned an index at propose time.

    use pepper_sync::keys::transparent::TransparentScope;
    use zcash_protocol::value::Zatoshis;
    use zip321::{Payment, TransactionRequest};

    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;
    use crate::wallet::spend::plan::plan_transfer;

    fn refund_address_count(wallet: &LightWallet) -> usize {
        wallet
            .transparent_addresses()
            .keys()
            .filter(|id| id.scope() == TransparentScope::Refund)
            .count()
    }

    fn tex_request(external: &LightWallet) -> TransactionRequest {
        use pepper_sync::keys::decode_address;
        use zcash_keys::address::Address;
        use zcash_transparent::address::TransparentAddress;

        let taddr = external
            .transparent_addresses()
            .values()
            .next()
            .unwrap()
            .clone();
        let Address::Transparent(TransparentAddress::PublicKeyHash(taddr_bytes)) =
            decode_address(&external.chain_type(), &taddr).unwrap()
        else {
            panic!("a wallet-generated first taddr is p2pkh")
        };
        let tex_address =
            crate::testutils::interpret_taddr_as_tex_addr(taddr_bytes, &external.chain_type());
        TransactionRequest::new(vec![Payment::without_memo(
            zcash_address::ZcashAddress::try_from_encoded(&tex_address).unwrap(),
            Zatoshis::const_from_u64(100_000),
        )])
        .unwrap()
    }

    #[tokio::test]
    async fn refund_address_reserved_at_apply_never_at_propose() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(5_000_000)
                .build();
        let external =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();

        // Proposing (and even resolving materials, which derives the
        // address) reserves nothing: an abandoned proposal leaves no
        // trace.
        let abandoned = plan_transfer(
            &wallet,
            tex_request(&external),
            zip32::AccountId::ZERO,
            None,
            false,
        )
        .expect("planner plans the TEX flow");
        let _materials = wallet.spend_materials(&abandoned).unwrap();
        assert_eq!(
            refund_address_count(&wallet),
            0,
            "no reservation before apply"
        );

        // Calculating end-to-end reserves exactly one.
        let proposal = plan_transfer(
            &wallet,
            tex_request(&external),
            zip32::AccountId::ZERO,
            None,
            false,
        )
        .expect("planner plans the TEX flow");
        let txids = wallet.calculate_transactions(proposal).await.unwrap();
        assert_eq!(txids.len(), 2, "the ZIP-320 pair");
        assert_eq!(refund_address_count(&wallet), 1, "one reservation at apply");
    }
}
