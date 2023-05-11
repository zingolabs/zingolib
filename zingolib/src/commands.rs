use crate::wallet::keys::is_shielded_address;
use crate::wallet::MemoDownloadOption;
use crate::{lightclient::LightClient, wallet::utils};
use indoc::indoc;
use json::object;
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::convert::TryInto;
use std::str::FromStr;
use tokio::runtime::Runtime;
use zcash_client_backend::address::RecipientAddress;
use zcash_primitives::transaction::components::amount::DEFAULT_FEE;

lazy_static! {
    static ref RT: Runtime = tokio::runtime::Runtime::new().unwrap();
}

pub trait Command {
    fn help(&self) -> &'static str;

    fn short_help(&self) -> &'static str;

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String;
}

pub trait ShortCircuitedCommand {
    fn exec_without_lc(args: Vec<String>) -> String;
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

struct ParseCommand {}
impl Command for ParseCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Parse an address
            Usage:
            parse [address]

            Example
            parse tmSwk8bjXdCgBvpS8Kybk5nUyE21QFcDqre
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
                    zingoconfig::ChainType::Regtest,
                ]
                .iter()
                .find_map(|chain| RecipientAddress::decode(chain, args[0]).zip(Some(chain)))
                .map(|(recipient_address, chain_name)| {
                    let chain_name_string = match chain_name {
                        zingoconfig::ChainType::Mainnet => "main",
                        zingoconfig::ChainType::Testnet => "test",
                        zingoconfig::ChainType::Regtest => "regtest",
                        zingoconfig::ChainType::FakeMainnet => unreachable!(),
                    };

                    match recipient_address {
                        RecipientAddress::Shielded(_) => object! {
                            "status" => "success",
                            "chain_name" => chain_name_string,
                            "address_kind" => "sapling",
                        },
                        RecipientAddress::Transparent(_) => object! {
                            "status" => "success",
                            "chain_name" => chain_name_string,
                            "address_kind" => "transparent",
                        },
                        RecipientAddress::Unified(ua) => {
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
                })
                .unwrap_or(object! {
                    "status" => "Invalid address",
                    "chain_name" => json::JsonValue::Null,
                    "address_kind" => json::JsonValue::Null
                }),
                4,
            ),
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
                Ok(j) => j.pretty(2),
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
                Ok(j) => j.pretty(2),
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

            This command will download all blocks since the intial block again from the light client server
            and attempt to scan each block for transactions belonging to the wallet.
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Rescan the wallet, downloading and scanning all blocks and transactions"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        RT.block_on(async move {
            match lightclient.do_rescan().await {
                Ok(j) => j.pretty(2),
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
        RT.block_on(async move { lightclient.do_balance().await.pretty(2) })
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

struct ExportCommand {}
impl Command for ExportCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Export private key for an individual wallet addresses.
            Note: To backup the whole wallet, use the 'seed' command insted
            Usage:
            export [t-address or z-address]

            If no address is passed, private key for all addresses in the wallet are exported.

            Example:
            export ztestsapling1x65nq4dgp0qfywgxcwk9n0fvm4fysmapgr2q00p85ju252h6l7mmxu2jg9cqqhtvzd69jwhgv8d
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Export private key for wallet addresses"
    }

    fn exec(&self, _args: &[&str], _lightclient: &LightClient) -> String {
        "Key export currently unimplemented, please use seed phrase to backup/restore keys"
            .to_string()
    }
}

struct ShieldCommand {}
impl Command for ShieldCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Shield all your transparent funds
            Usage:
            shield [optional address]

            NOTE: The fee required to send this transaction (currently ZEC 0.0001) is additionally deducted from your balance.
            Example:
            shield

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Shield your transparent ZEC into a sapling address"
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        // Parse the address or amount
        let address = if !args.is_empty() {
            Some(args[0].to_string())
        } else {
            None
        };
        RT.block_on(async move {
            match lightclient.do_shield(address).await {
                Ok(transaction_id) => {
                    object! { "txid" => transaction_id }
                }
                Err(e) => {
                    object! { "error" => e }
                }
            }
            .pretty(2)
        })
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

            let memo = utils::interpret_memo_string(j["memo"].as_str().unwrap().to_string());
            if memo.is_err() {
                return format!("{}\n{}", memo.err().unwrap(), self.help());
            }
            let to = j["address"].as_str().unwrap().to_string();

            (to, memo.unwrap())
        } else if args.len() == 2 {
            let to = args[0].to_string();

            let memo = utils::interpret_memo_string(args[1].to_string());
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

struct SendCommand {}
impl Command for SendCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Send ZEC to a given address(es)
            Usage:
            send <address> <amount in zatoshis> "optional_memo"
            OR
            send '[{'address': <address>, 'amount': <amount in zatoshis>, 'memo': <optional memo>}, ...]'

            NOTE: The fee required to send this transaction (currently ZEC 0.0001) is additionally deducted from your balance.
            Example:
            send ztestsapling1x65nq4dgp0qfywgxcwk9n0fvm4fysmapgr2q00p85ju252h6l7mmxu2jg9cqqhtvzd69jwhgv8d 200000 "Hello from the command line"

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Send ZEC to the given address"
    }

    fn exec(&self, args: &[&str], lightclient: &LightClient) -> String {
        // Parse the args. There are two argument types.
        // 1 - A set of 2(+1 optional) arguments for a single address send representing address, value, memo?
        // 2 - A single argument in the form of a JSON string that is "[{address: address, value: value, memo: memo},...]"
        if args.is_empty() || args.len() > 3 {
            return self.help().to_string();
        }

        RT.block_on(async move {
            // Check for a single argument that can be parsed as JSON
            let send_args = if args.len() == 1 {
                let arg_list = args[0];

                let json_args = match json::parse(arg_list) {
                    Ok(j) => j,
                    Err(e) => {
                        let es = format!("Couldn't understand JSON: {}", e);
                        return format!("{}\n{}", es, self.help());
                    }
                };

                if !json_args.is_array() {
                    return format!("Couldn't parse argument as array\n{}", self.help());
                }

                let fee = u64::from(DEFAULT_FEE);
                let maybe_send_args = json_args
                    .members()
                    .map(|j| {
                        if !j.has_key("address") || !j.has_key("amount") {
                            Err("Need 'address' and 'amount'\n".to_string())
                        } else {
                            let amount = Some(j["amount"].as_u64().unwrap());

                            match amount {
                                Some(amt) => Ok((
                                    j["address"].as_str().unwrap().to_string(),
                                    amt,
                                    j["memo"].as_str().map(|s| s.to_string()),
                                )),
                                None => Err(format!(
                                    "Not enough in wallet to pay transaction fee of {}",
                                    fee
                                )),
                            }
                        }
                    })
                    .collect::<Result<Vec<(String, u64, Option<String>)>, String>>();

                match maybe_send_args {
                    Ok(a) => a.clone(),
                    Err(s) => {
                        return format!("Error: {}\n{}", s, self.help());
                    }
                }
            } else if args.len() == 2 || args.len() == 3 {
                let address = args[0].to_string();

                // Make sure we can parse the amount
                let value = match args[1].parse::<u64>() {
                    Ok(amt) => amt,
                    Err(e) => return format!("Couldn't parse amount: {}", e),
                };

                let memo = if args.len() == 3 {
                    Some(args[2].to_string())
                } else {
                    None
                };

                // Memo has to be None if not sending to a shielded address
                if memo.is_some() && !is_shielded_address(&address, &lightclient.config) {
                    return format!("Can't send a memo to the non-shielded address {}", address);
                }

                vec![(args[0].to_string(), value, memo)]
            } else {
                return self.help().to_string();
            };

            // Convert to the right format. String -> &str.
            let tos = send_args
                .iter()
                .map(|(a, v, m)| (a.as_str(), *v, m.clone()))
                .collect::<Vec<_>>();
            match lightclient.do_send(tos).await {
                Ok(transaction_id) => {
                    object! { "txid" => transaction_id }
                }
                Err(e) => {
                    object! { "error" => e }
                }
            }
            .pretty(2)
        })
    }
}

fn wallet_saver(lightclient: &LightClient) -> String {
    RT.block_on(async move {
        match lightclient.do_save().await {
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
struct SaveCommand {}
impl Command for SaveCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Save the wallet to disk
            Usage:
            save

            The wallet is saved to disk. The wallet is periodically saved to disk (and also saved upon exit)
            but you can use this command to explicitly save it to disk

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Save wallet file to disk"
    }

    fn exec(&self, _args: &[&str], lightclient: &LightClient) -> String {
        wallet_saver(lightclient)
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
                Ok(j) => j,
                Err(e) => object! { "error" => e },
            }
            .pretty(2)
        })
    }
}

struct TransactionsCommand {}
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

        RT.block_on(async move {
            json::JsonValue::from(lightclient.do_list_txsummaries().await).pretty(2)
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

struct ImportCommand {}
impl Command for ImportCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Import an external spending or viewing key into the wallet
            Usage:
            import <spending_key | viewing_key> <birthday> [norescan]
            OR
            import '{'key': <spending_key or viewing_key>, 'birthday': <birthday>, 'norescan': <true>}'

            Birthday is the earliest block number that has transactions belonging to the imported key. Rescanning will start from this block. If not sure, you can specify '0', which will start rescanning from the first sapling block.
            Note that you can import only the full spending (private) key or the full viewing key.

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Import spending or viewing keys into the wallet"
    }

    fn exec(&self, _args: &[&str], _lightclient: &LightClient) -> String {
        "Key import not currently supported".to_string()
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
            let j = object! { "defaultfee" => u64::from(DEFAULT_FEE)};
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
            Save the wallet to disk and quit
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
                .expect("error unwraping stdout of ps");
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

        wallet_saver(lightclient)
    }
}

pub fn get_commands() -> HashMap<&'static str, Box<dyn Command>> {
    let entries: [(&'static str, Box<dyn Command>); 30] = [
        ("sync", Box::new(SyncCommand {})),
        ("syncstatus", Box::new(SyncStatusCommand {})),
        ("encryptmessage", Box::new(EncryptMessageCommand {})),
        ("decryptmessage", Box::new(DecryptMessageCommand {})),
        ("parse", Box::new(ParseCommand {})),
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
        ("getoption", Box::new(GetOptionCommand {})),
        ("import", Box::new(ImportCommand {})),
        ("export", Box::new(ExportCommand {})),
        ("info", Box::new(InfoCommand {})),
        ("updatecurrentprice", Box::new(UpdateCurrentPriceCommand {})),
        ("send", Box::new(SendCommand {})),
        ("shield", Box::new(ShieldCommand {})),
        ("save", Box::new(SaveCommand {})),
        ("quit", Box::new(QuitCommand {})),
        ("list", Box::new(TransactionsCommand {})),
        ("notes", Box::new(NotesCommand {})),
        ("new", Box::new(NewAddressCommand {})),
        ("defaultfee", Box::new(DefaultFeeCommand {})),
        ("seed", Box::new(SeedCommand {})),
    ];

    HashMap::from(entries)
}

pub fn do_user_command(cmd: &str, args: &[&str], lightclient: &LightClient) -> String {
    match get_commands().get(cmd.to_ascii_lowercase().as_str()) {
        Some(cmd) => cmd.exec(args, lightclient),
        None => format!(
            "Unknown command : {}. Type 'help' for a list of commands",
            cmd
        ),
    }
}

#[cfg(test)]
pub mod tests {
    use super::do_user_command;
    use crate::{lightclient::LightClient, wallet::WalletBase};
    use lazy_static::lazy_static;
    use tokio::runtime::Runtime;
    use zingoconfig::ZingoConfig;

    lazy_static! {
        static ref TEST_SEED: String = "youth strong sweet gorilla hammer unhappy congress stamp left stereo riot salute road tag clean toilet artefact fork certain leopard entire civil degree wonder".to_string();
    }

    pub fn test_command_caseinsensitive() {
        let lc = Runtime::new()
            .unwrap()
            .block_on(LightClient::test_new(
                &ZingoConfig::create_unconnected(zingoconfig::ChainType::FakeMainnet, None),
                WalletBase::MnemonicPhrase(TEST_SEED.to_string()),
                0,
            ))
            .unwrap();

        assert_eq!(
            do_user_command("addresses", &[], &lc),
            do_user_command("AddReSSeS", &[], &lc)
        );
        assert_eq!(
            do_user_command("addresses", &[], &lc),
            do_user_command("Addresses", &[], &lc)
        );
    }

    #[test]
    pub fn test_nosync_commands() {
        // The following commands should run
    }
}
