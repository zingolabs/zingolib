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
        let value_transfers = $client.list_value_transfers().await;
        let value_transfer_balance = value_transfers
            .iter()
            .map(|transfer| transfer.balance_delta())
            .sum::<i64>();
        assert_eq!(
            (balance.orchard_balance.unwrap()
                + balance.sapling_balance.unwrap()
                + balance.transparent_balance.unwrap()) as i64,
            value_transfer_balance,
            "do_list_transactions: {}\nlist_value_transfers: {}",
            ::json::JsonValue::from($client.do_list_transactions().await).pretty(4),
            ::json::JsonValue::from(value_transfers).pretty(4)
        );
    };
}
/// Given a client and txid, get the outgoing metadata from the tr
#[macro_export]
macro_rules! get_otd {
    ($client:ident, $txid:ident) => {
        $client
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .transaction_records_by_id
            .get($txid)
            .unwrap()
            .outgoing_tx_data
            .clone()
    };
}
/// Specific to two tests, validate outgoing metadata before and after rescan
#[macro_export]
macro_rules! validate_otds {
    ($client:ident, $nom_txid:ident, $memo_txid:ident) => {
        let pre_rescan_summaries = $client.list_value_transfers().await;
        let pre_rescan_no_memo_self_send_outgoing_tx_data = get_otd!($client, $nom_txid);
        let pre_rescan_with_memo_self_send_outgoing_tx_data = get_otd!($client, $memo_txid);
        $client.do_rescan().await.unwrap();
        let post_rescan_summaries = $client.list_value_transfers().await;
        let post_rescan_no_memo_self_send_outgoing_tx_data = get_otd!($client, $nom_txid);
        let post_rescan_with_memo_self_send_outgoing_tx_data = get_otd!($client, $memo_txid);
        assert_eq!(
            pre_rescan_no_memo_self_send_outgoing_tx_data,
            post_rescan_no_memo_self_send_outgoing_tx_data
        );
        assert_eq!(
            pre_rescan_with_memo_self_send_outgoing_tx_data,
            post_rescan_with_memo_self_send_outgoing_tx_data
        );
        assert_eq!(pre_rescan_summaries, post_rescan_summaries);
    };
}
