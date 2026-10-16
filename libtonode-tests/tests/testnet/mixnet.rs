use std::path::PathBuf;
use std::time::Duration;

use zcash_protocol::{PoolType, ShieldedPool};
use zingolib::lightclient::send::TransmitRoute;
use zingolib::mixnet::Indicator;
use zingolib::mixnet::provision::{SpawnHints, resolve_proxy_path};
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::NetworkedTestEnvironment;
use zingolib::testutils::lightclient::{from_inputs, get_base_address};

use super::SELF_SEND_VALUE;

const READINESS_LIMIT: Duration = Duration::from_secs(300);

/// Whether the mixnet has stopped bootstrapping, in either direction.
fn settled(mode: Indicator) -> bool {
    match mode {
        Indicator::Ready | Indicator::PreviouslyProvenThisEpoch => true,
        Indicator::Unattached | Indicator::SwitchedOff | Indicator::Died => true,
        Indicator::Bootstrapping => false,
    }
}

/// - Spawns the nym-proxy binary the provisioning precedence resolves and attaches the wallet to it.
/// - Sends over the real tunnel to the testnet indexer.
#[tokio::test]
async fn a_mixnet_send_crosses_a_real_proxy_to_the_node() {
    let proxy = PathBuf::from(resolve_proxy_path(&SpawnHints::default()));
    let mut environment = NetworkedTestEnvironment::setup().await;
    let mut client = environment.create_faucet().await;
    client
        .enable_mixnet(&proxy)
        .await
        .expect("the proxy spawns and the wallet attaches to it");
    let mut status = client.subscribe_mixnet_status();
    tokio::time::timeout(
        READINESS_LIMIT,
        status.wait_for(|status| settled(status.mode)),
    )
    .await
    .unwrap_or_else(|_| panic!("the mixnet did not settle within {READINESS_LIMIT:?}"))
    .expect("the status channel outlives the session");
    let mode = status.borrow().mode;
    assert!(
        mode.is_ready(),
        "the mixnet must settle in a routing state, found {mode:?}"
    );

    let own_address = get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let reports =
        from_inputs::quick_send_reported(&mut client, vec![(&own_address, SELF_SEND_VALUE, None)])
            .await
            .expect("the send crosses the tunnel and the node accepts it");
    assert!(
        reports
            .iter()
            .all(|report| matches!(report.route, TransmitRoute::Mixnet { .. })),
        "every step must report the mixnet route, found {:?}",
        reports
            .iter()
            .map(|report| &report.route)
            .collect::<Vec<_>>()
    );
}
