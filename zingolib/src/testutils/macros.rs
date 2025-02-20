/// Note that do_addresses returns an array, each element is a JSON representation
/// of a UA.  Legacy addresses can be extracted from the receivers, per:
/// <https://zips.z.cash/zip-0316>
#[macro_export]
macro_rules! get_base_address_macro {
    ($client:expr, $address_protocol:expr) => {
        match $address_protocol {
            "unified" => $client.do_addresses($crate::UAReceivers::All).await[0]["address"]
                .take()
                .to_string(),
            "sapling" => $client.do_addresses($crate::UAReceivers::All).await[0]["receivers"]
                ["sapling"]
                .clone()
                .to_string(),
            "transparent" => $client.do_addresses($crate::UAReceivers::All).await[0]["receivers"]
                ["transparent"]
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
        use zingolib::wallet::data::summaries::TransactionSummaryInterface as _;

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
        let summaries = $client.transaction_summaries().await;
        let summaries_balance = summaries
            .iter()
            .map(|summary| {
                summary
                    .balance_delta()
                    .unwrap_or_else(|| panic!("field not correctly populated"))
            })
            .sum::<i64>();
        assert_eq!(
            (balance.orchard_balance.unwrap()
                + balance.sapling_balance.unwrap()
                + balance.transparent_balance.unwrap()) as i64,
            summaries_balance,
            "transaction_summaries: {}",
            summaries
        );
    };
}
/// Specific to two tests, validate outgoing notes before and after rescan
#[macro_export]
macro_rules! validate_outgoing_notes {
    ($client:ident, $nom_txid:ident, $memo_txid:ident) => {
        let wallet = $client.wallet.lock().await;
        let pre_rescan_summaries = wallet.transaction_summaries().await;
        let pre_rescan_no_memo_self_send_outgoing_sapling_notes = wallet
            .wallet_transactions
            .get($nom_txid)
            .unwrap()
            .outgoing_sapling_notes()
            .clone();
        let pre_rescan_with_memo_self_send_outgoing_sapling_notes = wallet
            .wallet_transactions
            .get($memo_txid)
            .unwrap()
            .outgoing_sapling_notes()
            .clone();
        let pre_rescan_no_memo_self_send_outgoing_orchard_notes = wallet
            .wallet_transactions
            .get($nom_txid)
            .unwrap()
            .outgoing_orchard_notes()
            .clone();
        let pre_rescan_with_memo_self_send_outgoing_orchard_notes = wallet
            .wallet_transactions
            .get($memo_txid)
            .unwrap()
            .outgoing_orchard_notes()
            .clone();
        $client.do_rescan().await.unwrap();
        let post_rescan_no_memo_self_send_outgoing_sapling_notes = wallet
            .wallet_transactions
            .get($nom_txid)
            .unwrap()
            .outgoing_sapling_notes()
            .clone();
        let post_rescan_with_memo_self_send_outgoing_sapling_notes = wallet
            .wallet_transactions
            .get($memo_txid)
            .unwrap()
            .outgoing_sapling_notes()
            .clone();
        let post_rescan_no_memo_self_send_outgoing_orchard_notes = wallet
            .wallet_transactions
            .get($nom_txid)
            .unwrap()
            .outgoing_orchard_notes()
            .clone();
        let post_rescan_with_memo_self_send_outgoing_orchard_notes = wallet
            .wallet_transactions
            .get($memo_txid)
            .unwrap()
            .outgoing_orchard_notes()
            .clone();
        assert_eq!(
            pre_rescan_no_memo_self_send_outgoing_sapling_notes,
            post_rescan_no_memo_self_send_outgoing_sapling_notes
        );
        assert_eq!(
            pre_rescan_with_memo_self_send_outgoing_sapling_notes,
            post_rescan_with_memo_self_send_outgoing_sapling_notes
        );
        assert_eq!(
            pre_rescan_no_memo_self_send_outgoing_orchard_notes,
            post_rescan_no_memo_self_send_outgoing_orchard_notes
        );
        assert_eq!(
            pre_rescan_with_memo_self_send_outgoing_orchard_notes,
            post_rescan_with_memo_self_send_outgoing_orchard_notes
        );
        for summary in pre_rescan_summaries.iter() {
            zingolib::testutils::assert_transaction_summary_exists(&$client, summary).await;
        }
    };
}
