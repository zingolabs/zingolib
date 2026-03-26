//! TODO

/// First check that each pools' balance matches an expectation
/// then check that the overall balance as calculated by
/// summing the amounts listed in `tx_summaries` matches the
/// sum of the balances.
#[macro_export]
macro_rules! check_client_balances {
    ($client:ident, o: $orchard:tt s: $sapling:tt t: $transparent:tt) => {
        let balance = $client
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(
            balance.total_orchard_balance.unwrap().into_u64(),
            $orchard,
            "\no_balance: {} expectation: {} ",
            balance.total_orchard_balance.unwrap().into_u64(),
            $orchard
        );
        assert_eq!(
            balance.total_sapling_balance.unwrap().into_u64(),
            $sapling,
            "\ns_balance: {} expectation: {} ",
            balance.total_sapling_balance.unwrap().into_u64(),
            $sapling
        );
        assert_eq!(
            balance.confirmed_transparent_balance.unwrap().into_u64(),
            $transparent,
            "\nt_balance: {} expectation: {} ",
            balance.confirmed_transparent_balance.unwrap().into_u64(),
            $transparent
        );
    };
}
