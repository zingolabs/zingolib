use zcash_protocol::PoolType;
use zcash_protocol::value::Zatoshis;
use zingolib::testutils::assertions::assert_null_data_send;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::NetworkedTestEnvironment;
use zingolib::testutils::lightclient::get_base_address;
use zingolib::wallet::transparent::OpReturnData;
use zip32::AccountId;

/// A payload under the 80-byte limit.
const PAYLOAD: &[u8] = b"zingolib op_return payload";
const AMOUNT: Zatoshis = Zatoshis::const_from_u64(100_000);

/// - Awaits real testnet blocks, syncing after each, until both steps confirm or the patience runs out.
#[tokio::test]
async fn propose_and_send_with_op_return_confirms_on_testnet() {
    let mut environment = NetworkedTestEnvironment::setup().await;
    let mut client = environment.create_faucet().await;
    let recipient_address = get_base_address(&client, PoolType::Transparent).await;
    let data = OpReturnData::new(PAYLOAD.to_vec()).expect("the payload is under the limit");

    let proposal = client
        .propose_send_with_op_return(&recipient_address, AMOUNT, data, AccountId::ZERO)
        .await
        .expect("the OP_RETURN proposal builds");
    let op_return_fee = proposal.op_return_fee();
    let deshield_fee = proposal
        .deshield_fee()
        .expect("the deshield step carries a fee");
    assert_eq!(
        proposal.total_fee().expect("both fees sum"),
        (deshield_fee + op_return_fee).expect("both fees sum"),
        "total fee is the sum of both transaction fees"
    );

    let txids = client
        .send_stored_proposal(false)
        .await
        .expect("both steps build and transmit to testnet");
    assert_eq!(txids.len(), 2, "a deshield and an OP_RETURN send are sent");
    let deshield_txid = txids[0];
    let op_return_txid = txids[1];

    assert!(
        environment
            .await_confirmed(&mut client, &[deshield_txid, op_return_txid])
            .await,
        "the network must confirm the deshield and the OP_RETURN send within the patience"
    );

    let wallet = client.wallet();
    let wallet = wallet.read().await;
    let bundle = wallet
        .wallet_transactions
        .get(&op_return_txid)
        .expect("OP_RETURN send recorded")
        .transaction()
        .transparent_bundle()
        .expect("OP_RETURN send is a transparent transaction");
    assert_null_data_send(bundle, PAYLOAD);
}
