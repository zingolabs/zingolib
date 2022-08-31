#[macro_export]
macro_rules! scenario_test {
    ($scenario:ident,
     $numblocks:tt,
     #[tokio::test]
     async fn $test_name:ident()
        $implementation:block
    ) => {
        #[tokio::test]
        async fn $test_name() {
            let $scenario = setup_n_block_fcbl_scenario($numblocks).await;
            $implementation
            clean_shutdown($scenario.stop_transmitter, $scenario.test_server_handle).await;
        }
    };
}
