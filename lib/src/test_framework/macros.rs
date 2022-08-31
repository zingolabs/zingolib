#[macro_export]
macro_rules! scenario_test {
    ($($field:ident,)* $numblocks:literal,
     #[tokio::test]
     async fn $test_name:ident()
        $implementation:block
    ) => {
        #[tokio::test]
        async fn $test_name() {
            let NBlockFCBLScenario {
                stop_transmitter,
                test_server_handle,
                $($field,)*
                ..
            } = setup_n_block_fcbl_scenario($numblocks).await;
            $implementation
            clean_shutdown(stop_transmitter, test_server_handle).await;
        }
    };
}
