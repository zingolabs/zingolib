use pepper_sync::wallet::{NoteInterface, OrchardNote, OutputId, OutputInterface};
use zcash_primitives::transaction::TxId;
use zcash_protocol::PoolType;

use super::{DEFERRED_NU6_3, funded_client};
use crate::lightclient::LightClient;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::migration::inject_scheduled_migration;
use crate::testutils::mock_activation_heights_deferring_nu6_3;
use crate::testutils::mock_indexer::{MockChain, MockNet};
use crate::testutils::synthetic_wallet;
use crate::wallet::migration::{
    MigrationParams, MigrationPhase, PartId, PartState, RecommendedAction,
};

const BOUND_NOTE: u64 = 100_000;
const FREE_NOTE: u64 = 50_000;
const ORDINARY_SEND: u64 = 20_000;
const SWEEP_MINIMUM_REMAINDER: u64 = 10_000;

struct NoteRecord {
    output_id: OutputId,
    nullifier: [u8; 32],
    value: u64,
    spending_transaction: Option<TxId>,
}

async fn orchard_note_records(client: &LightClient) -> Vec<NoteRecord> {
    let wallet = client.wallet().read().await;
    wallet
        .wallet_transactions
        .values()
        .flat_map(|transaction| {
            OrchardNote::transaction_outputs(transaction)
                .iter()
                .map(|note| NoteRecord {
                    output_id: note.output_id(),
                    nullifier: note
                        .nullifier()
                        .expect("scanned wallet notes carry nullifiers")
                        .to_bytes(),
                    value: note.value(),
                    spending_transaction: note.spending_transaction(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn note_by_value(notes: &[NoteRecord], value: u64) -> &NoteRecord {
    notes
        .iter()
        .find(|note| note.value == value)
        .unwrap_or_else(|| panic!("no note of {value} zatoshis in the wallet"))
}

#[tokio::test]
async fn bound_note_reservation_and_external_spend_invalidation() {
    // Two V2 Orchard notes: the 100_000 note plays a bound split note, the
    // 50_000 note is free. The whole test then stays below the activation
    // height: reservation and reconciliation are era-independent, and
    // pre-activation orchard-only sends keep the note values' fee
    // arithmetic balanced.
    let mut net = MockNet::launch_with(MockChain::with_activation_heights(
        mock_activation_heights_deferring_nu6_3(DEFERRED_NU6_3),
    ))
    .await;
    let mut recipient = funded_client(
        &mut net,
        &[
            (PoolType::ORCHARD, BOUND_NOTE),
            (PoolType::ORCHARD, FREE_NOTE),
        ],
        0,
    )
    .await;
    let faucet_address = synthetic_wallet::external_address(PoolType::ORCHARD);

    let notes = orchard_note_records(&recipient).await;
    let reserved = note_by_value(&notes, BOUND_NOTE);
    let params = MigrationParams::provisional(recipient.wallet().read().await.chain_type());
    inject_scheduled_migration(
        &recipient,
        params,
        vec![(BOUND_NOTE, reserved.output_id, reserved.nullifier)],
        None,
        None,
    )
    .await;
    let reserved_id = reserved.output_id;

    // An ordinary send the free note can cover must leave the bound note
    // alone.
    from_inputs::quick_send(&mut recipient, vec![(&faucet_address, ORDINARY_SEND, None)])
        .await
        .expect("the ordinary send builds and transmits");
    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs the mined ordinary send");
    let notes = orchard_note_records(&recipient).await;
    assert!(
        notes
            .iter()
            .find(|note| note.output_id == reserved_id)
            .expect("the bound note is still in the wallet")
            .spending_transaction
            .is_none(),
        "an ordinary send must not consume a bound note while another note suffices"
    );
    assert!(
        note_by_value(&notes, FREE_NOTE)
            .spending_transaction
            .is_some(),
        "the free note covers the ordinary send"
    );

    // A send only the bound note can cover goes through anyway: the
    // reservation biases selection, it never blocks a spend.
    from_inputs::quick_send(&mut recipient, vec![(&faucet_address, BOUND_NOTE, None)])
        .await
        .expect("the send the bound note alone can cover builds and transmits");
    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs the mined bound-note spend");
    let notes = orchard_note_records(&recipient).await;
    assert!(
        notes
            .iter()
            .find(|note| note.output_id == reserved_id)
            .expect("the bound note is still in the wallet")
            .spending_transaction
            .is_some(),
        "the fallback pass consumes the bound note when nothing else can pay"
    );

    // Reconciliation sees the external spend, invalidates the part and
    // recommends replanning the remainder.
    let report = recipient
        .reconcile_migration()
        .await
        .expect("reconciliation runs over the synced wallet");
    assert!(
        report.actions.iter().any(|action| matches!(
            action,
            RecommendedAction::MarkInvalidated { part } if *part == PartId(0)
        )),
        "the external spend must invalidate the part: {:?}",
        report.actions
    );
    // The surviving change note is exactly the Sweep Minimum (10_000):
    // not worth replanning, so no replan is offered (ratified completion
    // rule; the prior behavior offered a replan that could only strand).
    assert!(
        !report
            .actions
            .iter()
            .any(|action| matches!(action, RecommendedAction::ReplanRemainder)),
        "a remainder at the Sweep Minimum must not prompt a replan: {:?}",
        report.actions
    );
    {
        let wallet = recipient.wallet().read().await;
        assert_eq!(
            wallet
                .migration
                .as_ref()
                .expect("the injected migration is recorded")
                .parts[0]
                .state,
            PartState::Invalidated
        );
    }

    // With every part terminal and nothing worth replanning, the next
    // reconciliation concludes the migration, disclosing the stranded
    // remainder as its residual.
    let report = recipient
        .reconcile_migration()
        .await
        .expect("the second reconciliation runs");
    assert!(
        report.actions.iter().any(|action| matches!(
            action,
            RecommendedAction::MarkComplete {
                residual: SWEEP_MINIMUM_REMAINDER
            }
        )),
        "an all-terminal migration with a Sweep-Minimum remainder must \
         complete: {:?}",
        report.actions
    );
    let wallet = recipient.wallet().read().await;
    assert_eq!(
        wallet
            .migration
            .as_ref()
            .expect("the injected migration is recorded")
            .phase,
        MigrationPhase::Complete {
            residual: SWEEP_MINIMUM_REMAINDER
        }
    );
}
