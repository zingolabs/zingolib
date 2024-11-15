//! implementation of conduct chain for live chains

use crate::lightclient::LightClient;

use super::conduct_chain::ConductChain;

/// this is essentially a placeholder.
/// allows using existing ChainGeneric functions with TestNet wallets
pub struct LiveChain;

impl ConductChain for LiveChain {
    async fn setup() -> Self {
        Self {}
    }

    async fn create_faucet(&mut self) -> LightClient {
        unimplemented!()
    }

    fn zingo_config(&mut self) -> crate::config::ZingoConfig {
        todo!()
    }

    async fn bump_chain(&mut self) {
        // average block time is 75 seconds. we do this twice here to insist on a new block
        tokio::time::sleep(std::time::Duration::from_secs(150)).await;
    }

    fn get_chain_height(&mut self) -> u32 {
        unimplemented!()
    }
}
