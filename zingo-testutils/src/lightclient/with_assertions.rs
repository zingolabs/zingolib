//! lightclient functions with added assertions. used for tests.

use zingolib::lightclient::LightClient;

use crate::{
    assertions::assert_send_outputs_match_client, chain_generic_tests::conduct_chain::ConductChain,
    lightclient::from_inputs,
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

    assert_send_outputs_match_client(client, &proposal, &txids).await;
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

    assert_send_outputs_match_client(client, &proposal, &txids).await;
}
