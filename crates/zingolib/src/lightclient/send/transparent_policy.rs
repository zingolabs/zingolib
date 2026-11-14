use crate::{
    lightclient::LightClient,
    lightclient::error::{LightClientError, SendError},
    testutils::lightclient::from_inputs,
    testutils::synthetic_wallet::SyntheticWalletBuilder,
    wallet::error::ProposeSendError,
};

#[tokio::test]
async fn t_incoming_t_outgoing_disallowed() {
    let value = 100_000;
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .transparent_coin(value)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let transaction = client
        .wallet()
        .read()
        .await
        .transaction_summaries(false)
        .await
        .unwrap()
        .0
        .first()
        .unwrap()
        .clone();
    // The builder confirms its first fabricated record at height 2.
    assert_eq!(transaction.blockheight, 2.into());
    assert_eq!(transaction.value, value);

    let sent_value = 20_000;
    let sent_transaction_error = from_inputs::quick_send(
        &mut client,
        vec![(zingo_test_vectors::EXT_TADDR, sent_value, None)],
    )
    .await
    .unwrap_err();
    assert!(matches!(
        sent_transaction_error,
        LightClientError::SendError(SendError::ProposeSendError(ProposeSendError::Proposal(
            zcash_client_backend::data_api::error::Error::InsufficientFunds {
                available: _,
                required: _
            }
        )))
    ));
}
