#![forbid(unsafe_code)]
mod load_wallet {
    use zcash_local_net::validator::Validator as _;
    use zingolib::{get_base_address_macro, testutils::lightclient::from_inputs};
    use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};

    #[tokio::test(flavor = "multi_thread")]
    async fn verify_old_wallet_uses_server_height_in_send() {
        // An earlier version of zingolib used the _wallet's_ 'height' when
        // constructing transactions.  This worked well enough when the
        // client completed sync prior to sending, but when we introduced
        // interrupting send, it made it immediately obvious that this was
        // the wrong height to use!  The correct height is the
        // "mempool height" which is the server_height + 1
        let (local_net, mut faucet, recipient) = scenarios::faucet_recipient_default().await;
        // Ensure that the client has confirmed spendable funds
        increase_height_and_wait_for_client(&local_net, &mut faucet, 5)
            .await
            .unwrap();

        // Without sync push server forward 2 blocks
        local_net.validator().generate_blocks(2).await.unwrap();
        let client_fully_scanned_height = faucet
            .wallet()
            .read()
            .await
            .sync_state
            .fully_scanned_height()
            .unwrap();

        // Verify the wallet is still at the height it last synced to
        // (funded setup height + the 5 blocks synced above), not the
        // Validator's tip 2 blocks ahead.
        assert_eq!(
            client_fully_scanned_height,
            (scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 5).into()
        );

        // Interrupt generating send
        from_inputs::quick_send(
            &mut faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                10_000,
                Some("Interrupting sync!!"),
            )],
        )
        .await
        .unwrap();
    }
}
