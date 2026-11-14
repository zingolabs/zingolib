use nonempty::NonEmpty;
use zcash_client_backend::proposal::ShieldedInputs;
use zcash_client_backend::wallet::{Note, ReceivedNote};
use zcash_protocol::PoolType;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::value::{COIN, Zatoshis};

use crate::mocks::default_txid;
use crate::mocks::orchard_note::OrchardCryptoNoteBuilder;
use crate::mocks::proposal::{
    PaymentBuilder, ProposalBuilder, StepBuilder, TransactionRequestBuilder,
};
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::expiry::tx_expiry_height;
use crate::wallet::output::OutputRef;
use pepper_sync::wallet::OutputId;

const TARGET: u32 = 435;
/// A ZIP 318 bucket boundary of the synthetic wallet's chain, which
/// activates NU7 at its second block: the grid's boundaries are 0 and the
/// multiples of 3 × 144 from there.
const BUCKET_BOUNDARY: u32 = 432;

/// An ordinary proposal takes the wallet's delta for its target height.
#[test]
fn an_ordinary_proposal_takes_the_wallets_delta() {
    let wallet =
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
    let mut builder = ProposalBuilder::default();
    builder.min_target_height(BlockHeight::from_u32(TARGET));
    let proposal = builder.build();

    assert_eq!(
        wallet.proposal_expiry_height(&proposal),
        Some(tx_expiry_height(
            &wallet.chain_type(),
            BlockHeight::from_u32(TARGET)
        ))
    );
}

/// A proposal holding a canonical ZIP 318 crossing, one Orchard note
/// paying exactly one denomination into Ironwood from a bucket-boundary
/// anchor at the canonical fee, leaves the expiry to the backend, which
/// gives such a step the ZIP's rolling expiry and refuses any other.
#[test]
fn a_canonical_crossing_leaves_the_expiry_to_the_backend() {
    let wallet =
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
    let chain = wallet.chain_type();
    let canonical_fee =
        zcash_client_backend::fees::canonical_crossing_fee(&chain, BlockHeight::from_u32(TARGET))
            .unwrap();
    let denomination = Zatoshis::const_from_u64(COIN);
    let txid = default_txid();
    let note = OrchardCryptoNoteBuilder::default()
        .value(orchard::value::NoteValue::from_raw(
            COIN + u64::from(canonical_fee),
        ))
        .build();
    let mut payment = PaymentBuilder::default();
    payment.amount(denomination);
    let mut request = TransactionRequestBuilder::new();
    request.payments(payment.build());
    let mut step = StepBuilder::default();
    step.transaction_request(request.build())
        .shielded_inputs(Some(ShieldedInputs::from_parts(NonEmpty::singleton(
            ReceivedNote::from_parts(
                OutputRef::new(OutputId::new(txid, 0), PoolType::ORCHARD),
                txid,
                0,
                Note::Orchard {
                    note,
                    pool: orchard::ValuePool::Orchard,
                },
                zip32::Scope::External,
                incrementalmerkletree::Position::from(1),
                None,
                None,
            ),
        ))))
        .anchor_height(BlockHeight::from_u32(BUCKET_BOUNDARY))
        .balance(
            zcash_client_backend::fees::TransactionBalance::new(vec![], canonical_fee).unwrap(),
        );
    let mut builder = ProposalBuilder::default();
    builder
        .min_target_height(BlockHeight::from_u32(TARGET))
        .steps(NonEmpty::singleton(step.build()));
    let proposal = builder.build();

    assert_eq!(wallet.proposal_expiry_height(&proposal), None);
}
