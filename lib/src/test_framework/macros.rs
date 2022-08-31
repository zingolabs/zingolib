#[macro_export]
macro_rules! scenario_test {
    ($data:ident,
     $stop_transmitter:ident,
     $test_server_handle:ident,
     $lightclient:ident,
     $fake_compactblock_list:ident,
     $config:ident,
     $numblocks:tt,
     #[tokio::test]
     async fn $test_name:ident()
        $implementation:block
    ) => {
        #[tokio::test]
        async fn $test_name() {
            let NBlockFCBLScenario {
                $data,
                $stop_transmitter,
                $test_server_handle,
                $lightclient,
                $fake_compactblock_list,
                $config
            } = setup_n_block_fcbl_scenario($numblocks).await;
            $implementation
            clean_shutdown($stop_transmitter, $test_server_handle).await;
        }
    };
}
