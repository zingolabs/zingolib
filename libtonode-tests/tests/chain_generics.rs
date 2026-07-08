#[cfg(feature = "chain_generic_tests")]
mod chain_generics {
    // The deterministic pool matrix that lived here migrated offline to
    // zingolib::lightclient::propose::pool_matrix: its fee, value, and
    // change assertions are pure proposer arithmetic over a synthetic
    // wallet. The two fixtures below still drive the full LocalNet round
    // trip (Transmitted -> Mempool -> Confirmed via follow_proposal), so
    // that machinery keeps chain-level coverage here.
    use libtonode_tests::chain_generics::LibtonodeEnvironment;

    use zingolib::testutils::chain_generics::fixtures;

    #[tokio::test]
    async fn generate_a_range_of_value_transfers() {
        fixtures::create_various_value_transfers::<LibtonodeEnvironment>().await;
    }
    #[tokio::test]
    async fn send_shield_cycle() {
        fixtures::send_shield_cycle::<LibtonodeEnvironment>(1).await;
    }
}
