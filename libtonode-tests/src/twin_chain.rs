//! The live (LocalNet) implementation of
//! [`zingolib::testutils::twin_fixtures::TwinChain`]: the twin fixtures'
//! control-group environment, driving the same test bodies the mock
//! twins run against a real zebrad + zainod stack.

use zcash_local_net::validator::Validator;

use zingolib::lightclient::LightClient;
use zingolib::testutils::twin_fixtures::TwinChain;

use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};
use zingolib_testutils::setup_metrics::MeteredNet;

/// A LocalNet twin-fixture environment.
pub struct LiveTwinChain {
    /// The launched network (validator + indexer + metrics recorder).
    pub local_net: MeteredNet,
}

impl TwinChain for LiveTwinChain {
    async fn setup_faucet_recipient() -> (Self, LightClient, LightClient) {
        let (local_net, faucet, recipient) = scenarios::faucet_recipient_default().await;
        (Self { local_net }, faucet, recipient)
    }

    async fn setup_funded_recipient(initial: u64) -> (Self, LightClient, LightClient) {
        let (local_net, faucet, recipient, _txid) =
            scenarios::faucet_funded_recipient_default(initial).await;
        (Self { local_net }, faucet, recipient)
    }

    async fn bump(&mut self) {
        self.local_net
            .validator()
            .generate_blocks(1)
            .await
            .expect("generating one block succeeds");
    }

    async fn sync(&self, client: &mut LightClient) {
        scenarios::sync_client_to_validator_tip(&self.local_net, client).await;
    }

    async fn bump_and_sync(&mut self, client: &mut LightClient) {
        increase_height_and_wait_for_client(&self.local_net, client, 1)
            .await
            .expect("bumping the chain and syncing the client succeeds");
    }

    fn funded_setup_height(&self) -> u32 {
        scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 2
    }

    fn second_wave_faucet_fee(&self) -> u64 {
        // The live faucet's note pool is fragmented by earlier waves and
        // smallest-first selection makes the second funding send four
        // logical actions.
        20_000
    }
}
