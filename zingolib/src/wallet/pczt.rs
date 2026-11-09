use std::convert::Infallible;
use std::sync::OnceLock;

use nonempty::NonEmpty;
use orchard::circuit::{OrchardCircuitVersion, ProvingKey, VerifyingKey};
use pczt::roles::prover::Prover;
use pczt::roles::signer::{Signer, extract_orchard_spend_auth_signatures};
use zcash_client_backend::data_api::OutputLockStore;
use zcash_client_backend::data_api::error::Error as ZcbError;
use zcash_client_backend::data_api::wallet::{
    SignerView, create_pczt_from_proposal, extract_and_store_transaction_from_pczt,
    redact_pczt_for_signer,
};
use zcash_client_backend::proposal::Proposal;
use zcash_client_backend::util::SystemClock;
use zcash_client_backend::wallet::OvkPolicy;
use zcash_primitives::transaction::TxId;
use zcash_primitives::transaction::builder::BundlePadding;
use zcash_primitives::transaction::fees::zip317;
use zcash_protocol::PoolType;
use zcash_protocol::consensus::{BranchId, OrchardProtocolRevision};
use zip32::AccountId;

use super::LightWallet;
use super::error::WalletError;
use super::output::OutputRef;
use super::utils::LazySaplingProver;
use crate::data::proposal::ZingoProposal;

pub use pczt::Pczt;

#[derive(Debug, thiserror::Error)]
pub enum PcztError {
    #[error("No proposal is stored.")]
    NoStoredProposal,
    #[error("The stored proposal cannot be turned into a PCZT: only ordinary sends are supported.")]
    ProposalNotSupported,
    #[error("A built PCZT already awaits the signer's signatures; finish or abandon it first.")]
    SessionPending,
    #[error("Creating the PCZT failed. {0:?}")]
    Create(
        Box<
            ZcbError<
                WalletError,
                Infallible,
                Infallible,
                zip317::FeeError,
                zip317::FeeError,
                OutputRef,
            >,
        >,
    ),
    #[error("Finishing the PCZT failed. {0:?}")]
    Extract(Box<ZcbError<WalletError, Infallible, Infallible, Infallible, Infallible, OutputRef>>),
    #[error("Parsing a PCZT failed. {0:?}")]
    Parse(pczt::ParseError),
    #[error("Serializing a PCZT failed. {0:?}")]
    Encoding(pczt::EncodingError),
    #[error("Proving the Sapling bundle failed. {0:?}")]
    SaplingProve(pczt::roles::prover::SaplingError),
    #[error("Proving the Orchard bundle failed. {0:?}")]
    OrchardProve(pczt::roles::prover::OrchardError),
    #[error("Proving the Ironwood bundle failed. {0}")]
    IronwoodProve(String),
    #[error("Applying the external signatures failed. {0:?}")]
    ApplySignature(pczt::roles::signer::Error),
    #[error(
        "The transaction targets consensus branch {0}, whose Orchard circuit this build cannot prove."
    )]
    UnsupportedBranch(u32),
}

fn orchard_circuit_key<K: 'static>(
    cell: &'static OnceLock<K>,
    build: fn(OrchardCircuitVersion) -> K,
) -> &'static K {
    cell.get_or_init(|| build(OrchardCircuitVersion::PostNu6_3))
}

static ORCHARD_PROVING_KEY: OnceLock<ProvingKey> = OnceLock::new();
static ORCHARD_VERIFYING_KEY: OnceLock<VerifyingKey> = OnceLock::new();

fn post_nu6_3_branch(pczt: &pczt::Pczt) -> Result<BranchId, PcztError> {
    let branch_id = *pczt.global().consensus_branch_id();
    let branch =
        BranchId::try_from(branch_id).map_err(|_| PcztError::UnsupportedBranch(branch_id))?;
    (branch.orchard_protocol_revision() == Some(OrchardProtocolRevision::V3))
        .then_some(branch)
        .ok_or(PcztError::UnsupportedBranch(branch_id))
}

#[must_use]
pub fn signer_view(pczt: &pczt::Pczt) -> pczt::Pczt {
    redact_pczt_for_signer(pczt, SignerView::Full)
}

pub fn prove(pczt: pczt::Pczt) -> Result<pczt::Pczt, PcztError> {
    post_nu6_3_branch(&pczt)?;

    let mut prover = Prover::new(pczt);
    if prover.requires_sapling_proofs() {
        let sapling_prover = LazySaplingProver::default();
        prover = prover
            .create_sapling_proofs(crate::utils::system_rng(), &sapling_prover, &sapling_prover)
            .map_err(PcztError::SaplingProve)?;
    }
    if prover.requires_orchard_proof() {
        prover = prover
            .create_orchard_proof(
                crate::utils::system_rng(),
                orchard_circuit_key(&ORCHARD_PROVING_KEY, ProvingKey::build),
            )
            .map_err(PcztError::OrchardProve)?;
    }
    if prover.requires_ironwood_proof() {
        prover = prover
            .create_ironwood_proof(
                crate::utils::system_rng(),
                orchard_circuit_key(&ORCHARD_PROVING_KEY, ProvingKey::build),
            )
            .map_err(|failure| PcztError::IronwoodProve(format!("{failure:?}")))?;
    }
    Ok(prover.finish())
}

pub fn apply_signatures(proven: pczt::Pczt, signed: pczt::Pczt) -> Result<pczt::Pczt, PcztError> {
    let mut signer = Signer::new(proven).map_err(PcztError::ApplySignature)?;
    for signature in extract_orchard_spend_auth_signatures(&signed) {
        signer
            .apply_orchard_spend_auth_signature(&signature)
            .map_err(PcztError::ApplySignature)?;
    }
    Ok(signer.finish())
}

pub fn encode(pczt: pczt::Pczt) -> Result<Vec<u8>, PcztError> {
    pczt.serialize().map_err(PcztError::Encoding)
}

pub fn decode(bytes: &[u8]) -> Result<pczt::Pczt, PcztError> {
    pczt::Pczt::parse(bytes).map_err(PcztError::Parse)
}

impl LightWallet {
    /// - Refuses while a signature session is pending; finish or abandon
    ///   the pending session first.
    /// - Marks a signature session pending on success.
    /// - Consumes the stored proposal; every unsupported kind is restored.
    /// - Releases the account's locks and the session mark when the build
    ///   fails, since the transaction that locked them will never exist.
    pub fn create_pczt_from_stored_proposal(&mut self) -> Result<pczt::Pczt, PcztError> {
        if self.pczt_session {
            return Err(PcztError::SessionPending);
        }
        let stored = self.take_proposal().ok_or(PcztError::NoStoredProposal)?;
        match stored {
            ZingoProposal::Send {
                proposal,
                sending_account,
            } => match self.create_pczt(proposal, sending_account) {
                Ok(pczt) => {
                    self.pczt_session = true;
                    Ok(pczt)
                }
                Err(failure) => {
                    self.pczt_session = false;
                    let _ = OutputLockStore::clear_locked_outputs(self, sending_account.into());
                    Err(failure)
                }
            },
            refused => {
                self.store_proposal(refused);
                Err(PcztError::ProposalNotSupported)
            }
        }
    }

    /// - Ends the pending signature session, releasing the account's output
    ///   locks with it.
    /// - Returns the number of locks released.
    pub fn abandon_pczt(&mut self, account: AccountId) -> Result<usize, WalletError> {
        self.pczt_session = false;
        OutputLockStore::clear_locked_outputs(self, account.into())
    }

    fn create_pczt(
        &mut self,
        proposal: Proposal<zip317::FeeRule, OutputRef>,
        sending_account: AccountId,
    ) -> Result<pczt::Pczt, PcztError> {
        let chain_type = self.chain_type;
        let expiry_height = self.proposal_expiry_height(&proposal);
        create_pczt_from_proposal(
            self,
            &chain_type,
            &SystemClock,
            &mut crate::utils::system_rng(),
            sending_account.into(),
            OvkPolicy::Sender,
            &proposal,
            expiry_height,
            BundlePadding::DEFAULT,
        )
        .map_err(|failure| PcztError::Create(Box::new(failure)))
    }

    /// - Ends the pending signature session.
    /// - Parses the embedded Sapling parameters when the PCZT carries a
    ///   Sapling bundle.
    /// - Marks the wallet as requiring a save.
    pub fn finish_pczt(&mut self, pczt: pczt::Pczt) -> Result<NonEmpty<TxId>, PcztError> {
        let sapling_verifying_keys = pczt
            .has_data_in_pool(PoolType::SAPLING)
            .then(|| LazySaplingProver::default().verifying_keys());
        let orchard_bundle_present =
            pczt.has_data_in_pool(PoolType::ORCHARD) || pczt.has_data_in_pool(PoolType::IRONWOOD);

        let txid = extract_and_store_transaction_from_pczt(
            self,
            &SystemClock,
            &mut crate::utils::system_rng(),
            pczt,
            sapling_verifying_keys
                .as_ref()
                .map(|(spend_vk, output_vk)| (spend_vk, output_vk)),
            orchard_bundle_present.then_some(orchard_circuit_key(
                &ORCHARD_VERIFYING_KEY,
                VerifyingKey::build,
            )),
        )
        .map_err(|failure| PcztError::Extract(Box::new(failure)))?;
        self.save_required = true;
        self.pczt_session = false;

        Ok(NonEmpty::new(txid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::synthetic_wallet::{SyntheticWalletBuilder, external_zcash_address};
    use zcash_client_backend::zip321::{Payment, TransactionRequest};
    use zcash_protocol::value::Zatoshis;
    use zingo_test_vectors::seeds;

    fn stored_send(wallet: &mut LightWallet, amount: u64) {
        let request = TransactionRequest::new(vec![Payment::without_memo(
            external_zcash_address(PoolType::ORCHARD),
            Zatoshis::const_from_u64(amount),
        )])
        .expect("a single payment builds a transaction request");
        let proposal = wallet
            .create_send_proposal(request, AccountId::ZERO)
            .expect("a funded synthetic wallet proposes the send");
        wallet.store_proposal(ZingoProposal::Send {
            proposal,
            sending_account: AccountId::ZERO,
        });
    }

    fn wallet_derivation(wallet: &LightWallet) -> zcash_client_backend::data_api::Zip32Derivation {
        zcash_client_backend::data_api::Zip32Derivation::new(
            zip32::fingerprint::SeedFingerprint::from_seed(&wallet.mnemonic().unwrap().to_seed(""))
                .expect("a 64-byte seed always fingerprints"),
            AccountId::ZERO,
        )
    }

    #[test]
    fn pczt_from_stored_proposal_names_the_signers_zip32_paths() {
        let mut wallet = SyntheticWalletBuilder::new(seeds::HOSPITAL_MUSEUM_SEED)
            .tip(20)
            .orchard_note(1_000_000)
            .build();
        let derivation = wallet_derivation(&wallet);
        wallet
            .account_derivations
            .insert(AccountId::ZERO, derivation.clone());

        stored_send(&mut wallet, 10_000);
        let pczt = wallet
            .create_pczt_from_stored_proposal()
            .expect("an ordinary send builds a PCZT");
        assert!(
            format!("{pczt:?}").contains("zip32_derivation: Some"),
            "a signer account's PCZT names the ZIP 32 path of every unsigned spend"
        );

        assert_eq!(wallet.account_derivation(AccountId::ZERO), Some(derivation));
    }

    #[test]
    fn pczt_round_trips_through_the_encoding() {
        let mut wallet = SyntheticWalletBuilder::new(seeds::HOSPITAL_MUSEUM_SEED)
            .tip(20)
            .orchard_note(1_000_000)
            .build();
        stored_send(&mut wallet, 10_000);
        let pczt = wallet.create_pczt_from_stored_proposal().unwrap();
        let redacted = signer_view(&pczt);

        let bytes = encode(redacted).unwrap();
        assert_eq!(&bytes[0..4], b"PCZT");
        assert!(decode(&bytes).is_ok());
    }

    #[test]
    fn a_stored_shield_proposal_is_restored_and_refused() {
        let mut wallet = SyntheticWalletBuilder::new(seeds::HOSPITAL_MUSEUM_SEED)
            .tip(20)
            .transparent_coin(1_000_000)
            .build();
        let shield_proposal = wallet
            .create_shield_proposal(AccountId::ZERO)
            .expect("a transparent coin shields");
        wallet.store_proposal(ZingoProposal::Shield {
            proposal: shield_proposal,
            shielding_account: AccountId::ZERO,
        });

        assert!(matches!(
            wallet.create_pczt_from_stored_proposal(),
            Err(PcztError::ProposalNotSupported)
        ));
        assert!(wallet.take_proposal().is_some());
    }

    /// The part a hardware wallet depends on, with the seed's spending key
    /// standing in for the device: create, redact, sign, verify and apply the
    /// signatures, prove, and store. The stored transaction's expiry must be
    /// the proposal's ordinary window, so an abandoned signing session
    /// lapses instead of holding its inputs forever.
    #[test]
    fn a_software_signer_round_trips_to_a_stored_transaction() {
        use orchard::keys::SpendAuthorizingKey;
        use zcash_keys::keys::UnifiedSpendingKey;
        use zcash_protocol::consensus::BlockHeight;

        let mut wallet = SyntheticWalletBuilder::new(seeds::HOSPITAL_MUSEUM_SEED)
            .tip(20)
            .orchard_note(1_000_000)
            .ironwood_note(70_000)
            .build();
        let derivation = wallet_derivation(&wallet);
        wallet
            .account_derivations
            .insert(AccountId::ZERO, derivation);
        let tip = u32::from(
            wallet
                .sync_state
                .last_known_chain_height()
                .unwrap_or(20.into()),
        );

        stored_send(&mut wallet, 10_000);
        let authoritative = wallet
            .create_pczt_from_stored_proposal()
            .expect("an ordinary send builds a PCZT");
        let view = signer_view(&authoritative);

        let seed = wallet.mnemonic().unwrap().to_seed("");
        let spending_key =
            UnifiedSpendingKey::from_seed(&wallet.chain_type(), &seed, AccountId::ZERO)
                .expect("the fixture seed derives a spending key");
        let ask = SpendAuthorizingKey::from(spending_key.orchard());
        let unsigned = |actions: &[pczt::orchard::Action]| {
            actions
                .iter()
                .enumerate()
                .filter(|(_, action)| action.spend().spend_auth_sig().is_none())
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
        };
        let orchard_unsigned = unsigned(view.orchard().actions());
        let ironwood_unsigned = unsigned(view.ironwood().actions());

        let mut signer = Signer::new(view).expect("the full signer view signs");
        for index in orchard_unsigned {
            signer
                .sign_orchard(crate::utils::system_rng(), index, &ask)
                .expect("the spending key authorizes its own spend");
        }
        for index in ironwood_unsigned {
            signer
                .sign_ironwood(crate::utils::system_rng(), index, &ask)
                .expect("the spending key authorizes its own spend");
        }
        let signed = signer.finish();

        let proven = prove(
            apply_signatures(authoritative, signed).expect("the signatures verify against rk"),
        )
        .expect("the proofs build");
        let txid = *wallet
            .finish_pczt(proven)
            .expect("the signed PCZT stores")
            .first();

        let stored = wallet
            .wallet_transactions
            .get(&txid)
            .expect("the finished transaction is in the wallet");
        let expiry = stored.transaction().expiry_height();
        assert!(
            expiry
                <= crate::wallet::expiry::tx_expiry_height(
                    &wallet.chain_type(),
                    BlockHeight::from_u32(tip + 1),
                ),
            "the transaction must expire within the ordinary window, not the ZIP 203 maximum"
        );
    }

    #[test]
    fn a_pending_session_refuses_until_finished_or_abandoned() {
        let mut wallet = SyntheticWalletBuilder::new(seeds::HOSPITAL_MUSEUM_SEED)
            .tip(20)
            .orchard_note(1_000_000)
            .build();
        let derivation = wallet_derivation(&wallet);
        wallet
            .account_derivations
            .insert(AccountId::ZERO, derivation);

        stored_send(&mut wallet, 10_000);
        let authoritative = wallet
            .create_pczt_from_stored_proposal()
            .expect("an ordinary send builds a PCZT");

        assert!(
            matches!(
                wallet.create_pczt_from_stored_proposal(),
                Err(PcztError::NoStoredProposal) | Err(PcztError::SessionPending)
            ),
            "a second session cannot start while the first awaits signatures"
        );
        stored_send(&mut wallet, 10_000);
        assert!(matches!(
            wallet.create_pczt_from_stored_proposal(),
            Err(PcztError::SessionPending)
        ));

        assert!(wallet.abandon_pczt(AccountId::ZERO).is_ok());
        stored_send(&mut wallet, 10_000);
        assert!(wallet.create_pczt_from_stored_proposal().is_ok());
        drop(authoritative);
    }
}
