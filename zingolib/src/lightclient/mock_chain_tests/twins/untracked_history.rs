use zcash_protocol::value::Zatoshis;
use zcash_protocol::{PoolType, ShieldedPool};

use super::{DEFERRED_NU6_3, FUNDING, HOSPITAL_MUSEUM, fund_on_chain};
use crate::lightclient::LightClient;
use crate::testutils::lightclient::get_base_address;
use crate::testutils::mock_indexer::{MockChain, MockNet};
use crate::testutils::{mock_activation_heights, mock_activation_heights_deferring_nu6_3};

const HIDDEN_BLOCKS: u32 = 2;
const RECOVERY_SESSIONS: usize = 5;

async fn confirmed_balance(client: &LightClient, pool: ShieldedPool) -> u64 {
    let balance = client
        .account_balance(zip32::AccountId::ZERO)
        .await
        .expect("a synced client reports a balance");
    match pool {
        ShieldedPool::Orchard => balance.confirmed_orchard_balance,
        ShieldedPool::Ironwood => balance.confirmed_ironwood_balance,
        ShieldedPool::Sapling => balance.confirmed_sapling_balance,
    }
    .map(Zatoshis::into_u64)
    .unwrap_or(0)
}

/// - Strips the recipient wallet's tracking of `pool` under the wallet's write lock.
/// - Mines two blocks and runs sync sessions on the recipient until the note is recovered.
pub(super) async fn recover_untracked_history(
    net: &MockNet,
    recipient: &mut LightClient,
    pool: ShieldedPool,
) {
    let funded_balance = confirmed_balance(recipient, pool).await;
    assert!(
        funded_balance > 0,
        "the recipient must hold a {pool:?} note before its history is stripped"
    );

    // Reduce the wallet to what a build that never tracked the pool would
    // have left: the very same scanned ranges, none of the pool's data.
    {
        let wallet = recipient.wallet();
        let mut wallet = wallet.write().await;
        pepper_sync::wallet::strip_pool_tracking_for_test(&mut *wallet, pool)
            .expect("stripping a local wallet is infallible");
    }
    assert_eq!(
        confirmed_balance(recipient, pool).await,
        0,
        "stripping must actually remove the note that recovery is then judged by"
    );

    // Mining gives the wallet blocks whose metadata it must account for.
    net.chain.write().await.mine_empty_blocks(HIDDEN_BLOCKS);

    // The first session finds the pool's recorded tree wanting and reopens
    // its history, ending there rather than reporting a sync that would have
    // left the notes lost.
    let reopened = recipient.sync_and_await().await;
    assert!(
        reopened.is_err(),
        "a wallet whose {pool:?} history cannot account for the chain must not \
         report a successful sync: {reopened:?}"
    );

    // The following sessions rescan the reopened history and recover the note.
    for _ in 0..RECOVERY_SESSIONS {
        if recipient.sync_and_await().await.is_ok()
            && confirmed_balance(recipient, pool).await == funded_balance
        {
            break;
        }
    }
    assert_eq!(
        confirmed_balance(recipient, pool).await,
        funded_balance,
        "the {pool:?} note was left lost in history that was scanned without \
         {pool:?} tracking",
    );
}

async fn notes_in_untracked_history_are_recovered(pool: ShieldedPool) {
    let activation_heights = match pool {
        ShieldedPool::Ironwood => mock_activation_heights(),
        ShieldedPool::Orchard => mock_activation_heights_deferring_nu6_3(DEFERRED_NU6_3),
        ShieldedPool::Sapling => panic!("a unified-address funding never lands in Sapling"),
    };
    let mut net =
        MockNet::launch_with(MockChain::with_activation_heights(activation_heights)).await;
    let mut recipient = net.client(HOSPITAL_MUSEUM, None).await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.chain.write().await.mine_empty_blocks(1);
    fund_on_chain(&net, vec![(&recipient_ua, FUNDING, None)]).await;
    recipient
        .sync_and_await()
        .await
        .expect("the funded recipient syncs");
    assert_eq!(confirmed_balance(&recipient, pool).await, FUNDING);
    recover_untracked_history(&net, &mut recipient, pool).await;
}

#[tokio::test]
async fn ironwood_notes_in_untracked_history_are_recovered() {
    notes_in_untracked_history_are_recovered(ShieldedPool::Ironwood).await;
}

#[tokio::test]
async fn orchard_notes_in_untracked_history_are_recovered() {
    notes_in_untracked_history_are_recovered(ShieldedPool::Orchard).await;
}
