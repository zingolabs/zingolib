//! lightclient functions with added assertions. used for tests.

use zcash_client_backend::PoolType;
use zingolib::lightclient::LightClient;

use crate::{
    assertions::assert_send_outputs_match_sender,
    chain_generic_tests::conduct_chain::ConductChain,
    lightclient::{from_inputs, get_base_address},
};

/// a test-only generic version of send that includes assertions that the proposal was fulfilled
/// NOTICE this function bumps the chain and syncs the client
/// only compatible with zip317
pub async fn propose_send_bump_sync<CC>(
    environment: &mut CC,
    client: &LightClient,
    raw_receivers: Vec<(&str, u64, Option<&str>)>,
) where
    CC: ConductChain,
{
    let proposal = from_inputs::propose(client, raw_receivers).await.unwrap();
    let txids = client
        .complete_and_broadcast_stored_proposal()
        .await
        .unwrap();

    environment.bump_chain().await;

    client.do_sync(false).await.unwrap();

    assert_send_outputs_match_sender(client, &proposal, &txids).await;
}

/// this version assumes a single recipient and measures that the recipient also recieved the expected balances
/// test-only generic
/// NOTICE this function bumps the chain and syncs the client
/// only compatible with zip317
pub async fn propose_send_bump_sync_recipient<CC>(
    environment: &mut CC,
    sender: &LightClient,
    recipient: &LightClient,
    sends: Vec<(PoolType, u64)>,
) where
    CC: ConductChain,
{
    let mut subraw_receivers = vec![];
    for (pooltype, amount) in sends {
        let address = get_base_address(recipient, pooltype).await;
        subraw_receivers.push((address, amount, None));
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

    environment.bump_chain().await;

    sender.do_sync(false).await.unwrap();
    assert_send_outputs_match_sender(sender, &proposal, &txids).await;

    recipient.do_sync(false).await.unwrap();
}

/// a test-only generic version of shield that includes assertions that the proposal was fulfilled
/// NOTICE this function bumps the chain and syncs the client
/// only compatible with zip317
pub async fn propose_shield_bump_sync<CC>(environment: &mut CC, client: &LightClient)
where
    CC: ConductChain,
{
    let proposal = client.propose_shield().await.unwrap();
    let txids = client
        .complete_and_broadcast_stored_proposal()
        .await
        .unwrap();

    environment.bump_chain().await;

    client.do_sync(false).await.unwrap();

    assert_send_outputs_match_sender(client, &proposal, &txids).await;
}
