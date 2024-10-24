//! lightclient functions with added assertions. used for tests.

use crate::lightclient::LightClient;
use zcash_client_backend::PoolType;

use crate::testutils::{
    assertions::{assert_recipient_total_lte_to_proposal_total, assert_record_fee_and_status},
    chain_generics::conduct_chain::ConductChain,
    lightclient::{from_inputs, get_base_address},
};
use zingo_status::confirmation_status::ConfirmationStatus;

/// sends to any combo of recipient clients checks that each recipient also received the expected balances
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

    let send_height = sender
        .wallet
        .get_target_height_and_anchor_offset()
        .await
        .expect("sender has a target height")
        .0;

    // digesting the calculated transaction
    // this step happens after transaction is recorded locally, but before learning anything about whether the server accepted it
    let recorded_fee = assert_record_fee_and_status(
        sender,
        &proposal,
        &txids,
        ConfirmationStatus::Transmitted(send_height.into()),
    )
    .await;

    let send_ua_id = sender.do_addresses().await[0]["address"].clone();

    if test_mempool {
        // mempool scan shows the same
        sender.do_sync(false).await.unwrap();

        // let the mempool monitor get a chance
        // to listen
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;

        assert_record_fee_and_status(
            sender,
            &proposal,
            &txids,
            ConfirmationStatus::Mempool(send_height.into()),
        )
        .await;

        // TODO: distribute receivers
        for (recipient, _, _, _) in sends.clone() {
            if send_ua_id != recipient.do_addresses().await[0]["address"].clone() {
                recipient.do_sync(false).await.unwrap();
                let records = &recipient
                    .wallet
                    .transaction_context
                    .transaction_metadata_set
                    .read()
                    .await
                    .transaction_records_by_id;
                for txid in &txids {
                    let record = records.get(txid).expect("recipient must recognize txid");
                    assert_eq!(
                        record.status,
                        ConfirmationStatus::Mempool(send_height.into()),
                    )
                }
            }
        }
    }

    environment.bump_chain().await;
    // chain scan shows the same
    sender.do_sync(false).await.unwrap();
    assert_record_fee_and_status(
        sender,
        &proposal,
        &txids,
        ConfirmationStatus::Confirmed((send_height).into()),
    )
    .await;
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

    let send_height = client
        .wallet
        .get_target_height_and_anchor_offset()
        .await
        .expect("sender has a target height")
        .0;

    let txids = client
        .complete_and_broadcast_stored_proposal()
        .await
        .unwrap();

    // digesting the calculated transaction
    let recorded_fee = assert_record_fee_and_status(
        client,
        &proposal,
        &txids,
        ConfirmationStatus::Transmitted(send_height.into()),
    )
    .await;

    if test_mempool {
        // mempool scan shows the same
        client.do_sync(false).await.unwrap();
        assert_record_fee_and_status(
            client,
            &proposal,
            &txids,
            ConfirmationStatus::Mempool(send_height.into()),
        )
        .await;
    }

    environment.bump_chain().await;
    // chain scan shows the same
    client.do_sync(false).await.unwrap();
    assert_record_fee_and_status(
        client,
        &proposal,
        &txids,
        ConfirmationStatus::Confirmed(send_height.into()),
    )
    .await;

    recorded_fee
}
