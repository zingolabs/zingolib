use std::path::PathBuf;
use std::time::{Duration, Instant};

use zcash_protocol::{PoolType, ShieldedPool};
use zingolib::lightclient::send::TransmitRoute;
use zingolib::mixnet::Indicator;
use zingolib::mixnet::provision::NYM_PROXY_ENV;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::NetworkedTestEnvironment;
use zingolib::testutils::lightclient::{from_inputs, get_base_address};

const PAYMENT: u64 = 20_000;
const READINESS_LIMIT: Duration = Duration::from_secs(300);
const READINESS_POLL: Duration = Duration::from_secs(1);

/// - Spawns the nym-proxy binary that `ZINGO_NYM_PROXY` names and attaches the wallet to it.
/// - Sends over the real tunnel to the testnet indexer.
#[tokio::test]
async fn a_mixnet_send_crosses_a_real_proxy_to_the_node() {
    let proxy = PathBuf::from(
        std::env::var_os(NYM_PROXY_ENV)
            .unwrap_or_else(|| panic!("{NYM_PROXY_ENV} must name the nym-proxy binary")),
    );
    let mut environment = NetworkedTestEnvironment::setup().await;
    let mut client = environment.create_faucet().await;
    client
        .enable_mixnet(&proxy)
        .await
        .expect("the proxy spawns and the wallet attaches to it");
    let deadline = Instant::now() + READINESS_LIMIT;
    while client.read_mixnet_indicator() != Indicator::Ready {
        assert!(
            Instant::now() < deadline,
            "the mixnet did not reach Ready within {READINESS_LIMIT:?}: {:?}",
            client.read_mixnet_indicator()
        );
        tokio::time::sleep(READINESS_POLL).await;
    }

    let own_address = get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let reports =
        from_inputs::quick_send_reported(&mut client, vec![(&own_address, PAYMENT, None)])
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
