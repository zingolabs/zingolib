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

// Note that do_addresses returns an array, each element is a JSON representation
// of a UA.  Legacy addresses can be extracted from the receivers, per:
// <https://zips.z.cash/zip-0316>
#[macro_export]
macro_rules! get_base_address {
    ($client:ident, $address_protocol:expr) => {
        match $address_protocol {
            "unified" => $client.do_addresses().await[0]["address"]
                .take()
                .to_string(),
            "sapling" => $client.do_addresses().await[0]["receivers"]["sapling"]
                .clone()
                .to_string(),
            "transparent" => $client.do_addresses().await[0]["receivers"]["transparent"]
                .clone()
                .to_string(),
            _ => "ERROR".to_string(),
        }
    };
}
#[macro_export]
macro_rules! check_client_balances {
    ($client:ident, o: $orchard:tt s: $sapling:tt t: $transparent:tt) => {
        let balance = $client.do_balance().await;
        assert_eq!(
            balance["orchard_balance"], $orchard,
            "\no_balance: {} expectation: {} ",
            balance["orchard_balance"], $orchard
        );
        assert_eq!(
            balance["sapling_balance"], $sapling,
            "\ns_balance: {} expectation: {} ",
            balance["sapling_balance"], $sapling
        );
        assert_eq!(
            balance["transparent_balance"], $transparent,
            "\nt_balance: {} expectation: {} ",
            balance["transparent_balance"], $transparent
        );
    };
}
