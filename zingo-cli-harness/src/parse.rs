//! Parsers for zingo-cli's one-shot command output.
//!
//! Each parser mirrors what the binary prints today; the unit tests
//! below pin them against recorded shapes, and the proof-scenario
//! integration test pins the shapes against the real binary. One-shot
//! stdout can carry non-JSON noise around the command response (e.g.
//! `Creating a new wallet` on first run), so the JSON parsers slice
//! from the first opening bracket to the last closing one rather than
//! parsing stdout whole.

use zcash_local_net::error::WalletError;
use zcash_local_net::wallet::{AddressReceiver, GetInfo};

/// The command response as a JSON value, sliced out of possibly noisy
/// stdout.
fn json_payload(
    operation: &'static str,
    stdout: &str,
    open: char,
    close: char,
) -> Result<serde_json::Value, WalletError> {
    let unexpected = |reason: String| WalletError::UnexpectedOutput {
        operation,
        reason,
        stdout: stdout.to_string(),
    };
    let start = stdout
        .find(open)
        .ok_or_else(|| unexpected(format!("no `{open}` in output")))?;
    let end = stdout
        .rfind(close)
        .ok_or_else(|| unexpected(format!("no `{close}` in output")))?;
    if end < start {
        return Err(unexpected(format!("`{close}` precedes `{open}`")));
    }
    serde_json::from_str(&stdout[start..=end])
        .map_err(|e| unexpected(format!("payload is not valid JSON: {e}")))
}

/// A `u64` field of a JSON object.
fn u64_field(
    operation: &'static str,
    stdout: &str,
    value: &serde_json::Value,
    field: &str,
) -> Result<u64, WalletError> {
    value[field]
        .as_u64()
        .ok_or_else(|| WalletError::UnexpectedOutput {
            operation,
            reason: format!("missing or non-integer `{field}`"),
            stdout: stdout.to_string(),
        })
}

/// The txid from a `{"txids": […]}` response (quicksend/quickshield).
/// The harness drives single-step proposals, so exactly one txid is
/// the contract; more than one means the operation did something the
/// caller cannot reason about.
pub(crate) fn single_txid(operation: &'static str, stdout: &str) -> Result<String, WalletError> {
    let payload = json_payload(operation, stdout, '{', '}')?;
    let txids = payload["txids"]
        .as_array()
        .ok_or_else(|| WalletError::UnexpectedOutput {
            operation,
            reason: "missing `txids` array".to_string(),
            stdout: stdout.to_string(),
        })?;
    match txids.as_slice() {
        [single] => {
            single
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| WalletError::UnexpectedOutput {
                    operation,
                    reason: "txid is not a string".to_string(),
                    stdout: stdout.to_string(),
                })
        }
        other => Err(WalletError::UnexpectedOutput {
            operation,
            reason: format!("expected exactly one txid, got {}", other.len()),
            stdout: stdout.to_string(),
        }),
    }
}

/// The per-pool zatoshi values from the `balance` command's bracketed
/// listing (underscore-grouped integers, one `key: value` line per
/// field).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PoolBalances {
    pub(crate) confirmed_orchard: u64,
    pub(crate) total_orchard: u64,
    pub(crate) confirmed_sapling: u64,
    pub(crate) total_sapling: u64,
    pub(crate) confirmed_transparent: u64,
    pub(crate) total_transparent: u64,
}

impl PoolBalances {
    /// Total balance across all pools, including unconfirmed funds.
    pub(crate) fn total(&self) -> u64 {
        self.total_orchard + self.total_sapling + self.total_transparent
    }
}

/// Parse the `balance` command's display listing.
///
/// A view-limited wallet prints `no view capability` for pools it
/// cannot see; harness wallets are seed-restored with full capability,
/// so that string (or any other non-integer) is contract drift, not a
/// value to interpret.
pub(crate) fn account_balance(
    operation: &'static str,
    stdout: &str,
) -> Result<PoolBalances, WalletError> {
    let field =
        |key: &str| -> Result<u64, WalletError> {
            let line = stdout
                .lines()
                .find(|line| line.trim_start().starts_with(key))
                .ok_or_else(|| WalletError::UnexpectedOutput {
                    operation,
                    reason: format!("no `{key}` line in balance output"),
                    stdout: stdout.to_string(),
                })?;
            let (_, value) = line
                .split_once(':')
                .ok_or_else(|| WalletError::UnexpectedOutput {
                    operation,
                    reason: format!("`{key}` line has no `:`"),
                    stdout: stdout.to_string(),
                })?;
            value.trim().replace('_', "").parse::<u64>().map_err(|_| {
                WalletError::UnexpectedOutput {
                    operation,
                    reason: format!("`{key}` value is not an integer: {}", value.trim()),
                    stdout: stdout.to_string(),
                }
            })
        };
    Ok(PoolBalances {
        confirmed_orchard: field("confirmed_orchard_balance")?,
        total_orchard: field("total_orchard_balance")?,
        confirmed_sapling: field("confirmed_sapling_balance")?,
        total_sapling: field("total_sapling_balance")?,
        confirmed_transparent: field("confirmed_transparent_balance")?,
        total_transparent: field("total_transparent_balance")?,
    })
}

/// The height from the `height` command's `{"height": N}` response.
pub(crate) fn height(operation: &'static str, stdout: &str) -> Result<u32, WalletError> {
    let payload = json_payload(operation, stdout, '{', '}')?;
    let height = u64_field(operation, stdout, &payload, "height")?;
    u32::try_from(height).map_err(|_| WalletError::UnexpectedOutput {
        operation,
        reason: format!("height {height} exceeds u32"),
        stdout: stdout.to_string(),
    })
}

/// The fields of [`GetInfo`] from the `info` command's JSON response.
pub(crate) fn get_info(operation: &'static str, stdout: &str) -> Result<GetInfo, WalletError> {
    let payload = json_payload(operation, stdout, '{', '}')?;
    let string_field = |field: &str| -> Result<String, WalletError> {
        payload[field]
            .as_str()
            .map(ToString::to_string)
            .ok_or_else(|| WalletError::UnexpectedOutput {
                operation,
                reason: format!("missing or non-string `{field}`"),
                stdout: stdout.to_string(),
            })
    };
    Ok(GetInfo {
        server_uri: string_field("server_uri")?,
        chain_name: string_field("chain_name")?,
        chain_tip_height: u64_field(operation, stdout, &payload, "latest_block_height")?,
    })
}

/// The lowest-index unified address from the `addresses` command's
/// JSON array.
pub(crate) fn first_unified_address(
    operation: &'static str,
    stdout: &str,
) -> Result<String, WalletError> {
    let payload = json_payload(operation, stdout, '[', ']')?;
    let entries = payload
        .as_array()
        .ok_or_else(|| WalletError::UnexpectedOutput {
            operation,
            reason: "addresses payload is not an array".to_string(),
            stdout: stdout.to_string(),
        })?;
    entries
        .iter()
        .min_by_key(|entry| entry["address_index"].as_u64().unwrap_or(u64::MAX))
        .and_then(|entry| entry["encoded_address"].as_str())
        .map(ToString::to_string)
        .ok_or_else(|| WalletError::UnexpectedOutput {
            operation,
            reason: "no entry with a string `encoded_address`".to_string(),
            stdout: stdout.to_string(),
        })
}

/// Re-encode the requested `receiver` of the given unified address.
///
/// zingo-cli exposes the wallet's unified addresses; the bare
/// per-receiver encodings the trait offers are derived here by
/// decoding the UA and re-encoding the selected receiver, so no
/// additional binary invocation shape enters the output contract.
pub(crate) fn receiver_from_unified(
    operation: &'static str,
    unified: &str,
    receiver: AddressReceiver,
) -> Result<String, WalletError> {
    use zcash_address::unified::{self, Container as _, Encoding as _, Receiver};
    use zcash_address::{ToAddress as _, ZcashAddress};

    let unexpected = |reason: String| WalletError::UnexpectedOutput {
        operation,
        reason,
        stdout: unified.to_string(),
    };
    let (net, address) = unified::Address::decode(unified)
        .map_err(|e| unexpected(format!("cannot decode unified address: {e}")))?;
    let mut items = address.items();
    match receiver {
        AddressReceiver::Unified => Ok(unified.to_string()),
        AddressReceiver::Transparent => items
            .iter()
            .find_map(|item| match item {
                Receiver::P2pkh(data) => {
                    Some(ZcashAddress::from_transparent_p2pkh(net, *data).to_string())
                }
                _ => None,
            })
            .ok_or_else(|| unexpected("no transparent receiver in unified address".to_string())),
        AddressReceiver::Sapling => items
            .iter()
            .find_map(|item| match item {
                Receiver::Sapling(data) => Some(ZcashAddress::from_sapling(net, *data).to_string()),
                _ => None,
            })
            .ok_or_else(|| unexpected("no sapling receiver in unified address".to_string())),
        AddressReceiver::Orchard => {
            items.retain(|item| matches!(item, Receiver::Orchard(_)));
            if items.is_empty() {
                return Err(unexpected(
                    "no orchard receiver in unified address".to_string(),
                ));
            }
            let orchard_only = unified::Address::try_from_items(items)
                .map_err(|e| unexpected(format!("cannot build orchard-only UA: {e}")))?;
            Ok(ZcashAddress::from_unified(net, orchard_only).to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded shape of `quicksend`/`quickshield` output.
    const TXIDS: &str = r#"{
  "txids": [
    "d0b02fd2ba9c4a9646cbcbaa2b0d0a70ce1e13a1148f22fbe3439fbe0a03ba24"
  ]
}"#;

    #[test]
    fn single_txid_from_recorded_shape() {
        assert_eq!(
            single_txid("send", TXIDS).unwrap(),
            "d0b02fd2ba9c4a9646cbcbaa2b0d0a70ce1e13a1148f22fbe3439fbe0a03ba24"
        );
    }

    #[test]
    fn multiple_txids_are_contract_drift() {
        let two = r#"{"txids": ["aa", "bb"]}"#;
        assert!(matches!(
            single_txid("send", two),
            Err(WalletError::UnexpectedOutput { .. })
        ));
    }

    /// Recorded shape of the `balance` display listing
    /// (`AccountBalance`'s `Display` impl: underscore-grouped zats).
    const BALANCE: &str = "[
    confirmed_orchard_balance: 625_000_000
    unconfirmed_orchard_balance: 0
    total_orchard_balance: 625_000_000

    confirmed_sapling_balance: 0
    unconfirmed_sapling_balance: 0
    total_sapling_balance: 0

    confirmed_transparent_balance: 1_250_000
    unconfirmed_transparent_balance: 0
    total_transparent_balance: 1_250_000
]";

    #[test]
    fn account_balance_from_recorded_shape() {
        let pools = account_balance("balance", BALANCE).unwrap();
        assert_eq!(pools.confirmed_orchard, 625_000_000);
        assert_eq!(pools.total_orchard, 625_000_000);
        assert_eq!(pools.confirmed_sapling, 0);
        assert_eq!(pools.confirmed_transparent, 1_250_000);
        assert_eq!(pools.total(), 626_250_000);
    }

    #[test]
    fn view_limited_balance_is_contract_drift() {
        let no_view = BALANCE.replace("625_000_000", "no view capability");
        assert!(matches!(
            account_balance("balance", &no_view),
            Err(WalletError::UnexpectedOutput { .. })
        ));
    }

    #[test]
    fn height_from_recorded_shape() {
        assert_eq!(height("balance", "{\n  \"height\": 5\n}").unwrap(), 5);
    }

    /// Recorded shape of `info` output (`do_info`), with startup noise
    /// preceding the payload as one-shot stdout can carry.
    const INFO: &str = r#"Creating a new wallet
{
  "version": "0.6.0",
  "git_commit": "",
  "server_uri": "http://127.0.0.1:20000/",
  "vendor": "",
  "taddr_support": true,
  "chain_name": "regtest",
  "sapling_activation_height": 1,
  "consensus_branch_id": "",
  "latest_block_height": 6
}"#;

    #[test]
    fn get_info_from_recorded_shape() {
        let info = get_info("get_info", INFO).unwrap();
        assert_eq!(info.server_uri, "http://127.0.0.1:20000/");
        assert_eq!(info.chain_name, "regtest");
        assert_eq!(info.chain_tip_height, 6);
    }

    /// Recorded shape of `addresses` output.
    const ADDRESSES: &str = r#"[
  {
    "account": 0,
    "address_index": 0,
    "has_orchard": true,
    "has_sapling": true,
    "has_transparent": true,
    "encoded_address": "uregtest1zkuzfv5m3yhv2j4fmvq5rjurkxenxyq8r7h4daun2zkznrjaa8ra8asgdm8wwgwjvlwwrxx7347r8w0ee6dqyw4rufw4wg9djwcr6frzkezmdw6dud3wsm99eany5r8wgsctlxquu009nzd6hsme2tcsk0v3sgjvxa70er7h27z5epr67p5q767s2z5gt88paru56mxpm6pwz0zd5ymtmu5t2wcfd924luqsgcz6rkxlsphkqavkmqrmfxa2p54ertx2kew8xls2xnna8n2rmsdw8yyxdamnd0gcwwkxpwss9c8v"
  }
]"#;

    #[test]
    fn first_unified_address_from_recorded_shape() {
        let address = first_unified_address("address", ADDRESSES).unwrap();
        assert!(address.starts_with("uregtest1"));
    }

    #[test]
    fn lowest_index_wins() {
        let two = r#"[
  {"address_index": 1, "encoded_address": "uregtest1later"},
  {"address_index": 0, "encoded_address": "uregtest1first"}
]"#;
        assert_eq!(
            first_unified_address("address", two).unwrap(),
            "uregtest1first"
        );
    }
}
