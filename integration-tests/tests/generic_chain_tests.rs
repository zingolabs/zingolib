use zingo_testutils::scenarios::setup;

struct LibToNodeChain {
    server_id: http::Uri,
    regtest_network: RegtestNetwork,
}

impl ChainTest for LibToNodeChain {
    // async fn setup() -> Self {
    //     todo!()
    // }

    async fn build_client_and_fund(
        &self,
        funds: u32,
        pool: zcash_client_backend::PoolType,
    ) -> zingolib::lightclient::LightClient {
        let mut sb = setup::ScenarioBuilder::build_configure_launch(
            Some(mine_to_pool),
            None,
            None,
            &regtest_network,
        )
        .await;
        let faucet = sb.client_builder.build_faucet(false, regtest_network).await;
    }

    async fn build_client(&self) -> zingolib::lightclient::LightClient {
        todo!()
    }

    async fn bump_chain(&self) {
        todo!()
    }
}
