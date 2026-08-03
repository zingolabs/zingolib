//! Test scaffolding for the editorial layer, plus a path-for-path mirror
//! of zingolib's testutils so funneled consumers keep their test imports.

pub use zingolib::testutils::*;

use zcash_protocol::{PoolType, ShieldedPool};

use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::with_assertions;
use zingolib::testutils::lightclient::get_base_address;

use crate::ext::LightClientViewModelExt as _;
use crate::value_transfer::{SelfSendValueTransfer, SentValueTransfer, ValueTransferKind};

/// Fixture for testing various vt transactions
pub async fn create_various_value_transfers<CC>()
where
    CC: ConductChain,
{
    let mut environment = CC::setup().await;
    let mut sender = environment.fund_client_orchard(250_000).await;
    let sender_orchard_addr =
        get_base_address(&sender, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let sender_sapling_addr =
        get_base_address(&sender, PoolType::Shielded(ShieldedPool::Sapling)).await;
    let sender_taddr = get_base_address(&sender, PoolType::Transparent).await;
    let send_value_for_recipient = 23_000;
    let send_value_self = 17_000;

    tracing::info!("client is ready to send");

    let mut recipient = environment.create_client().await;
    tracing::debug!("TEST 1");
    with_assertions::assure_propose_send_bump_sync_all_recipients(
        &mut environment,
        &mut sender,
        vec![
            (
                &get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await,
                send_value_for_recipient,
                Some("Orchard sender to recipient"),
            ),
            (
                &sender_sapling_addr,
                send_value_self,
                Some("Orchard sender to self"),
            ),
            (&sender_taddr, send_value_self, None),
        ],
        vec![&mut recipient],
        false,
    )
    .await
    .unwrap();

    assert_eq!(sender.value_transfers(true).await.unwrap().len(), 3);

    assert!(
        sender
            .value_transfers(false)
            .await
            .unwrap()
            .iter()
            .any(|vt| { vt.kind == ValueTransferKind::Received })
    );

    assert!(
        sender
            .value_transfers(false)
            .await
            .unwrap()
            .iter()
            .any(|vt| { vt.kind == ValueTransferKind::Sent(SentValueTransfer::Send) })
    );

    assert!(
        sender
            .value_transfers(false)
            .await
            .unwrap()
            .iter()
            .any(|vt| {
                vt.kind
                    == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                        SelfSendValueTransfer::MemoToSelf,
                    ))
            })
    );

    assert_eq!(recipient.value_transfers(true).await.unwrap().len(), 1);

    tracing::debug!("TEST 2");
    with_assertions::assure_propose_send_bump_sync_all_recipients(
        &mut environment,
        &mut sender,
        vec![(&sender_orchard_addr, send_value_self, None)],
        vec![],
        false,
    )
    .await
    .unwrap();

    assert_eq!(sender.value_transfers(true).await.unwrap().len(), 4);
    assert_eq!(
        sender.value_transfers(true).await.unwrap()[0].kind,
        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(SelfSendValueTransfer::Basic))
    );

    with_assertions::assure_propose_shield_bump_sync(&mut environment, &mut sender, false)
        .await
        .unwrap();
    assert_eq!(sender.value_transfers(true).await.unwrap().len(), 5);
    assert_eq!(
        sender.value_transfers(true).await.unwrap()[0].kind,
        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(SelfSendValueTransfer::Shield))
    );
}
