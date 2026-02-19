//! TODO: Add Mod Description Here!

use std::time::Duration;

use tonic::Request;

use zcash_client_backend::proto::service::{BlockId, ChainSpec, Empty, LightdInfo, RawTransaction};

#[cfg(feature = "testutils")]
use zcash_client_backend::proto::service::TreeState;

pub const DEFAULT_GRPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Get server info.
pub async fn get_info(uri: http::Uri) -> Result<LightdInfo, String> {
    let mut client = crate::grpc_client::get_zcb_client(uri.clone())
        .await
        .map_err(|e| format!("Error getting client: {e:?}"))?;

    let mut request = Request::new(Empty {});
    request.set_timeout(DEFAULT_GRPC_TIMEOUT);

    let response = client
        .get_lightd_info(request)
        .await
        .map_err(|e| format!("Error with get_lightd_info response at {uri}: {e:?}"))?;
    Ok(response.into_inner())
}

/// TODO: Add Doc Comment Here!
#[cfg(feature = "testutils")]
pub async fn get_trees(uri: http::Uri, height: u64) -> Result<TreeState, String> {
    let mut client = crate::grpc_client::get_zcb_client(uri.clone())
        .await
        .map_err(|e| format!("Error getting client: {e:?}"))?;

    let b = BlockId {
        height,
        hash: vec![],
    };
    let response = client
        .get_tree_state(Request::new(b))
        .await
        .map_err(|e| format!("Error with get_tree_state response at {uri}: {e:?}"))?;

    Ok(response.into_inner())
}

/// `get_latest_block` GRPC call
pub async fn get_latest_block(uri: http::Uri) -> Result<BlockId, String> {
    let mut client = crate::grpc_client::get_zcb_client(uri.clone())
        .await
        .map_err(|e| format!("Error getting client: {e:?}"))?;

    let mut request = Request::new(ChainSpec {});
    request.set_timeout(DEFAULT_GRPC_TIMEOUT);

    let response = client
        .get_latest_block(request)
        .await
        .map_err(|e| format!("Error with get_latest_block response at {uri}: {e:?}"))?;

    Ok(response.into_inner())
}

pub(crate) async fn send_transaction(
    uri: http::Uri,
    transaction_bytes: Box<[u8]>,
) -> Result<String, String> {
    let mut client = crate::grpc_client::get_zcb_client(uri)
        .await
        .map_err(|e| format!("Error getting client: {e:?}"))?;

    let mut request = Request::new(RawTransaction {
        data: transaction_bytes.to_vec(),
        height: 0,
    });
    request.set_timeout(DEFAULT_GRPC_TIMEOUT);

    let response = client
        .send_transaction(request)
        .await
        .map_err(|e| format!("Send Error: {e}"))?;

    let sendresponse = response.into_inner();
    if sendresponse.error_code == 0 {
        let mut transaction_id = sendresponse.error_message;
        if transaction_id.starts_with('\"') && transaction_id.ends_with('\"') {
            transaction_id = transaction_id[1..transaction_id.len() - 1].to_string();
        }

        Ok(transaction_id)
    } else {
        Err(format!("Error: {sendresponse:?}"))
    }
}
