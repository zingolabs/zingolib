use zcash_protocol::ShieldedPool;
use zingolib::get_base_address_macro;
use zingolib::lightclient::LightClient;
use zingolib_testutils::scenarios::{self, IndexerConvergence};
use zingolib_testutils::setup_metrics::MeteredNet;

/// - Sends 100_000 zatoshis from the faucet to the recipient and mines the block.
/// - Strips the recipient wallet's tracking of `pool` under the wallet's write lock.
/// - Mines two more blocks and runs two sync sessions on the recipient.
pub async fn assert_notes_in_untracked_history_are_recovered(
    local_net: &MeteredNet,
    faucet: &mut LightClient,
    recipient: &mut LightClient,
    pool: ShieldedPool,
) {
    let balance = async |client: &LightClient| -> u64 {
        let balance = client
            .account_balance(zip32::AccountId::ZERO)
            .await
            .expect("a synced client reports a balance");
        match pool {
            ShieldedPool::Orchard => balance.confirmed_orchard_balance,
            ShieldedPool::Ironwood => balance.confirmed_ironwood_balance,
            ShieldedPool::Sapling => balance.confirmed_sapling_balance,
        }
        .map(zcash_protocol::value::Zatoshis::into_u64)
        .unwrap_or(0)
    };

    let recipient_address = get_base_address_macro!(recipient, "unified");
    scenarios::send_and_bump(
        local_net,
        faucet,
        vec![(recipient_address.as_str(), 100_000, None)],
    )
    .await;
    scenarios::sync_client_to_validator_tip(local_net, recipient).await;

    let funded_balance = balance(recipient).await;
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
        balance(recipient).await,
        0,
        "stripping must actually remove the note that recovery is then judged by"
    );

    // Mining gives the wallet a block whose metadata it must account for.
    // The barrier is held here rather than by a syncing helper, so the
    // sessions below are counted exactly.
    let target = scenarios::generate_n_blocks_return_new_height(local_net, 2).await;
    local_net.converge(target).await;

    // The first session finds the pool's recorded tree wanting and reopens
    // its history, ending there rather than reporting a sync that would have
    // left the notes lost.
    let reopened = recipient.sync_and_await().await;
    assert!(
        reopened.is_err(),
        "a wallet whose {pool:?} history cannot account for the chain must not \
         report a successful sync: {reopened:?}"
    );

    // The second session rescans the reopened history and recovers the note.
    scenarios::sync_client_to_validator_tip(local_net, recipient).await;

    assert_eq!(
        balance(recipient).await,
        funded_balance,
        "the {pool:?} note was left lost in history that was scanned without \
         {pool:?} tracking",
    );
}
