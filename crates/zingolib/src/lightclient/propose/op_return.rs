use pepper_sync::keys::transparent::TransparentScope;
use zcash_protocol::TxId;
use zcash_protocol::value::Zatoshis;

use crate::data::proposal::{ZingoProposal, total_payment_amount};
use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, SendError};
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::error::WalletError;
use crate::wallet::keys::unified::ReceiverSelection;
use crate::wallet::transparent::OpReturnData;

const ACCOUNT: zip32::AccountId = zip32::AccountId::ZERO;
const PAYLOAD: &[u8] = b"zingolib op_return payload";

async fn client() -> LightClient {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(1_000_000)
        .build();
    LightClient::new_for_test(wallet).await
}

fn data() -> OpReturnData {
    OpReturnData::new(PAYLOAD.to_vec()).unwrap()
}

/// The proposal is stored as an OP_RETURN proposal. The deshield pays
/// the amount plus the OP_RETURN fee to the next Refund-scope address.
/// The total fee is the sum of both fees. The address is not reserved.
#[tokio::test]
async fn proposal_is_stored_and_reports_both_fees() {
    let mut client = client().await;
    let amount = Zatoshis::const_from_u64(100_000);

    let proposal = client
        .propose_send_with_op_return(zingo_test_vectors::EXT_TADDR, amount, data(), ACCOUNT)
        .await
        .unwrap();

    assert_eq!(proposal.amount(), amount);
    assert_eq!(proposal.data(), &data());
    assert!(u64::from(proposal.op_return_fee()) >= 10_000);
    assert_eq!(u64::from(proposal.op_return_fee()) % 5_000, 0);
    assert_eq!(
        total_payment_amount(proposal.deshield()).unwrap(),
        (amount + proposal.op_return_fee()).unwrap(),
        "the deshield pays the amount plus the OP_RETURN fee"
    );
    assert_eq!(
        proposal.total_fee().unwrap(),
        (proposal.deshield_fee().unwrap() + proposal.op_return_fee()).unwrap()
    );

    let mut wallet = client.wallet().write().await;
    let stored = wallet.take_proposal().expect("proposal stored");
    assert!(matches!(stored, ZingoProposal::OpReturn(_)));
    let (next_id, next_address) = wallet.derive_refund_addresses(1, ACCOUNT).unwrap()[0];
    assert_eq!(
        proposal.source_address(),
        &next_address,
        "the deshield pays the next Refund-scope address"
    );
    assert!(
        !wallet.transparent_addresses().contains_key(&next_id),
        "proposing does not reserve the address"
    );
}

/// A stored proposal whose deshield was transmitted is kept: proposing
/// again is refused until `send_stored_proposal` resumes it.
#[tokio::test]
async fn proposing_is_refused_while_a_deshielded_proposal_waits() {
    let mut client = client().await;
    let amount = Zatoshis::const_from_u64(100_000);
    let proposal = client
        .propose_send_with_op_return(zingo_test_vectors::EXT_TADDR, amount, data(), ACCOUNT)
        .await
        .expect("a funded wallet proposes to a transparent recipient");
    let deshield_txid = TxId::from_bytes([9; 32]);
    client
        .wallet()
        .write()
        .await
        .store_proposal(ZingoProposal::OpReturn(
            proposal.with_deshield_txid(deshield_txid),
        ));

    let refused = client
        .propose_send_with_op_return(zingo_test_vectors::EXT_TADDR, amount, data(), ACCOUNT)
        .await;

    assert!(matches!(
        refused,
        Err(LightClientError::SendError(SendError::OpReturnResumePending { deshield_txid: txid }))
            if txid == deshield_txid
    ));
    let stored = client
        .wallet()
        .write()
        .await
        .take_proposal()
        .expect("the refusal leaves the waiting proposal stored");
    assert!(matches!(
        stored,
        ZingoProposal::OpReturn(kept) if kept.deshield_txid() == Some(deshield_txid)
    ));
}

/// Proposing twice derives the same source address. Neither call
/// reserves it.
#[tokio::test]
async fn proposing_does_not_reserve_the_source_address() {
    let mut client = client().await;
    let amount = Zatoshis::const_from_u64(100_000);

    let first = client
        .propose_send_with_op_return(zingo_test_vectors::EXT_TADDR, amount, data(), ACCOUNT)
        .await
        .unwrap();
    let second = client
        .propose_send_with_op_return(zingo_test_vectors::EXT_TADDR, amount, data(), ACCOUNT)
        .await
        .unwrap();

    assert_eq!(first.source_address(), second.source_address());
    let wallet = client.wallet().read().await;
    assert!(
        wallet
            .transparent_addresses()
            .keys()
            .all(|id| id.scope() != TransparentScope::Refund),
        "no Refund-scope address was reserved"
    );
}

/// The network of the test wallet.
async fn network(client: &LightClient) -> zcash_protocol::consensus::NetworkType {
    use zcash_protocol::consensus::Parameters as _;
    client.wallet().read().await.chain_type().network_type()
}

/// A TEX recipient is accepted. The proposal pays the P2PKH hash it
/// wraps.
#[tokio::test]
async fn tex_recipient_is_accepted() {
    use zcash_address::ToAddress as _;
    use zcash_transparent::address::TransparentAddress;
    let mut client = client().await;
    let hash = [4u8; 20];
    let tex = zcash_address::ZcashAddress::from_tex(network(&client).await, hash).encode();

    let proposal = client
        .propose_send_with_op_return(&tex, Zatoshis::const_from_u64(100_000), data(), ACCOUNT)
        .await
        .unwrap();

    assert_eq!(
        proposal.recipient(),
        &TransparentAddress::PublicKeyHash(hash)
    );
}

/// A P2SH recipient is accepted.
#[tokio::test]
async fn p2sh_recipient_is_accepted() {
    use zcash_address::ToAddress as _;
    use zcash_transparent::address::TransparentAddress;
    let mut client = client().await;
    let hash = [5u8; 20];
    let p2sh =
        zcash_address::ZcashAddress::from_transparent_p2sh(network(&client).await, hash).encode();

    let proposal = client
        .propose_send_with_op_return(&p2sh, Zatoshis::const_from_u64(100_000), data(), ACCOUNT)
        .await
        .unwrap();

    assert_eq!(proposal.recipient(), &TransparentAddress::ScriptHash(hash));
}

/// A shielded recipient is refused. Nothing is stored.
#[tokio::test]
async fn shielded_recipient_is_refused() {
    let mut client = client().await;
    let unified = {
        let mut wallet = client.wallet().write().await;
        let (_, address) = wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), ACCOUNT)
            .unwrap();
        address.encode(&wallet.chain_type())
    };

    let result = client
        .propose_send_with_op_return(&unified, Zatoshis::const_from_u64(100_000), data(), ACCOUNT)
        .await;

    assert!(matches!(
        result,
        Err(LightClientError::SendError(SendError::OpReturn(
            WalletError::OpReturnRecipientNotTransparent
        )))
    ));
    assert!(client.wallet().write().await.take_proposal().is_none());
}

/// Insufficient shielded funds fail at the deshield proposal.
#[tokio::test]
async fn insufficient_funds_fail_at_the_deshield() {
    let mut client = client().await;

    let result = client
        .propose_send_with_op_return(
            zingo_test_vectors::EXT_TADDR,
            Zatoshis::const_from_u64(5_000_000),
            data(),
            ACCOUNT,
        )
        .await;

    assert!(matches!(
        result,
        Err(LightClientError::SendError(SendError::ProposeSendError(_)))
    ));
}

/// Offline calculation refuses an OP_RETURN proposal and keeps it
/// stored.
#[tokio::test]
async fn calculate_refuses_and_preserves_the_proposal() {
    let mut client = client().await;
    client
        .propose_send_with_op_return(
            zingo_test_vectors::EXT_TADDR,
            Zatoshis::const_from_u64(100_000),
            data(),
            ACCOUNT,
        )
        .await
        .unwrap();

    let result = client.calculate_stored_proposal().await;

    assert!(matches!(
        result,
        Err(LightClientError::SendError(
            SendError::OpReturnNotCalculable
        ))
    ));
    assert!(matches!(
        client.wallet().write().await.take_proposal(),
        Some(ZingoProposal::OpReturn(_))
    ));
}
