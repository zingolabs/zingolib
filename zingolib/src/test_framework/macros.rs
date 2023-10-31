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
        let tx_summary_balance = $client
            .do_list_txsummaries()
            .await
            .iter()
            .map(|transfer| transfer.balance_delta())
            .sum::<i64>();
        let balance_report = format!(
            "\n\
             observed orchard: {} expected orchard: {}\n\
             observed sapling: {} expected sapling: {}\n\
             observed transpa: {} expected transpa: {}\n",
            balance.orchard_balance.unwrap(),
            $orchard as i64,
            balance.sapling_balance.unwrap(),
            $sapling as i64,
            balance.transparent_balance.unwrap(),
            $transparent as i64,
        );
        let sum_of_balances = balance.orchard_balance.unwrap_or(0) as i64
            + balance.sapling_balance.unwrap_or(0) as i64
            + balance.transparent_balance.unwrap_or(0) as i64;
        assert_eq!(
            sum_of_balances, tx_summary_balance,
            "\nbalance_report: {}\nsum_of_balances: {} tx_summary_balance: {}",
            balance_report, sum_of_balances, tx_summary_balance
        );
        assert_eq!(
            balance.orchard_balance.unwrap(),
            $orchard,
            "{balance_report}",
        );
        assert_eq!(
            balance.sapling_balance.unwrap(),
            $sapling,
            "{balance_report}",
        );
        assert_eq!(
            balance.transparent_balance.unwrap(),
            $transparent,
            "{balance_report}",
        );
    };
}
