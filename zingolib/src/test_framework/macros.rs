/// We use this macro to remove repetitive test set-up and teardown in the zingolib unit tests.
#[macro_export]
macro_rules! apply_scenario {
    ($inner_test_name:ident $numblocks:literal) => {
        concat_idents::concat_idents!(
            test_name = scenario_, $inner_test_name {
                #[tokio::test]
                async fn test_name() {
                    let (scenario, stop_transmitter, test_server_handle) = $crate::lightclient::test_server::setup_n_block_fcbl_scenario($numblocks).await;
                    $inner_test_name(scenario).await;
                    clean_shutdown(stop_transmitter, test_server_handle).await;
                }
            }
        );
    };
}
