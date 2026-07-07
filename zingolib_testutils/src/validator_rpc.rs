//! Direct JSON-RPC probes against the running validator.
//!
//! Every wallet-facing observation flows through the indexer, so
//! attribution experiments (is a surprising verdict zebra's, zainod's, or
//! the wallet's?) need a channel that bypasses the indexer entirely. The
//! running Zebrad exposes its JSON-RPC port via `rpc_listen_port()`;
//! these helpers speak the two methods the attribution cells need.
//!
//! A mempool rejection is DATA here, not an error: the probes exist to
//! compare verdicts, so [`RawTransactionVerdict::Rejected`] carries the
//! validator's message for predicate matching.

/// The validator's verdict on a directly-submitted raw transaction.
#[derive(Debug)]
pub enum RawTransactionVerdict {
    /// Accepted into the mempool; carries the txid the validator returned.
    Accepted(String),
    /// Rejected; carries the validator's error message.
    Rejected(String),
}

async fn rpc_call(rpc_port: u16, method: &str, params: serde_json::Value) -> serde_json::Value {
    let request_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
    .to_string();
    let response_text = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{rpc_port}"))
        .header("content-type", "application/json")
        .body(request_body)
        .send()
        .await
        .expect("validator JSON-RPC must be reachable")
        .text()
        .await
        .expect("validator JSON-RPC response must be readable");
    serde_json::from_str(&response_text).expect("validator JSON-RPC response must be JSON")
}

/// Submits raw transaction bytes directly to the validator's
/// `sendrawtransaction`, bypassing the indexer.
pub async fn send_raw_transaction(
    rpc_port: u16,
    transaction_bytes: &[u8],
) -> RawTransactionVerdict {
    let transaction_hex: String = transaction_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let response = rpc_call(
        rpc_port,
        "sendrawtransaction",
        serde_json::json!([transaction_hex]),
    )
    .await;
    if let Some(error) = response.get("error").filter(|error| !error.is_null()) {
        RawTransactionVerdict::Rejected(
            error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<no message>")
                .to_string(),
        )
    } else {
        RawTransactionVerdict::Accepted(
            response
                .get("result")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<no txid>")
                .to_string(),
        )
    }
}

/// Returns the txids (display-order hex) currently in the validator's
/// mempool, bypassing the indexer.
pub async fn get_raw_mempool(rpc_port: u16) -> Vec<String> {
    let response = rpc_call(rpc_port, "getrawmempool", serde_json::json!([])).await;
    response
        .get("result")
        .and_then(serde_json::Value::as_array)
        .expect("getrawmempool must return an array")
        .iter()
        .filter_map(|txid| txid.as_str().map(str::to_string))
        .collect()
}
