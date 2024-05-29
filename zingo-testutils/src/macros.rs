/// Note that do_addresses returns an array, each element is a JSON representation
/// of a UA.  Legacy addresses can be extracted from the receivers, per:
/// <https://zips.z.cash/zip-0316>
#[macro_export]
macro_rules! get_base_address_macro {
    ($client:expr, $address_protocol:expr) => {
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

/// First check that each pools' balance matches an expectation
/// then check that the overall balance as calculated by
/// summing the amounts listed in tx_summaries matches the
/// sum of the balances.
#[macro_export]
macro_rules! check_client_balances {
    ($client:ident, o: $orchard:tt s: $sapling:tt t: $transparent:tt) => {
        let balance = $client.do_balance().await;
        assert_eq!(
            balance.orchard_balance.unwrap(),
            $orchard,
            "\no_balance: {} expectation: {} ",
            balance.orchard_balance.unwrap(),
            $orchard
        );
        assert_eq!(
            balance.sapling_balance.unwrap(),
            $sapling,
            "\ns_balance: {} expectation: {} ",
            balance.sapling_balance.unwrap(),
            $sapling
        );
        assert_eq!(
            balance.transparent_balance.unwrap(),
            $transparent,
            "\nt_balance: {} expectation: {} ",
            balance.transparent_balance.unwrap(),
            $transparent
        );
        let tx_summaries = $client.list_txsummaries().await;
        let tx_summary_balance = tx_summaries
            .iter()
            .map(|transfer| transfer.balance_delta())
            .sum::<i64>();
        assert_eq!(
            (balance.orchard_balance.unwrap()
                + balance.sapling_balance.unwrap()
                + balance.transparent_balance.unwrap()) as i64,
            tx_summary_balance,
            "do_list_transactions: {}\nlist_txsummaries: {}",
            ::json::JsonValue::from($client.do_list_transactions().await).pretty(4),
            ::json::JsonValue::from(tx_summaries).pretty(4)
        );
    };
}
