#[macro_export]
macro_rules! apply_scenario {
    ($test_implementation_fn:ident) => {
        let scenario = setup_n_block_fcbl_scenario($numblock).await;
        $test_implementation_fn(&mut scenario).await;
        clean_shutdown(scenario.stop_transmitter, scenario.test_server_handle).await;
    };
}

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
