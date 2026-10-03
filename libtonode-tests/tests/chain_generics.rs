#[cfg(feature = "chain_generic_tests")]
mod chain_generics {
    // The deterministic pool matrix that lived here migrated offline to
    // zingolib::lightclient::propose::pool_matrix: its fee, value, and
    // change assertions are pure proposer arithmetic over a synthetic
    // wallet. The two fixtures below still drive the LocalNet round trip
    // through follow_proposal (Transmitted -> Confirmed; both pass
    // test_mempool = false, so the Mempool-status leg is not asserted
    // here), keeping chain-level coverage of that machinery.
    use libtonode_tests::chain_generics::LibtonodeEnvironment;

    use zingolib::testutils::chain_generics::fixtures;

    #[tokio::test]
    async fn generate_a_range_of_value_transfers() {
        fixtures::create_various_value_transfers::<LibtonodeEnvironment>().await;
    }
    #[tokio::test]
    async fn send_shield_cycle() {
        tracing_subscriber::fmt().init();
        fixtures::send_shield_cycle::<LibtonodeEnvironment>(1).await;
    }
}
