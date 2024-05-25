//! An interface that passes strings (e.g. from a cli, into zingolib)
//! upgrade-or-replace

use crate::wallet::MemoDownloadOption;
use crate::{lightclient::LightClient, wallet};
use indoc::indoc;
use json::object;
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::convert::TryInto;
use std::str::FromStr;
use tokio::runtime::Runtime;
use zcash_address::unified::{Container, Encoding, Ufvk};
use zcash_client_backend::address::Address;
use zcash_primitives::consensus::Parameters;
use zcash_primitives::transaction::fees::zip317::MINIMUM_FEE;

/// Errors associated with the commands interface
mod error;
/// Utilities associated with the commands interface
mod utils;

lazy_static! {
    static ref RT: Runtime = tokio::runtime::Runtime::new().unwrap();
}

/// TODO: Add Doc Comment Here!
pub trait Command {
    /// TODO: Add Doc Comment Here!
    fn help(&self) -> &'static str;

    /// TODO: Add Doc Comment Here!
    fn short_help(&self) -> &'static str;

    /// TODO: Add Doc Comment Here!
    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String;
}

/// TODO: Add Doc Comment Here!
pub trait ShortCircuitedCommand {
    /// TODO: Add Doc Comment Here!
    fn exec_without_lc(args: Vec<String>) -> String;
}

struct GetVersionCommand {}
impl Command for GetVersionCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Return the git describe --dirty of the repo at build time.
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Get version of build code"
    }

    fn exec(&self, _args: &[&str], _lightclient: &LightClient) -> String {
        crate::git_description().to_string()
    }
}

struct ChangeServerCommand {}
impl Command for ChangeServerCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Change the lightwalletd server to receive blockchain data from
            Usage:
            changeserver [server_uri]

            Example:
            changeserver https://mainnet.lightwalletd.com:9067
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Change lightwalletd server"
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        match args.len() {
            1 => match http::Uri::from_str(args[0]) {
                Ok(uri) => {
                    lightclient.set_server(uri);
                    "server set"
                }
                Err(_) => "invalid server uri",
            }
            .to_string(),
            _ => self.help().to_string(),
        }
    }
}

struct GetBirthdayCommand {}
impl Command for GetBirthdayCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Introspect over wallet value transfers, and report the lowest observed block height.
            Usage:
            get_birthday

            Example:
            get_birthday
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Get wallet birthday."
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move { lightclient.wallet.get_birthday().await.to_string() })
    }
}

struct WalletKindCommand {}
impl Command for WalletKindCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Displays the kind of wallet currently loaded
            If a Ufvk, displays what pools are supported.
            Currently, spend-capable wallets will always have spend capability for all three pools
            "#}
    }

    fn short_help(&self) -> &'static str {
        "Displays the kind of wallet currently loaded"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move {
            if lightclient.do_seed_phrase().await.is_ok() {
                object! {"kind" => "Seeded"}.pretty(4)
            } else {
                let capability = lightclient.wallet.wallet_capability();
                object! {
                    "kind" => "Loaded from key",
                    "transparent" => capability.transparent.kind_str(),
                    "sapling" => capability.sapling.kind_str(),
                    "orchard" => capability.orchard.kind_str(),
                }
                .pretty(4)
            }
        })
    }
}

struct InterruptCommand {}
impl Command for InterruptCommand {
    fn help(&self) -> &'static str {
        "Toggle the sync interrupt after batch flag."
    }
    fn short_help(&self) -> &'static str {
        "Toggle the sync interrupt after batch flag."
    }
    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        match args.len() {
            1 => RT.block_on(async move {
                match args[0] {
                    "true" => {
                        lightclient.interrupt_sync_after_batch(true).await;
                        "true".to_string()
                    }
                    "false" => {
                        lightclient.interrupt_sync_after_batch(false).await;
                        "false".to_string()
                    }
                    _ => self.help().to_string(),
                }
            }),
            _ => self.help().to_string(),
        }
    }
}

struct ParseAddressCommand {}
impl Command for ParseAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Parse an address
            Usage:
            parse_address [address]

            Example
            parse_address tmSwk8bjXdCgBvpS8Kybk5nUyE21QFcDqre
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Parse an address"
    }

    fn exec(&self, args: &[&str], _lightclient: &LightClient) -> String {
        match args.len() {
            1 => json::stringify_pretty(
                [
                    zingoconfig::ChainType::Mainnet,
                    zingoconfig::ChainType::Testnet,
                    zingoconfig::ChainType::Regtest(
                        zingoconfig::RegtestNetwork::all_upgrades_active(),
                    ),
                ]
                .iter()
                .find_map(|chain| Address::decode(chain, args[0]).zip(Some(chain)))
                .map(|(recipient_address, chain_name)| {
                    let chain_name_string = match chain_name {
                        zingoconfig::ChainType::Mainnet => "main",
                        zingoconfig::ChainType::Testnet => "test",
                        zingoconfig::ChainType::Regtest(_) => "regtest",
                    };

                    match recipient_address {
                        Address::Sapling(_) => object! {
                            "status" => "success",
                            "chain_name" => chain_name_string,
                            "address_kind" => "sapling",
                        },
                        Address::Transparent(_) => object! {
                            "status" => "success",
                            "chain_name" => chain_name_string,
                            "address_kind" => "transparent",
                        },
                        Address::Unified(ua) => {
                            let mut receivers_available = vec![];
                            if ua.orchard().is_some() {
                                receivers_available.push("orchard")
                            }
                            if ua.sapling().is_some() {
                                receivers_available.push("sapling")
                            }
                            if ua.transparent().is_some() {
                                receivers_available.push("transparent")
                            }
                            object! {
                                "status" => "success",
                                "chain_name" => chain_name_string,
                                "address_kind" => "unified",
                                "receivers_available" => receivers_available,
                            }
                        }
                    }
                }),
                4,
            ),
            _ => self.help().to_string(),
        }
    }
}

struct ParseViewKeyCommand {}
impl Command for ParseViewKeyCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Parse a View Key
            Usage:
            parse_viewkey viewing_key

            Example
            parse_viewkey uviewregtest1l6s73mncrefycjhksvcp3zd6x2rpwddewv852ms8w0j828wu77h8v07fs6ph68kyp0ujwk4qmr3w4v9js4mr3ufqyasr0sddgumzyjamcgreda44kxtv4ar084szez337ld58avd9at4r5lptltgkn6uayzd055upf8cnlkarnxp69kz0vzelfww08xxhm0q0azdsplxff0mn2yyve88jyl8ujfau66pnc37skvl9528zazztf6xgk8aeewswjg4eeahpml77cxh57spgywdsc99h99twmp8sqhmp7g78l3g90equ2l4vh9vy0va6r8p568qr7nm5l5y96qgwmw9j2j788lalpeywy0af86krh4td69xqrrye6dvfx0uff84s3pm50kqx3tg3ktx88j2ujswe25s7pqvv3w4x382x07w0dp5gguqu757wlyf80f5nu9uw7wqttxmvrjhkl22x43de960c7kt97ge0dkt52j7uckht54eq768
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Parse a view_key."
    }

    fn exec(&self, args: &[&str], _lightclient: &LightClient) -> String {
        match args.len() {
            1 => {
                json::stringify_pretty(
                    match Ufvk::decode(args[0]) {
                        Ok((network, ufvk)) => {
                            let mut pools_available = vec![];
                            for fvk in ufvk.items_as_parsed() {
                                match fvk {
                            zcash_address::unified::Fvk::Orchard(_) => {
                                pools_available.push("orchard")
                            }
                            zcash_address::unified::Fvk::Sapling(_) => {
                                pools_available.push("sapling")
                            }
                            zcash_address::unified::Fvk::P2pkh(_) => {
                                pools_available.push("transparent")
                            }
                            zcash_address::unified::Fvk::Unknown { .. } => pools_available
                                .push("Unknown future protocol. Perhaps you're using old software"),
                        }
                            }
                            object! {
                                "status" => "success",
                                "chain_name" => match network {
                                    zcash_address::Network::Main => "main",
                                    zcash_address::Network::Test => "test",
                                    zcash_address::Network::Regtest => "regtest",
                                },
                                "address_kind" => "ufvk",
                                "pools_available" => pools_available,
                            }
                        }
                        Err(_) => {
                            object! {
                                "status" => "Invalid viewkey",
                                "chain_name" => json::JsonValue::Null,
                                "address_kind" => json::JsonValue::Null
                            }
                        }
                    },
                    4,
                )
            }
            _ => self.help().to_string(),
        }
    }
}

struct SyncCommand {}
impl Command for SyncCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Sync the light client with the server
            Usage:
            sync

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Download CompactBlocks and sync to the server"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move {
            match lightclient.do_sync(true).await {
                Ok(j) => j.to_json().pretty(2),
                Err(e) => e,
            }
        })
    }
}

struct SyncStatusCommand {}
impl Command for SyncStatusCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Get the sync status of the wallet
            Usage:
            syncstatus

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Get the sync status of the wallet"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move {
            let status = lightclient.do_sync_status().await;

            let o = if status.in_progress {
                object! {
                    "sync_id" => status.sync_id,
                    "in_progress" => status.in_progress,
                    "last_error" => status.last_error,
                    "start_block" => status.start_block,
                    "end_block" => status.end_block,
                    "synced_blocks" => status.blocks_done,
                    "trial_decryptions_blocks" => status.trial_dec_done,
                    "txn_scan_blocks" => status.txn_scan_done,
                    "witnesses_updated" => *status.witnesses_updated.values().min().unwrap_or(&0),
                    "total_blocks" => status.blocks_total,
                    "batch_num" => status.batch_num,
                    "batch_total" => status.batch_total,
                    "sync_interrupt" => lightclient.get_sync_interrupt().await
                }
            } else {
                object! {
                    "sync_id" => status.sync_id,
                    "in_progress" => status.in_progress,
                    "last_error" => status.last_error,

                }
            };
            o.pretty(2)
        })
    }
}

struct SendProgressCommand {}
impl Command for SendProgressCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Get the progress of any send transactions that are currently computing
            Usage:
            sendprogress
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Get the progress of any send transactions that are currently computing"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move {
            match lightclient.do_send_progress().await {
                Ok(p) => p.to_json().pretty(2),
                Err(e) => e,
            }
        })
    }
}

struct RescanCommand {}
impl Command for RescanCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Rescan the wallet, rescanning all blocks for new transactions
            Usage:
            rescan

            This command will download all blocks since the initial block again from the light client server
            and attempt to scan each block for transactions belonging to the wallet.
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Rescan the wallet, downloading and scanning all blocks and transactions"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move {
            match lightclient.do_rescan().await {
                Ok(j) => j.to_json().pretty(2),
                Err(e) => e,
            }
        })
    }
}

struct ClearCommand {}
impl Command for ClearCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Clear the wallet state, rolling back the wallet to an empty state.
            Usage:
            clear

            This command will clear all notes, utxos and transactions from the wallet, setting up the wallet to be synced from scratch.
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Clear the wallet state, rolling back the wallet to an empty state."
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move {
            lightclient.clear_state().await;

            let result = object! { "result" => "success" };
            result.pretty(2)
        })
    }
}

/// TODO: Add Doc Comment Here!
pub struct HelpCommand {}
impl Command for HelpCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            List all available commands
            Usage:
            help [command_name]

            If no "command_name" is specified, a list of all available commands is returned
            Example:
            help send

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Lists all available commands"
    }

    fn exec(&self, args: &[&str], _: &LightClient) -> String {
        let mut responses = vec![];

        // Print a list of all commands
        match args.len() {
            0 => {
                responses.push("Available commands:".to_string());
                get_commands().iter().for_each(|(cmd, obj)| {
                    responses.push(format!("{} - {}", cmd, obj.short_help()));
                });

                responses.sort();
                responses.join("\n")
            }
            1 => match get_commands().get(args[0]) {
                Some(cmd) => cmd.help().to_string(),
                None => format!("Command {} not found", args[0]),
            },
            _ => self.help().to_string(),
        }
    }
}

impl ShortCircuitedCommand for HelpCommand {
    fn exec_without_lc(args: Vec<String>) -> String {
        let mut responses = vec![];

        // Print a list of all commands
        match args.len() {
            0 => {
                responses.push("Available commands:".to_string());
                get_commands().iter().for_each(|(cmd, obj)| {
                    responses.push(format!("{} - {}", cmd, obj.short_help()));
                });

                responses.sort();
                responses.join("\n")
            }
            1 => match get_commands().get(args[0].as_str()) {
                Some(cmd) => cmd.help().to_string(),
                None => format!("Command {} not found", args[0]),
            },
            _ => panic!("Unexpected number of parameters."),
        }
    }
}

struct InfoCommand {}
impl Command for InfoCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Get info about the lightwalletd we're connected to
            Usage:
            info

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Get the lightwalletd server's info"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move { lightclient.do_info().await })
    }
}

struct UpdateCurrentPriceCommand {}
impl Command for UpdateCurrentPriceCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Get the latest ZEC price from Gemini exchange's API.
            Currently using USD.
            Usage:
            zecprice

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Get the latest ZEC price in the wallet's currency (USD)"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move { lightclient.update_current_price().await })
    }
}

struct BalanceCommand {}
impl Command for BalanceCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Show the current ZEC balance in the wallet
            Usage:
            balance

            Transparent and Shielded balances, along with the addresses they belong to are displayed
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Show the current ZEC balance in the wallet"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move {
            serde_json::to_string_pretty(&lightclient.do_balance().await).unwrap()
        })
    }
}

struct AddressCommand {}
impl Command for AddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            List current addresses in the wallet
            Usage:
            address

        "#}
    }

    fn short_help(&self) -> &'static str {
        "List all addresses in the wallet"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move { lightclient.do_addresses().await.pretty(2) })
    }
}

struct ExportUfvkCommand {}
impl Command for ExportUfvkCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Export Unified full viewing key for the wallet.
            Note: If you want to backup spend capability, use the 'seed' command instead.
            Usage:
            exportufvk

            Example:
            exportufvk
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Export full viewing key for wallet addresses"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        let ufvk_res = lightclient.wallet.transaction_context.key.ufvk();
        match ufvk_res {
            Ok(ufvk) => {
                use zcash_address::unified::Encoding as _;
                object! {
                    "ufvk" => ufvk.encode(&lightclient.config().chain.network_type()),
                    "birthday" => RT.block_on(lightclient.wallet.get_birthday())
                }
                .pretty(2)
            }
            Err(e) => format!("Error: {e}"),
        }
    }
}

struct EncryptMessageCommand {}
impl Command for EncryptMessageCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Encrypt a memo to be sent to a z-address offline
            Usage:
            encryptmessage <address> "memo"
            OR
            encryptmessage "{'address': <address>, 'memo': <memo>}"

            NOTE: This command only returns the encrypted payload. It does not broadcast it. You are expected to send the encrypted payload to the recipient offline
            Example:
            encryptmessage ztestsapling1x65nq4dgp0qfywgxcwk9n0fvm4fysmapgr2q00p85ju252h6l7mmxu2jg9cqqhtvzd69jwhgv8d "Hello from the command line"

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Encrypt a memo to be sent to a z-address offline"
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if args.is_empty() || args.len() > 3 {
            return self.help().to_string();
        }

        // Check for a single argument that can be parsed as JSON
        let (to, memo) = if args.len() == 1 {
            let arg_list = args[0];
            let j = match json::parse(arg_list) {
                Ok(j) => j,
                Err(e) => {
                    let es = format!("Couldn't understand JSON: {}", e);
                    return format!("{}\n{}", es, self.help());
                }
            };

            if !j.has_key("address") || !j.has_key("memo") {
                let es = "Need 'address' and 'memo'\n".to_string();
                return format!("{}\n{}", es, self.help());
            }

            let memo =
                wallet::utils::interpret_memo_string(j["memo"].as_str().unwrap().to_string());
            if memo.is_err() {
                return format!("{}\n{}", memo.err().unwrap(), self.help());
            }
            let to = j["address"].as_str().unwrap().to_string();

            (to, memo.unwrap())
        } else if args.len() == 2 {
            let to = args[0].to_string();

            let memo = wallet::utils::interpret_memo_string(args[1].to_string());
            if memo.is_err() {
                return format!("{}\n{}", memo.err().unwrap(), self.help());
            }

            (to, memo.unwrap())
        } else {
            return format!(
                "Wrong number of arguments. Was expecting 1 or 2\n{}",
                self.help()
            );
        };

        if let Ok(m) = memo.try_into() {
            lightclient.do_encrypt_message(to, m).pretty(2)
        } else {
            "Couldn't encode memo".to_string()
        }
    }
}

struct DecryptMessageCommand {}
impl Command for DecryptMessageCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Attempt to decrypt a message with all the view keys in the wallet.
            Usage:
            decryptmessage "encrypted_message_base64"

            Example:
            decryptmessage RW5jb2RlIGFyYml0cmFyeSBvY3RldHMgYXMgYmFzZTY0LiBSZXR1cm5zIGEgU3RyaW5nLg==

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Attempt to decrypt a message with all the view keys in the wallet."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if args.len() != 1 {
            return self.help().to_string();
        }

        RT.block_on(async move {
            lightclient
                .do_decrypt_message(args[0].to_string())
                .await
                .pretty(2)
        })
    }
}

#[cfg(not(feature = "zip317"))]
struct SendCommand {}
#[cfg(not(feature = "zip317"))]
impl Command for SendCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Send ZEC to the given address(es).
            The 10_000 zat fee required to send this transaction is additionally deducted from your balance.
            Usage:
                send <address> <amount in zatoshis> "<optional memo>"
                OR
                send '[{"address":"<address>", "amount":<amount in zatoshis>, "memo":"<optional memo>"}, ...]'
            Example:
                send ztestsapling1x65nq4dgp0qfywgxcwk9n0fvm4fysmapgr2q00p85ju252h6l7mmxu2jg9cqqhtvzd69jwhgv8d 200000 "Hello from the command line"

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Send ZEC to the given address(es)."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        let send_inputs = match utils::parse_send_args(args, &lightclient.config().chain) {
            Ok(args) => args,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help send' for correct usage and examples.",
                    e
                )
            }
        };
        RT.block_on(async move {
            match lightclient.do_send(send_inputs).await {
                Ok(txid) => {
                    object! { "txid" => txid.to_string() }
                }
                Err(e) => {
                    object! { "error" => e }
                }
            }
            .pretty(2)
        })
    }
}

#[cfg(not(feature = "zip317"))]
struct ShieldCommand {}
#[cfg(not(feature = "zip317"))]
impl Command for ShieldCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Shield all your transparent and/or sapling funds
            Usage:
            shield ['transparent' or 'sapling' or 'all'] [optional address]

            NOTE: The fee required to send this transaction (currently ZEC 0.0001) is additionally deducted from your balance.
            Example:
            shield all

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Shield your transparent and/or sapling ZEC into the orchard pool"
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        let (pools_to_shield, address) =
            match utils::parse_shield_args(args, &lightclient.config().chain) {
                Ok(args) => args,
                Err(e) => {
                    return format!(
                        "Error: {}\nTry 'help shield' for correct usage and examples.",
                        e
                    )
                }
            };
        RT.block_on(async move {
            match lightclient.do_shield(&pools_to_shield, address).await {
                Ok(txid) => {
                    object! { "txid" => txid.to_string() }
                }
                Err(e) => {
                    object! { "error" => e }
                }
            }
            .pretty(2)
        })
    }
}

#[cfg(feature = "zip317")]
struct SendCommand {}
#[cfg(feature = "zip317")]
impl Command for SendCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Propose a transfer of ZEC to the given address(es).
            The fee required to send this transaction will be added to the proposal and displayed to the user.
            The 'confirm' command must be called to complete and broadcast the proposed transaction(s).

            Usage:
                send <address> <amount in zatoshis> "<optional memo>"
                OR
                send '[{"address":"<address>", "amount":<amount in zatoshis>, "memo":"<optional memo>"}, ...]'
            Example:
                send ztestsapling1x65nq4dgp0qfywgxcwk9n0fvm4fysmapgr2q00p85ju252h6l7mmxu2jg9cqqhtvzd69jwhgv8d 200000 "Hello from the command line"
                confirm

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Propose a transfer of ZEC to the given address(es) and display a proposal for confirmation."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        let receivers = match utils::parse_send_args(args, &lightclient.config().chain) {
            Ok(receivers) => receivers,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help send' for correct usage and examples.",
                    e
                )
            }
        };
        let request = match crate::data::receivers::transaction_request_from_receivers(receivers) {
            Ok(request) => request,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help send' for correct usage and examples.",
                    e
                )
            }
        };
        RT.block_on(async move {
            match lightclient
                .propose_send(request)
                .await {
                Ok(proposal) => {
                    object! { "fee" => proposal.steps().iter().fold(0, |acc, step| acc + u64::from(step.balance().fee_required())) }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        })
    }
}

/*
// Unimplemented
#[cfg(feature = "zip317")]
struct SendAllCommand {}
#[cfg(feature = "zip317")]
impl Command for SendAllCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Propose to transfer all ZEC from shielded pools to a given address.
            The fee required to send this transaction will be added to the proposal and displayed to the user.
            The 'confirm' command must be called to complete and broadcast the proposed transaction(s).

            Warning:
                Does not send transparent funds. These funds must be shielded first. Type `help shield` for more information.
            Usage:
                sendall <address> "<optional memo>"
                OR
                sendall '[{"address":"<address>", "memo":"<optional memo>"}]'
            Example:
                sendall ztestsapling1x65nq4dgp0qfywgxcwk9n0fvm4fysmapgr2q00p85ju252h6l7mmxu2jg9cqqhtvzd69jwhgv8d "Sending all funds"
                confirm

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Propose to transfer all ZEC from shielded pools to a given address and display a proposal for confirmation."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        let (address, memo) = match utils::parse_send_all_args(args, &lightclient.config().chain) {
            Ok(args) => args,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help sendall' for correct usage and examples.",
                    e
                )
            }
        };
        RT.block_on(async move {
            match lightclient
                .propose_send_all(address, memo)
                .await {
                Ok(proposal) => {
                    object! {
                        "amount" => proposal.steps().iter().fold(0, |acc, step| acc + step.shielded_inputs().unwrap().notes().iter().fold(0, |acc, note| acc + u64::from(note.note().value()))),
                        "fee" => proposal.steps().iter().fold(0, |acc, step| acc + u64::from(step.balance().fee_required())),
                    }
                }
                Err(e) => {
                    object! { "error" => e }
                }
            }
            .pretty(2)
        })
    }
}
*/
#[cfg(feature = "zip317")]
struct QuickSendCommand {}
#[cfg(feature = "zip317")]
impl Command for QuickSendCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Send ZEC to the given address(es). Combines `send` and `confirm` into a single command.
            The fee required to send this transaction is additionally deducted from your balance.
            Warning:
                Transaction(s) will be sent without the user being aware of the fee amount.
            Usage:
                quicksend <address> <amount in zatoshis> "<optional memo>"
                OR
                quicksend '[{"address":"<address>", "amount":<amount in zatoshis>, "memo":"<optional memo>"}, ...]'
            Example:
                quicksend ztestsapling1x65nq4dgp0qfywgxcwk9n0fvm4fysmapgr2q00p85ju252h6l7mmxu2jg9cqqhtvzd69jwhgv8d 200000 "Hello from the command line"

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Send ZEC to the given address(es). Combines `send` and `confirm` into a single command."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        let receivers = match utils::parse_send_args(args, &lightclient.config().chain) {
            Ok(receivers) => receivers,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help quicksend' for correct usage and examples.",
                    e
                )
            }
        };
        let request = match crate::data::receivers::transaction_request_from_receivers(receivers) {
            Ok(request) => request,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help quicksend' for correct usage and examples.",
                    e
                )
            }
        };
        RT.block_on(async move {
            match lightclient.quick_send(request).await {
                Ok(txids) => {
                    object! { "txids" => txids.iter().map(|txid| txid.to_string()).collect::<Vec<_>>() }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        })
    }
}

#[cfg(feature = "zip317")]
struct ShieldCommand {}
#[cfg(feature = "zip317")]
impl Command for ShieldCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Propose a shield of transparent funds to the orchard pool.
            The fee required to send this transaction will be added to the proposal and displayed to the user.
            The 'confirm' command must be called to complete and broadcast the proposed shield.

            Usage:
                shield
            Example:
                shield
                confirm

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Propose a shield of transparent funds to the orchard pool and display a proposal for confirmation.."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if !args.is_empty() {
            return format!(
                "Error: {}\nTry 'help shield' for correct usage and examples.",
                error::CommandError::InvalidArguments
            );
        }

        RT.block_on(async move {
            match lightclient.propose_shield().await {
                Ok(proposal) => {
                    // TODO: return amount to be shielded also?
                    let step = proposal.steps().first();
                    let value_to_shield = step
                        .transparent_inputs()
                        .iter()
                        .fold(0, |acc, transparent_output| {
                            transparent_output.value().into_u64() + acc
                        });
                    if proposal.steps().len() != 1 {
                        object! {"error" => "zip320 transactions not yet supported"}
                    } else {
                        object! {
                            "fee" => step.balance().fee_required().into_u64(),
                            "value_to_shield" => value_to_shield
                        }
                    }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        })
    }
}

#[cfg(feature = "zip317")]
struct QuickShieldCommand {}
#[cfg(feature = "zip317")]
impl Command for QuickShieldCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Shield transparent funds to the orchard pool. Combines `shield` and `confirm` into a single command.
            The fee required to send this transaction is additionally deducted from your balance.
            Warning:
                Transaction(s) will be sent without the user being aware of the fee amount.
            Usage:
                quickshield

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Shield transparent funds to the orchard pool. Combines `shield` and `confirm` into a single command."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if !args.is_empty() {
            return format!(
                "Error: {}\nTry 'help shield' for correct usage and examples.",
                error::CommandError::InvalidArguments
            );
        }

        RT.block_on(async move {
            match lightclient
                .quick_shield()
                .await {
                Ok(txids) => {
                    object! { "txids" => txids.iter().map(|txid| txid.to_string()).collect::<Vec<_>>() }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        })
    }
}

#[cfg(feature = "zip317")]
struct ConfirmCommand {}
#[cfg(feature = "zip317")]
impl Command for ConfirmCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Confirms the latest proposal, completing and broadcasting the transaction(s).
            Fails if a proposal has not already been created with the 'send', 'send_all' or 'shield' commands.
            Type 'help send', 'help sendall' or 'help shield' for more information on creating proposals.

            Usage:
                confirm
            Example:
                send ztestsapling1x65nq4dgp0qfywgxcwk9n0fvm4fysmapgr2q00p85ju252h6l7mmxu2jg9cqqhtvzd69jwhgv8d 200000 "Hello from the command line"
                confirm

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Confirms the latest proposal, completing and broadcasting the transaction(s)."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if !args.is_empty() {
            return format!(
                "Error: {}\nTry 'help confirm' for correct usage and examples.",
                error::CommandError::InvalidArguments
            );
        }

        RT.block_on(async move {
            match lightclient
                .complete_and_broadcast_stored_proposal()
                .await {
                Ok(txids) => {
                    object! { "txids" => txids.iter().map(|txid| txid.to_string()).collect::<Vec<_>>() }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        })
    }
}

// TODO: add a decline command which deletes latest proposal?

struct DeleteCommand {}
impl Command for DeleteCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Delete the wallet from disk
            Usage:
            delete

            The wallet is deleted from disk. If you want to use another wallet first you need to remove the existing wallet file

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Delete wallet file from disk"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move {
            match lightclient.do_delete().await {
                Ok(_) => {
                    let r = object! { "result" => "success",
                    "wallet_path" => lightclient.config.get_wallet_path().to_str().unwrap() };
                    r.pretty(2)
                }
                Err(e) => {
                    let r = object! {
                        "result" => "error",
                        "error" => e
                    };
                    r.pretty(2)
                }
            }
        })
    }
}

struct SeedCommand {}
impl Command for SeedCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Show the wallet's seed phrase
            Usage:
            seed

            Your wallet is entirely recoverable from the seed phrase. Please save it carefully and don't share it with anyone

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Display the seed phrase"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move {
            match lightclient.do_seed_phrase().await {
                Ok(m) => serde_json::to_string_pretty(&m).unwrap(),
                Err(e) => object! { "error" => e }.pretty(2),
            }
        })
    }
}

#[cfg(feature = "lightclient-deprecated")]
struct TransactionsCommand {}
#[cfg(feature = "lightclient-deprecated")]
impl Command for TransactionsCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            List all incoming and outgoing transactions from this wallet
            Usage:
            list [allmemos]

            If you include the 'allmemos' argument, all memos are returned in their raw hex format

        "#}
    }

    fn short_help(&self) -> &'static str {
        "List all transactions in the wallet"
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if args.len() > 1 {
            return format!("Didn't understand arguments\n{}", self.help());
        }

        RT.block_on(async move { lightclient.do_list_transactions().await.pretty(2) })
    }
}

struct ValueTxSummariesCommand {}
impl Command for ValueTxSummariesCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            List summaries of value transfers for this seed.
            Usage:
            summaries
        "#}
    }

    fn short_help(&self) -> &'static str {
        "List all value transfer summaries for this seed."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if args.len() > 1 {
            return format!("Didn't understand arguments\n{}", self.help());
        }

        RT.block_on(
            async move { json::JsonValue::from(lightclient.list_txsummaries().await).pretty(2) },
        )
    }
}

struct MemoBytesToAddressCommand {}
impl Command for MemoBytesToAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Get an object where keys are addresses and values are total bytes of memo sent to that address.
            usage:
            memobytes_to_address
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Show by address memo_bytes transfers for this seed."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if args.len() > 1 {
            return format!("didn't understand arguments\n{}", self.help());
        }

        RT.block_on(async move {
            json::JsonValue::from(lightclient.do_total_memobytes_to_address().await).pretty(2)
        })
    }
}

struct ValueToAddressCommand {}
impl Command for ValueToAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Get an object where keys are addresses and values are total value sent to that address.
            usage:
            value_to_address
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Show by address value transfers for this seed."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if args.len() > 1 {
            return format!("didn't understand arguments\n{}", self.help());
        }

        RT.block_on(async move {
            json::JsonValue::from(lightclient.do_total_value_to_address().await).pretty(2)
        })
    }
}

struct SendsToAddressCommand {}
impl Command for SendsToAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Get an object where keys are addresses and values are total value sent to that address.
            usage:
            sends_to_address
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Show by address number of sends for this seed."
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if args.len() > 1 {
            return format!("didn't understand arguments\n{}", self.help());
        }

        RT.block_on(async move {
            json::JsonValue::from(lightclient.do_total_spends_to_address().await).pretty(2)
        })
    }
}

struct SetOptionCommand {}
impl Command for SetOptionCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Set a wallet option
            Usage:
            setoption <optionname>=<optionvalue>
            List of available options:
            download_memos : none | wallet | all

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Set a wallet option"
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if args.len() != 1 {
            return format!("Error: Need exactly 1 argument\n\n{}", self.help());
        }

        let option = args[0];
        let values: Vec<&str> = option.split('=').collect();

        if values.len() != 2 {
            return "Error: Please set option value like: <optionname>=<optionvalue>".to_string();
        }

        let option_name = values[0];
        let option_value = values[1];

        RT.block_on(async move {
            match option_name {
                "download_memos" => match option_value {
                    "none" => {
                        lightclient
                            .wallet
                            .set_download_memo(MemoDownloadOption::NoMemos)
                            .await
                    }
                    "wallet" => {
                        lightclient
                            .wallet
                            .set_download_memo(MemoDownloadOption::WalletMemos)
                            .await
                    }
                    "all" => {
                        lightclient
                            .wallet
                            .set_download_memo(MemoDownloadOption::AllMemos)
                            .await
                    }
                    _ => {
                        return format!(
                            "Error: Couldn't understand {} value {}",
                            option_name, option_value
                        )
                    }
                },
                "transaction_filter_threshold" => match option_value.parse() {
                    Ok(number) => {
                        lightclient
                            .wallet
                            .wallet_options
                            .write()
                            .await
                            .transaction_size_filter = Some(number)
                    }
                    Err(e) => return format!("Error {e}, couldn't parse {option_value} as number"),
                },
                _ => return format!("Error: Couldn't understand {}", option_name),
            }

            let r = object! {
                "success" => true
            };

            r.pretty(2)
        })
    }
}

struct GetOptionCommand {}
impl Command for GetOptionCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Get a wallet option
            Argument is either "download_memos" and "transaction_filter_threshold"

            Usage:
            getoption <optionname>

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Get a wallet option"
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if args.len() != 1 {
            return format!("Error: Need exactly 1 argument\n\n{}", self.help());
        }

        let option_name = args[0];

        RT.block_on(async move {
            let value = match option_name {
                "download_memos" => match lightclient
                    .wallet
                    .wallet_options
                    .read()
                    .await
                    .download_memos
                {
                    MemoDownloadOption::NoMemos => "none".to_string(),
                    MemoDownloadOption::WalletMemos => "wallet".to_string(),
                    MemoDownloadOption::AllMemos => "all".to_string(),
                },
                "transaction_filter_threshold" => lightclient
                    .wallet
                    .wallet_options
                    .read()
                    .await
                    .transaction_size_filter
                    .map(|filter| filter.to_string())
                    .unwrap_or("No filter".to_string()),
                _ => return format!("Error: Couldn't understand {}", option_name),
            };

            let r = object! {
                option_name => value
            };

            r.pretty(2)
        })
    }
}

struct HeightCommand {}
impl Command for HeightCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Get the latest block height that the wallet is at.
            Usage:
            height

            Pass 'true' (default) to sync to the server to get the latest block height. Pass 'false' to get the latest height in the wallet without checking with the server.

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Get the latest block height that the wallet is at"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move {
            object! { "height" => lightclient.do_wallet_last_scanned_height().await}.pretty(2)
        })
    }
}

struct DefaultFeeCommand {}
impl Command for DefaultFeeCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Returns the default fee in zats for outgoing transactions
            Usage:
            defaultfee <optional_block_height>

            Example:
            defaultfee
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Returns the default fee in zats for outgoing transactions"
    }

    fn exec(&self, args: &[&str], _lightclient: &LightClient) -> String {
        if args.len() > 1 {
            return format!("Was expecting at most 1 argument\n{}", self.help());
        }

        RT.block_on(async move {
            let j = object! { "defaultfee" => u64::from(MINIMUM_FEE)};
            j.pretty(2)
        })
    }
}

struct NewAddressCommand {}
impl Command for NewAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Create a new address in this wallet
            Usage:
            new [z | t | o]

            Example:
            To create a new z address:
            new z
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Create a new address in this wallet"
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        if args.len() != 1 {
            return format!("No address type specified\n{}", self.help());
        }

        RT.block_on(async move {
            match lightclient.do_new_address(args[0]).await {
                Ok(j) => j,
                Err(e) => object! { "error" => e },
            }
            .pretty(2)
        })
    }
}

struct NotesCommand {}
impl Command for NotesCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Show all sapling notes and utxos in this wallet
            Usage:
            notes [all]

            If you supply the "all" parameter, all previously spent sapling notes and spent utxos are also included

        "#}
    }

    fn short_help(&self) -> &'static str {
        "List all sapling notes and utxos in the wallet"
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        // Parse the args.
        if args.len() > 1 {
            return self.short_help().to_string();
        }

        // Make sure we can parse the amount
        let all_notes = if args.len() == 1 {
            match args[0] {
                "all" => true,
                a => {
                    return format!(
                        "Invalid argument \"{}\". Specify 'all' to include unspent notes",
                        a
                    )
                }
            }
        } else {
            false
        };

        RT.block_on(async move { lightclient.do_list_notes(all_notes).await.pretty(2) })
    }
}

struct QuitCommand {}
impl Command for QuitCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Quit the light client
            Usage:
            quit

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Quit the lightwallet, saving state to disk"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        // before shutting down, shut down all child processes..
        // ...but only if the network being used is regtest.
        let o = RT.block_on(async move { lightclient.do_info().await });
        if o.contains("\"chain_name\": \"regtest\",") {
            use std::process::Command;

            // find zingo-cli's PID
            let cli_pid: u32 = std::process::id();

            // now find all child processes of this PID
            let raw_child_processes = Command::new("ps")
                .args(["--no-headers", "--ppid", &cli_pid.to_string()])
                .output()
                .expect("error running ps");

            let owned_child_processes: String = String::from_utf8(raw_child_processes.stdout)
                .expect("error unwrapping stdout of ps");
            let child_processes = owned_child_processes.split('\n').collect::<Vec<&str>>();

            // &str representation of PIDs
            let mut spawned_pids: Vec<&str> = Vec::new();

            for child in child_processes {
                if !child.is_empty() {
                    let ch: Vec<&str> = child.split_whitespace().collect();
                    spawned_pids.push(ch[0]);
                }
            }

            for pid in spawned_pids {
                Command::new("kill")
                    .arg(pid)
                    .output()
                    .expect("error while killing regtest-spawned processes!");
            }
        }
        "quit".to_string()
    }
}

struct DeprecatedNoCommand {}
impl Command for DeprecatedNoCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            This command has been deprecated.
            Usage:
            dont

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Deprecated command."
    }

    fn exec(&self, _args: &[&str], _lightclient: &LightClient) -> String {
        ".deprecated.".to_string()
    }
}

/// TODO: Add Doc Comment Here!
pub fn get_commands() -> HashMap<&'static str, Box<dyn Command>> {
    #[allow(unused_mut)]
    let mut entries: Vec<(&'static str, Box<dyn Command>)> = vec![
        (("version"), Box::new(GetVersionCommand {})),
        ("sync", Box::new(SyncCommand {})),
        ("syncstatus", Box::new(SyncStatusCommand {})),
        ("encryptmessage", Box::new(EncryptMessageCommand {})),
        ("decryptmessage", Box::new(DecryptMessageCommand {})),
        ("parse_address", Box::new(ParseAddressCommand {})),
        ("parse_viewkey", Box::new(ParseViewKeyCommand {})),
        ("interrupt_sync_after_batch", Box::new(InterruptCommand {})),
        ("changeserver", Box::new(ChangeServerCommand {})),
        ("rescan", Box::new(RescanCommand {})),
        ("clear", Box::new(ClearCommand {})),
        ("help", Box::new(HelpCommand {})),
        ("balance", Box::new(BalanceCommand {})),
        ("addresses", Box::new(AddressCommand {})),
        ("height", Box::new(HeightCommand {})),
        ("sendprogress", Box::new(SendProgressCommand {})),
        ("setoption", Box::new(SetOptionCommand {})),
        ("summaries", Box::new(ValueTxSummariesCommand {})),
        ("value_to_address", Box::new(ValueToAddressCommand {})),
        ("sends_to_address", Box::new(SendsToAddressCommand {})),
        (
            "memobytes_to_address",
            Box::new(MemoBytesToAddressCommand {}),
        ),
        ("getoption", Box::new(GetOptionCommand {})),
        ("exportufvk", Box::new(ExportUfvkCommand {})),
        ("info", Box::new(InfoCommand {})),
        ("updatecurrentprice", Box::new(UpdateCurrentPriceCommand {})),
        ("send", Box::new(SendCommand {})),
        ("shield", Box::new(ShieldCommand {})),
        ("save", Box::new(DeprecatedNoCommand {})),
        ("quit", Box::new(QuitCommand {})),
        ("notes", Box::new(NotesCommand {})),
        ("new", Box::new(NewAddressCommand {})),
        ("defaultfee", Box::new(DefaultFeeCommand {})),
        ("seed", Box::new(SeedCommand {})),
        ("get_birthday", Box::new(GetBirthdayCommand {})),
        ("wallet_kind", Box::new(WalletKindCommand {})),
        ("delete", Box::new(DeleteCommand {})),
    ];
    #[cfg(feature = "lightclient-deprecated")]
    {
        entries.push(("list", Box::new(TransactionsCommand {})));
    }
    #[cfg(feature = "zip317")]
    {
        //entries.push(("sendall", Box::new(SendAllCommand {})));
        entries.push(("quicksend", Box::new(QuickSendCommand {})));
        entries.push(("quickshield", Box::new(QuickShieldCommand {})));
        entries.push(("confirm", Box::new(ConfirmCommand {})));
    }
    entries.into_iter().collect()
}

/// TODO: Add Doc Comment Here!
pub fn do_user_command(cmd: &str, args: &[&str], lightclient: &LightClient) -> String {
    match get_commands().get(cmd.to_ascii_lowercase().as_str()) {
        Some(cmd) => cmd.exec(args, lightclient),
        None => format!(
            "Unknown command : {}. Type 'help' for a list of commands",
            cmd
        ),
    }
}
