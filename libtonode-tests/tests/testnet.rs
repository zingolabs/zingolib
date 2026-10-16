#![forbid(unsafe_code)]
#![cfg(feature = "testnet")]
//! - Dials the public testnet indexer and spends from the shared GloryGoddess testnet wallet.
//! - Waits for real testnet blocks, so each test runs for minutes.

mod testnet {
    use zingolib::testutils::chain_generics::fixtures;
    use zingolib::testutils::chain_generics::networked::NetworkedTestEnvironment;

    #[tokio::test]
    async fn generate_a_range_of_value_transfers() {
        tracing_subscriber::fmt().init();
        fixtures::create_various_value_transfers::<NetworkedTestEnvironment>().await;
    }

    #[tokio::test]
    async fn send_shield_cycle() {
        tracing_subscriber::fmt().init();
        fixtures::send_shield_cycle::<NetworkedTestEnvironment>(1).await;
    }
}
