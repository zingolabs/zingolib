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
