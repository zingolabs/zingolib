//! Direct JSON-RPC probes against the running validator.
//!
//! Every wallet-facing observation flows through the indexer, so
//! attribution experiments (is a surprising verdict zebra's, zainod's, or
//! the wallet's?) need a channel that bypasses the indexer entirely. The
//! running Zebrad exposes its JSON-RPC port via `rpc_listen_port()`, and
//! these helpers speak the methods the attribution cells and the
//! chain-cache replay mechanism need.
//!
//! A mempool rejection is DATA here, not an error: the probes exist to
//! compare verdicts, so [`RawTransactionVerdict::Rejected`] carries the
//! validator's message for predicate matching.
//!
//! Every call through this module is recorded in a per-test RPC ledger
//! (see [`ledger_snapshot`]). Together with the observability module's
//! state watches, the ledger closes the attribution loop: a chain
//! mutation with no matching write in the ledger came from outside the
//! test.

use std::time::Instant;

/// The validator's verdict on a directly-submitted raw transaction.
#[derive(Debug)]
pub enum RawTransactionVerdict {
    /// Accepted into the mempool. Carries the txid the validator returned.
    Accepted(String),
    /// Rejected. Carries the validator's error message.
    Rejected(String),
}

/// One outgoing JSON-RPC call this crate issued.
#[derive(Clone, Debug)]
pub struct LedgerEntry {
    /// When the call was issued.
    pub at: Instant,
    /// The JSON-RPC method name.
    pub method: String,
}

thread_local! {
    /// The per-test RPC ledger. Tests run single-threaded under
    /// `#[tokio::test]`, so thread-local scope IS test scope.
    static RPC_LEDGER: std::cell::RefCell<Vec<LedgerEntry>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Every JSON-RPC call this crate has issued from the current test, in
/// order. Chain-mutating methods are identified by [`is_write_method`].
pub fn ledger_snapshot() -> Vec<LedgerEntry> {
    RPC_LEDGER.with(|ledger| ledger.borrow().clone())
}

/// Whether a JSON-RPC method can mutate the regtest chain.
pub fn is_write_method(method: &str) -> bool {
    matches!(method, "submitblock" | "generate" | "generatetoaddress")
}

/// Issue a JSON-RPC call, recording it in the ledger. Returns `None`
/// on any transport or parse failure, since probes that poll through launch
/// and teardown windows want silence, not panics.
async fn try_rpc_call(
    rpc_port: u16,
    method: &str,
    params: serde_json::Value,
) -> Option<serde_json::Value> {
    RPC_LEDGER.with(|ledger| {
        ledger.borrow_mut().push(LedgerEntry {
            at: Instant::now(),
            method: method.to_string(),
        })
    });
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
        .ok()?
        .text()
        .await
        .ok()?;
    serde_json::from_str(&response_text).ok()
}

async fn rpc_call(rpc_port: u16, method: &str, params: serde_json::Value) -> serde_json::Value {
    try_rpc_call(rpc_port, method, params)
        .await
        .expect("validator JSON-RPC must be reachable and answer with JSON")
}

/// Non-panicking probe of the validator's best chain: (height, best
/// block hash). `None` when the validator is unreachable.
pub async fn try_get_chain_info(rpc_port: u16) -> Option<(u32, String)> {
    let response = try_rpc_call(rpc_port, "getblockchaininfo", serde_json::json!([])).await?;
    let result = response.get("result")?;
    let height = result.get("blocks")?.as_u64()? as u32;
    let hash = result.get("bestblockhash")?.as_str()?.to_string();
    Some((height, hash))
}

/// Non-panicking probe of the validator's connected peer count. On
/// regtest this must be zero. Anything else names a mutation channel
/// the isolation assumptions exclude.
pub async fn try_get_peer_count(rpc_port: u16) -> Option<usize> {
    let response = try_rpc_call(rpc_port, "getpeerinfo", serde_json::json!([])).await?;
    Some(response.get("result")?.as_array()?.len())
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

/// Returns the raw serialized block at `height` as hex (`getblock` at
/// verbosity 0). Chain-cache export reads setup chains out block by
/// block through this.
pub async fn get_block_hex(rpc_port: u16, height: u32) -> String {
    let response = rpc_call(
        rpc_port,
        "getblock",
        serde_json::json!([height.to_string(), 0]),
    )
    .await;
    response
        .get("result")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("getblock {height} must return a hex string: {response}"))
        .to_string()
}

/// Submits a raw serialized block (hex) via `submitblock`. Panics on any
/// verdict other than acceptance, since chain-cache replay resubmits blocks
/// the validator itself produced, so a rejection means the cache is
/// invalid, not that rejection is interesting data.
///
/// A `"duplicate"` verdict counts as acceptance: it means the validator
/// already holds this exact block, which is the replay's desired
/// postcondition. It occurs legitimately when a cached block collides
/// with the launch block (`Zebrad::launch` mines one block on regtest,
/// and transparent-pool regtest blocks are byte-deterministic, so the
/// cached and launch-mined block 1 can be the same block).
pub async fn submit_block(rpc_port: u16, block_hex: &str) {
    let response = rpc_call(rpc_port, "submitblock", serde_json::json!([block_hex])).await;
    let error = response.get("error").filter(|error| !error.is_null());
    // submitblock signals acceptance with a null result; any string
    // other than "duplicate" ("rejected", "inconclusive", ...) is a
    // refusal.
    let verdict = response
        .get("result")
        .filter(|result| !result.is_null() && result.as_str() != Some("duplicate"));
    assert!(
        error.is_none() && verdict.is_none(),
        "submitblock refused a cached block: {response}"
    );
}
