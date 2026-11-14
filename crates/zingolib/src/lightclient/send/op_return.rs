use pepper_sync::keys::transparent::TransparentScope;
use zcash_primitives::transaction::TxId;
use zcash_protocol::value::Zatoshis;

use crate::data::proposal::{OpReturnProposal, ZingoProposal};
use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, SendError};
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::error::WalletError;
use crate::wallet::transparent::OpReturnData;

const ACCOUNT: zip32::AccountId = zip32::AccountId::ZERO;

async fn client() -> LightClient {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(1_000_000)
        .build();
    LightClient::new_for_test(wallet).await
}

async fn proposal(client: &mut LightClient) -> OpReturnProposal {
    client
        .create_op_return_proposal(
            zingo_test_vectors::EXT_TADDR,
            Zatoshis::const_from_u64(100_000),
            OpReturnData::new(b"payload".to_vec()).unwrap(),
            ACCOUNT,
        )
        .await
        .unwrap()
}

async fn reserved_refund_addresses(client: &LightClient) -> usize {
    client
        .wallet()
        .read()
        .await
        .transparent_addresses()
        .keys()
        .filter(|id| id.scope() == TransparentScope::Refund)
        .count()
}

/// A proposal whose source address another send reserved is refused.
/// Nothing is reserved or sent. The other reservation is untouched.
#[tokio::test]
async fn stale_source_address_is_refused() {
    let mut client = client().await;
    let proposal = proposal(&mut client).await;
    client
        .wallet()
        .write()
        .await
        .generate_refund_addresses(1, ACCOUNT)
        .unwrap();

    let result = client.send_op_return(proposal).await;

    assert!(matches!(
        result,
        Err(LightClientError::SendError(
            SendError::OpReturnSourceAddressStale
        ))
    ));
    assert_eq!(reserved_refund_addresses(&client).await, 1);
}

/// A deshield that fails before transmission releases the
/// reservation. An Indexerless client fails at preflight.
#[tokio::test]
async fn deshield_failure_before_transmission_releases_the_reservation() {
    let mut client = client().await;
    let proposal = proposal(&mut client).await;

    let result = client.send_op_return(proposal).await;

    assert!(matches!(result, Err(LightClientError::Offline)));
    assert_eq!(reserved_refund_addresses(&client).await, 0);
}

/// A proposal with a deshield txid skips the reservation and the
/// deshield. A failure in the OP_RETURN step stores the proposal
/// again with that txid and reports it.
#[tokio::test]
async fn resume_skips_the_deshield_and_reports_the_txid_on_failure() {
    let mut client = client().await;
    let deshield_txid = TxId::from_bytes([9u8; 32]);
    let proposal = proposal(&mut client)
        .await
        .with_deshield_txid(deshield_txid);

    let result = client.send_op_return(proposal).await;

    match result {
        Err(LightClientError::SendError(SendError::OpReturnAfterDeshield {
            deshield_txid: reported,
            op_return_txid,
            source,
        })) => {
            assert_eq!(reported, deshield_txid);
            assert_eq!(op_return_txid, None);
            assert!(matches!(
                *source,
                LightClientError::SendError(SendError::OpReturn(WalletError::TransactionNotFound(
                    _
                )))
            ));
        }
        other => panic!("expected OpReturnAfterDeshield, got {other:?}"),
    }
    assert_eq!(reserved_refund_addresses(&client).await, 0);
    match client.wallet().write().await.take_proposal() {
        Some(ZingoProposal::OpReturn(stored)) => {
            assert_eq!(stored.deshield_txid(), Some(deshield_txid));
        }
        other => panic!("expected the proposal stored again, got {other:?}"),
    }
}

/// The stale check and the release do not apply to a resumed send.
/// Another reservation made after the deshield does not refuse it.
#[tokio::test]
async fn resume_ignores_the_stale_check() {
    let mut client = client().await;
    let proposal = proposal(&mut client)
        .await
        .with_deshield_txid(TxId::from_bytes([9u8; 32]));
    client
        .wallet()
        .write()
        .await
        .generate_refund_addresses(1, ACCOUNT)
        .unwrap();

    let result = client.send_op_return(proposal).await;

    assert!(matches!(
        result,
        Err(LightClientError::SendError(
            SendError::OpReturnAfterDeshield { .. }
        ))
    ));
    assert_eq!(reserved_refund_addresses(&client).await, 1);
}
