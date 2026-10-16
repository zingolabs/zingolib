use zcash_protocol::PoolType;
use zcash_protocol::value::Zatoshis;
use zingolib::lightclient::LightClient;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::NetworkedTestEnvironment;
use zingolib::testutils::lightclient::get_base_address;
use zingolib::wallet::transparent::OpReturnData;
use zip32::AccountId;

/// A payload under the 80-byte limit.
const PAYLOAD: &[u8] = b"zingolib op_return payload";
const AMOUNT: Zatoshis = Zatoshis::const_from_u64(100_000);
const OP_RETURN_OPCODE: u8 = 0x6a;

async fn both_confirmed(
    client: &LightClient,
    txids: &[zcash_primitives::transaction::TxId],
) -> bool {
    let wallet = client.wallet().read().await;
    txids.iter().all(|txid| {
        wallet
            .wallet_transactions
            .get(txid)
            .is_some_and(|transaction| transaction.status().is_confirmed())
    })
}

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

    for _ in 0..environment.confirmation_patience_blocks() {
        environment.increase_chain_height().await;
        environment.sync_client_to_tip(&mut client).await;
        if both_confirmed(&client, &[deshield_txid, op_return_txid]).await {
            break;
        }
    }
    assert!(
        both_confirmed(&client, &[deshield_txid, op_return_txid]).await,
        "the network must confirm the deshield and the OP_RETURN send within the patience"
    );

    let wallet = client.wallet();
    let wallet = wallet.read().await;
    let op_return_tx = wallet
        .wallet_transactions
        .get(&op_return_txid)
        .expect("OP_RETURN send recorded");
    let bundle = op_return_tx
        .transaction()
        .transparent_bundle()
        .expect("OP_RETURN send is a transparent transaction");
    assert_eq!(bundle.vin.len(), 1, "spends the single deshield output");
    assert_eq!(
        bundle.vout.len(),
        2,
        "recipient + OP_RETURN, no change output"
    );
    let op_return_out = bundle
        .vout
        .iter()
        .find(|out| out.value() == Zatoshis::ZERO)
        .expect("a zero-value OP_RETURN output");
    let script = op_return_out.script_pubkey().0.0.clone();
    assert_eq!(
        script[0], OP_RETURN_OPCODE,
        "null-data script starts with OP_RETURN"
    );
    assert!(
        script.windows(PAYLOAD.len()).any(|w| w == PAYLOAD),
        "the OP_RETURN carries the payload verbatim"
    );
}
