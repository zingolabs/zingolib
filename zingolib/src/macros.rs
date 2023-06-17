/// Commands are highly repetitive we use this to avoid copy paste boilerplate
#[macro_export]
macro_rules! createcommand {
    (
        $command: tt
        $help_message: literal
        $short_help: literal
        $exec_expression: tt
    ) => {
        struct $command {}
        impl Command for $command {
            fn help(&self) -> &'static str {
                indoc! {$help_message}
            }

            fn short_help(&self) -> &'static str {
                $short_help
            }

            fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
                $exec_expression.to_string()
            }
        }
    };
}
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
        assert_eq!(
            balance["orchard_balance"].as_i64().unwrap_or(0)
                + balance["sapling_balance"].as_i64().unwrap_or(0)
                + balance["transparent_balance"].as_i64().unwrap_or(0),
            tx_summary_balance,
            "tx_summaries follow: {}\ndo_list_transactions follow: {}",
            ::json::JsonValue::from($client.do_list_txsummaries().await).pretty(4),
            $client.do_list_transactions().await.pretty(4)
        );
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
