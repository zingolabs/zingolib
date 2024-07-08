//! lightclient functions with added assertions. used for tests.

use zcash_client_backend::PoolType;
use zingolib::lightclient::LightClient;

use crate::{
    assertions::{assert_recipient_total_lte_to_proposal_total, assert_sender_fee_and_status},
    chain_generics::conduct_chain::ConductChain,
    lightclient::{from_inputs, get_base_address},
};
use zingo_status::confirmation_status::ConfirmationStatus;

/// sends to any combo of recipient clients checks that each recipient also recieved the expected balances
/// test-only generic
/// NOTICE this function bumps the chain and syncs the client
/// only compatible with zip317
/// returns the total fee for the transfer
/// test_mempool can be enabled when the test harness supports it: TBI
pub async fn propose_send_bump_sync_all_recipients<CC>(
    environment: &mut CC,
    sender: &LightClient,
    sends: Vec<(&LightClient, PoolType, u64, Option<&str>)>,
    test_mempool: bool,
) -> u64
where
    CC: ConductChain,
{
    let mut subraw_receivers = vec![];
    for (recipient, pooltype, amount, memo_str) in sends.clone() {
        let address = get_base_address(recipient, pooltype).await;
        subraw_receivers.push((address, amount, memo_str));
    }

    let raw_receivers = subraw_receivers
        .iter()
        .map(|(address, amount, opt_memo)| (address.as_str(), *amount, *opt_memo))
        .collect();

    let proposal = from_inputs::propose(sender, raw_receivers).await.unwrap();

    let txids = sender
        .complete_and_broadcast_stored_proposal()
        .await
        .unwrap();

    // digesting the calculated transaction
    let recorded_fee = assert_sender_fee_and_status(
        sender,
        &proposal,
        &txids,
        ConfirmationStatus::Pending(0.into()),
    )
    .await;

    if test_mempool {
        // mempool scan shows the same
        sender.do_sync(false).await.unwrap();
        assert_sender_fee_and_status(
            sender,
            &proposal,
            &txids,
            ConfirmationStatus::Pending(0.into()),
        )
        .await;
        // TODO: distribute receivers
        // recipient.do_sync(false).await.unwrap();
        // assert_receiver_fee(recipient, &proposal, &txids).await;
    }

    environment.bump_chain().await;
    // chain scan shows the same
    sender.do_sync(false).await.unwrap();
    assert_sender_fee_and_status(
        sender,
        &proposal,
        &txids,
        ConfirmationStatus::Pending(0.into()),
    )
    .await;
    let send_ua_id = sender.do_addresses().await[0]["address"].clone();
    for (recipient, _, _, _) in sends {
        if send_ua_id != recipient.do_addresses().await[0]["address"].clone() {
            recipient.do_sync(false).await.unwrap();
            assert_recipient_total_lte_to_proposal_total(recipient, &proposal, &txids).await;
        }
    }
    recorded_fee
}

/// a test-only generic version of shield that includes assertions that the proposal was fulfilled
/// NOTICE this function bumps the chain and syncs the client
/// only compatible with zip317
/// returns the total fee for the transfer
pub async fn propose_shield_bump_sync<CC>(
    environment: &mut CC,
    client: &LightClient,
    test_mempool: bool,
) -> u64
where
    CC: ConductChain,
{
    let proposal = client.propose_shield().await.unwrap();
    let txids = client
        .complete_and_broadcast_stored_proposal()
        .await
        .unwrap();

    // digesting the calculated transaction
    let recorded_fee = assert_sender_fee_and_status(
        client,
        &proposal,
        &txids,
        ConfirmationStatus::Pending(0.into()),
    )
    .await;

    if test_mempool {
        // mempool scan shows the same
        client.do_sync(false).await.unwrap();
        assert_sender_fee_and_status(
            client,
            &proposal,
            &txids,
            ConfirmationStatus::Pending(0.into()),
        )
        .await;
    }

    environment.bump_chain().await;
    // chain scan shows the same
    client.do_sync(false).await.unwrap();
    assert_sender_fee_and_status(
        client,
        &proposal,
        &txids,
        ConfirmationStatus::Pending(0.into()),
    )
    .await;

    recorded_fee
}
