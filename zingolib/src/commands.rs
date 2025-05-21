//! An interface that passes strings (e.g. from a cli, into zingolib)
//! upgrade-or-replace

use std::collections::HashMap;
use std::convert::TryInto;
use std::str::FromStr;

use indoc::indoc;
use json::object;
use lazy_static::lazy_static;
use tokio::runtime::Runtime;

use zcash_address::unified::{Container, Encoding, Ufvk};
use zcash_keys::address::Address;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_protocol::consensus::NetworkType;
use zcash_protocol::value::Zatoshis;

use crate::data::{PollReport, proposal};
use crate::lightclient::LightClient;
use crate::utils::conversion::txid_from_hex_encoded_str;
use crate::wallet::keys::unified::UnifiedKeyStore;
use pepper_sync::wallet::{OrchardNote, SaplingNote, SyncMode};

mod error;
mod utils;

lazy_static! {
    pub static ref RT: Runtime = tokio::runtime::Runtime::new().unwrap();
}

/// This command interface is used both by cli and also consumers.
pub trait Command {
    /// display command help (in cli)
    fn help(&self) -> &'static str;

    /// TODO: Add Doc Comment for this!
    fn short_help(&self) -> &'static str;

    /// in zingocli, this string is printed to console
    /// consumers occasionally make assumptions about this
    /// e. expect it to be a json object
    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String;
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

    fn exec(&self, _args: &[&str], _lightclient: &mut LightClient) -> String {
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

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        match args.len() {
            0 => {
                lightclient.set_server(http::Uri::default());
                "server set".to_string()
            }
            1 => match http::Uri::from_str(args[0]) {
                Ok(uri) => {
                    lightclient.set_server(uri);
                    "server set"
                }
                Err(_) => match args[0] {
                    "" => {
                        lightclient.set_server(http::Uri::default());
                        "server set"
                    }
                    _ => "invalid server uri",
                },
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

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(async move { lightclient.wallet.lock().await.birthday.to_string() })
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

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(async move {
            let wallet = lightclient.wallet.lock().await;
            if wallet.mnemonic().is_some() {
                object! {"kind" => "Loaded from mnemonic (seed or phrase)",
                        "transparent" => true,
                        "sapling" => true,
                        "orchard" => true,
                }
                .pretty(4)
            } else {
                match wallet
                    .unified_key_store
                    .get(&zip32::AccountId::ZERO)
                    .expect("account 0 must always exist")
                {
                    UnifiedKeyStore::Spend(_) => object! {
                        "kind" => "Loaded from unified spending key",
                        "transparent" => true,
                        "sapling" => true,
                        "orchard" => true,
                    }
                    .pretty(4),
                    UnifiedKeyStore::View(ufvk) => object! {
                        "kind" => "Loaded from unified full viewing key",
                        "transparent" => ufvk.transparent().is_some(),
                        "sapling" => ufvk.sapling().is_some(),
                        "orchard" => ufvk.orchard().is_some(),
                    }
                    .pretty(4),
                    UnifiedKeyStore::Empty => object! {
                        "kind" => "No keys found",
                        "transparent" => false,
                        "sapling" => false,
                        "orchard" => false,
                    }
                    .pretty(4),
                }
            }
        })
    }
}

struct ParseAddressCommand {}
impl Command for ParseAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Parse an address
            Usage:
            parse_address <address>

            Example
            parse_address tmSwk8bjXdCgBvpS8Kybk5nUyE21QFcDqre
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Parse an address"
    }

    fn exec(&self, args: &[&str], _lightclient: &mut LightClient) -> String {
        if args.len() > 1 || args.is_empty() {
            return self.help().to_string();
        }
        fn make_decoded_chain_pair(
            address: &str,
        ) -> Option<(
            zcash_client_backend::address::Address,
            crate::config::ChainType,
        )> {
            [
                crate::config::ChainType::Mainnet,
                crate::config::ChainType::Testnet,
                crate::config::ChainType::Regtest(
                    crate::config::RegtestNetwork::all_upgrades_active(),
                ),
            ]
            .iter()
            .find_map(|chain| Address::decode(chain, address).zip(Some(*chain)))
        }
        if let Some((recipient_address, chain_name)) = make_decoded_chain_pair(args[0]) {
            let chain_name_string = match chain_name {
                crate::config::ChainType::Mainnet => "main",
                crate::config::ChainType::Testnet => "test",
                crate::config::ChainType::Regtest(_) => "regtest",
            };
            match recipient_address {
                Address::Sapling(_) => object! {
                    "status" => "success",
                    "chain_name" => chain_name_string,
                    "address_kind" => "sapling",
                }
                .to_string(),
                Address::Transparent(_) => object! {
                    "status" => "success",
                    "chain_name" => chain_name_string,
                    "address_kind" => "transparent",
                }
                .to_string(),
                Address::Tex(_) => object! {
                    "status" => "success",
                    "chain_name" => chain_name_string,
                    "address_kind" => "tex",
                }
                .to_string(),
                Address::Unified(ua) => {
                    let mut receivers_available = vec![];
                    if ua.sapling().is_some() {
                        receivers_available.push("sapling")
                    }
                    if ua.transparent().is_some() {
                        receivers_available.push("transparent")
                    }
                    if ua.orchard().is_some() {
                        receivers_available.push("orchard");
                        object! {
                            "status" => "success",
                            "chain_name" => chain_name_string,
                            "address_kind" => "unified",
                            "receivers_available" => receivers_available,
                            "only_orchard_ua" => zcash_keys::address::UnifiedAddress::from_receivers(ua.orchard().cloned(), None, None).expect("To construct UA").encode(&chain_name),
                        }
                        .to_string()
                    } else {
                        object! {
                            "status" => "success",
                            "chain_name" => chain_name_string,
                            "address_kind" => "unified",
                            "receivers_available" => receivers_available,
                        }
                        .to_string()
                    }
                }
            }
        } else {
            object! {
                "status" => "Invalid address",
                "chain_name" => json::JsonValue::Null,
                "address_kind" => json::JsonValue::Null,
            }
            .to_string()
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

    fn exec(&self, args: &[&str], _lightclient: &mut LightClient) -> String {
        match args.len() {
            1 => json::stringify_pretty(
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
                                NetworkType::Main => "main",
                                NetworkType::Test => "test",
                                NetworkType::Regtest => "regtest",
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
            ),
            _ => self.help().to_string(),
        }
    }
}

struct SyncCommand {}
impl Command for SyncCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Launches a task for syncing the wallet to the latest state of the block chain.

            Sub-commands:
            `run` starts or resumes sync.
            `pause` pauses scanning until sync is resumed.
            `stop` shuts down sync before its complete.
            `status` returns a report of the wallet's current sync status.
            `poll` polls the sync task handle, returning a sync result if complete. If sync failed, returns the error
            instead. Poll is not intended to be called manually for zingo-cli.

            Usage:
            sync run
            sync pause
            sync stop
            sync status
            sync poll

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Sync the wallet to the latest state of the blockchain."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if args.len() != 1 {
            return "Error: sync command expects 1 argument. Type \"help sync\" for usage."
                .to_string();
        }

        match args[0] {
            "run" => {
                if lightclient.sync_mode() == SyncMode::Paused {
                    lightclient.resume_sync().expect("sync should be paused");
                    "Resuming sync task...".to_string()
                } else {
                    RT.block_on(async move {
                        match lightclient.sync().await {
                            Ok(_) => "Launching sync task...".to_string(),
                            Err(e) => format!("Error: {e}"),
                        }
                    })
                }
            }
            "pause" => match lightclient.pause_sync() {
                Ok(_) => "Pausing sync task...".to_string(),
                Err(e) => format!("Error: {e}"),
            },
            "stop" => match lightclient.stop_sync() {
                Ok(_) => "Stopping sync task...".to_string(),
                Err(e) => format!("Error: {e}"),
            },
            "status" => RT.block_on(async move {
                match pepper_sync::sync_status(&*lightclient.wallet.lock().await).await {
                    Ok(status) => json::JsonValue::from(status).pretty(2),
                    Err(e) => format!("Error: {e}"),
                }
            }),
            "poll" => match lightclient.poll_sync() {
                PollReport::NoHandle => "Sync task has not been launched.".to_string(),
                PollReport::NotReady => "Sync task is not complete.".to_string(),
                PollReport::Ready(result) => match result {
                    Ok(sync_result) => sync_result.to_string(),
                    Err(e) => format!("Error: {e}"),
                },
            },
            _ => "Error: invalid sub-command. Type \"help sync\" for usage.".to_string(),
        }
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

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(
            async move { json::JsonValue::from(lightclient.send_progress().await).pretty(2) },
        )
    }
}

struct RescanCommand {}
impl Command for RescanCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Rescan the wallet, clearing all wallet data obtained from the blockchain and launching sync from the wallet
            birthday.

            Usage:
            rescan
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Rescan the wallet, clearing all wallet data obtained from the blockchain and launching sync from the wallet
        birthday."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if !args.is_empty() {
            return "Error: rescan command expects no arguments. Type \"rescan help\" for usage."
                .to_string();
        }

        RT.block_on(async move {
            match lightclient.rescan().await {
                Ok(_) => "Launching rescan...".to_string(),
                Err(e) => format!("Error: {e}"),
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

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(async move {
            lightclient.wallet.lock().await.clear_all();

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

    fn exec(&self, args: &[&str], _: &mut LightClient) -> String {
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

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(async move { lightclient.do_info().await })
    }
}

struct UpdatePriceCommand {}
impl Command for UpdatePriceCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Updates current zec price and historical prices for wallet transactions.
            Currently only supports USD.

            Type `help set_price_api_key` for setting up an API key for price fetching.

            Usage:
            update_price

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Updates current zec price and historic prices for wallet transactions."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(async move {
            match lightclient.wallet.lock().await.update_price().await {
                Ok(_) => "prices successfully updated".to_string(),
                Err(e) => format!("Error: {e}"),
            }
        })
    }
}

struct CurrentPriceCommand {}
impl Command for CurrentPriceCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Updates price list and returns current price of zec.
            Currently only supports USD.

            Usage:
            current_price

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Updates price list and returns current price of zec."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(async move {
            match lightclient.wallet.lock().await.current_price().await {
                Ok(price) => object! { "current_price" => price },
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        })
    }
}

struct SetPriceApiKeyCommand {}
impl Command for SetPriceApiKeyCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Sets the API key for fetching zec prices. Register with CoinCap for a free API key.
            https://pro.coincap.io/signup

            Currently only CoinCap is supported.

            Usage:
            set_price_api_key <api_key>

            Example:
            set_price_api_key 4bce48cf8766d5c55ecdd83622cbba676fdc23745c7176916fd518b40e5ee6f1

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Sets the API key for fetching zec prices. Register with CoinCap for a free API key."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if args.len() != 1 {
            return "Error: incorrect number of parameters\nTry 'help set_price_api_key' for correct usage and examples.".to_string();
        }
        RT.block_on(async move {
            lightclient
                .wallet
                .lock()
                .await
                .set_price_api_key(args[0].to_string());
        });

        "Successfully set API key".to_string()
    }
}

struct BalanceCommand {}
impl Command for BalanceCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Return the wallet ZEC balance for each pool (account 0).
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Return the wallet ZEC balance for each pool (account 0)."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(async move {
            match lightclient
                .wallet
                .lock()
                .await
                .account_balance(zip32::AccountId::ZERO)
                .await
            {
                Ok(bal) => bal.to_string(),
                Err(e) => format!("Error: {e}"),
            }
        })
    }
}

struct SpendableBalanceCommand {}
impl Command for SpendableBalanceCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Display the wallet's spendable balance.
            Calculated as the confirmed shielded balance minus the fee required to send all funds to
            the given address.
            An address must be specified as fees, and therefore spendable balance, depends on the receiver
            type.
            zennies_for_zingo must also be specified as "true"|"false".  If set to "true" 1_000_000 ZAT will
            earmarked to the zingolabs developer fund with each transaction.

            Usage:
            spendablebalance <address>
            OR
            spendablebalance { "address": "<address>", "zennies_for_zingo": <true|false> }

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Display the wallet's spendable balance."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        let (address, zennies_for_zingo) = match utils::parse_spendable_balance_args(args) {
            Ok(address_and_zennies) => address_and_zennies,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help spendablebalance' for correct usage and examples.",
                    e
                );
            }
        };
        RT.block_on(async move {
            match lightclient
                .get_spendable_shielded_balance(address, zennies_for_zingo, zip32::AccountId::ZERO)
                .await
            {
                Ok(bal) => {
                    object! {
                        "balance" => bal.into_u64(),
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

struct AddressCommand {}
impl Command for AddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            List current addresses in the wallet, shielded excludes t-addresses.
            Usage:
            addresses [shielded|orchard]

        "#}
    }

    fn short_help(&self) -> &'static str {
        "List all addresses in the wallet"
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        use crate::lightclient::describe::UAReceivers;
        match args.len() {
            0 => RT.block_on(
                async move { lightclient.do_addresses(UAReceivers::All).await.pretty(2) },
            ),
            1 => match args[0] {
                "shielded" => RT.block_on(async move {
                    lightclient
                        .do_addresses(UAReceivers::Shielded)
                        .await
                        .pretty(2)
                }),
                "orchard" => RT.block_on(async move {
                    lightclient
                        .do_addresses(UAReceivers::Orchard)
                        .await
                        .pretty(2)
                }),
                _ => self.help().to_string(),
            },
            _ => self.help().to_string(),
        }
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

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(async move {
            let wallet = lightclient.wallet.lock().await;
            let ufvk: UnifiedFullViewingKey = match wallet
                .unified_key_store
                .get(&zip32::AccountId::ZERO)
                .expect("account 0 must always exist")
                .try_into()
            {
                Ok(ufvk) => ufvk,
                Err(e) => return e.to_string(),
            };
            object! {
                "ufvk" => ufvk.encode(&wallet.network),
                "birthday" => u32::from(wallet.birthday)
            }
            .pretty(2)
        })
    }
}

struct SendCommand {}
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

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        let receivers = match utils::parse_send_args(args) {
            Ok(receivers) => receivers,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help send' for correct usage and examples.",
                    e
                );
            }
        };
        let request = match crate::data::receivers::transaction_request_from_receivers(receivers) {
            Ok(request) => request,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help send' for correct usage and examples.",
                    e
                );
            }
        };
        RT.block_on(async move {
            match lightclient
                .propose_send(request, zip32::AccountId::ZERO)
                .await
            {
                Ok(proposal) => {
                    let fee = match crate::data::proposal::total_fee(&proposal) {
                        Ok(fee) => fee,
                        Err(e) => return object! { "error" => e.to_string() }.pretty(2),
                    };
                    object! { "fee" => fee.into_u64() }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        })
    }
}

struct SendAllCommand {}
impl Command for SendAllCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Propose to transfer all ZEC from shielded pools to a given address.
            The fee required to send this transaction will be added to the proposal and displayed to the user.
            The 'confirm' command must be called to complete and broadcast the proposed transaction(s).
            If invoked with a JSON arg "zennies_for_zingo" must be specified, if set to 'true' 1_000_000 ZAT
            will be sent to the zingolabs developer address with each transaction.

            Warning:
                Does not send transparent funds. These funds must be shielded first. Type `help shield` for more information.
            Usage:
                sendall <address> "<optional memo>"
                OR
                sendall '{ "address": "<address>", "memo": "<optional memo>", "zennies_for_zingo": <true|false> }'
            Example:
                sendall ztestsapling1x65nq4dgp0qfywgxcwk9n0fvm4fysmapgr2q00p85ju252h6l7mmxu2jg9cqqhtvzd69jwhgv8d "Sending all funds"
                confirm

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Propose to transfer all ZEC from shielded pools to a given address and display a proposal for confirmation."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        let (address, zennies_for_zingo, memo) = match utils::parse_send_all_args(args) {
            Ok(parse_results) => parse_results,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help sendall' for correct usage and examples.",
                    e
                );
            }
        };
        RT.block_on(async move {
            match lightclient
                .propose_send_all(address, zennies_for_zingo, memo, zip32::AccountId::ZERO)
                .await
            {
                Ok(proposal) => {
                    let amount = match proposal::total_payment_amount(&proposal) {
                        Ok(amount) => amount,
                        Err(e) => return object! { "error" => e.to_string() }.pretty(2),
                    };
                    let fee = match proposal::total_fee(&proposal) {
                        Ok(fee) => fee,
                        Err(e) => return object! { "error" => e.to_string() }.pretty(2),
                    };
                    object! {
                        "amount" => amount.into_u64(),
                        "fee" => fee.into_u64(),
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

struct QuickSendCommand {}
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

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        let receivers = match utils::parse_send_args(args) {
            Ok(receivers) => receivers,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help quicksend' for correct usage and examples.",
                    e
                );
            }
        };
        let request = match crate::data::receivers::transaction_request_from_receivers(receivers) {
            Ok(request) => request,
            Err(e) => {
                return format!(
                    "Error: {}\nTry 'help quicksend' for correct usage and examples.",
                    e
                );
            }
        };
        RT.block_on(async move {
            match lightclient.quick_send(request, zip32::AccountId::ZERO).await {
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

struct ShieldCommand {}
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

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if !args.is_empty() {
            return format!(
                "Error: {}\nTry 'help shield' for correct usage and examples.",
                error::CommandError::InvalidArguments
            );
        }

        RT.block_on(async move {
            match lightclient.propose_shield(zip32::AccountId::ZERO).await {
                Ok(proposal) => {
                    if proposal.steps().len() != 1 {
                        return object! { "error" => "shielding transactions should not have multiple proposal steps" }.pretty(2);
                    }
                    let step = proposal.steps().first();
                    let Some(value_to_shield) = step
                        .balance()
                        .proposed_change()
                        .iter()
                        .try_fold(Zatoshis::ZERO, |acc, c| acc + c.value()) else {
                            return object! { "error" => "shield amount outside valid range of zatoshis" }
                                .pretty(2);
                    };
                    let fee = step.balance().fee_required();
                    object! {
                        "value_to_shield" => value_to_shield.into_u64(),
                        "fee" => fee.into_u64(),
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

struct QuickShieldCommand {}
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

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if !args.is_empty() {
            return format!(
                "Error: {}\nTry 'help shield' for correct usage and examples.",
                error::CommandError::InvalidArguments
            );
        }

        RT.block_on(async move {
            match lightclient
                .quick_shield(zip32::AccountId::ZERO)
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

struct ConfirmCommand {}
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

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if !args.is_empty() {
            return format!(
                "Error: {}\nTry 'help confirm' for correct usage and examples.",
                error::CommandError::InvalidArguments
            );
        }

        RT.block_on(async move {
            match lightclient
                .send_stored_proposal()
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

struct ResendCommand {}
impl Command for ResendCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Re-transmits a calculated transaction from the wallet with the given txid.
            This is a manual operation so the user has the option to alternatively use the "remove_transaction" command
            to remove the calculated transaction in the case of send failure.

            usage:
            resend <txid>

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Re-transmits a calculated transaction from the wallet with the given txid."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if args.len() != 1 {
            return "Error: resend command expects 1 argument. Type \"help resend\" for usage."
                .to_string();
        }

        let txid = match txid_from_hex_encoded_str(args[0]) {
            Ok(txid) => txid,
            Err(e) => return format!("Error: {e}"),
        };

        RT.block_on(async move {
            match lightclient.resend(txid).await {
                Ok(_) => "Successfully resent transaction.".to_string(),
                Err(e) => format!("Error: {e}"),
            }
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

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(async move {
            match lightclient.do_delete().await {
                Ok(_) => {
                    let r = object! { "result" => "success",
                    "wallet_path" => lightclient.config.get_wallet_path().to_str().expect("should be valid UTF-8") };
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

struct RecoveryInfoCommand {}
impl Command for RecoveryInfoCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Display the wallet's seed phrase, birthday and number of accounts in use.

            Your wallet is entirely recoverable from the seed phrase. Please save it carefully and don't share it with anyone.

            Usage:
            recovery_info

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Display the wallet's seed phrase, birthday and number of accounts in use."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(async move {
            match lightclient.wallet.lock().await.recovery_info() {
                Some(backup_info) => backup_info.to_string(),
                None => "error: no mnemonic found. wallet loaded from key.".to_string(),
            }
        })
    }
}

struct ValueTransfersCommand {}
impl Command for ValueTransfersCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            List all value transfers for this wallet.
            A value transfer is a group of all notes to a specific receiver in a transaction.

            Usage:
            valuetransfers [bool]
        "#}
    }

    fn short_help(&self) -> &'static str {
        "List all value transfers for this wallet."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if args.len() > 1 {
            return "Error: invalid arguments\nTry 'help valuetransfers' for correct usage and examples"
                .to_string();
        }

        let newer_first = args
            .first()
            .map(|s| s.parse())
            .unwrap_or(Ok(false))
            .unwrap_or(false);

        RT.block_on(async move {
            match lightclient.sorted_value_transfers(newer_first).await {
                Ok(value_transfers) => value_transfers.to_string(),
                Err(e) => format!("Error: {e}"),
            }
        })
    }
}

struct MessagesFilterCommand {}
impl Command for MessagesFilterCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            List memo-containing value transfers sent to/from wallet. If an address is provided,
            only messages to/from that address will be provided. If a string is provided,
            messages containing that string are displayed. Otherwise, all memos are displayed.
            Currently, for received messages, this relies on the reply-to address contained in the memo.
            A value transfer is a group of all notes to a specific receiver in a transaction.

            Usage:
            messages [address]/[string]
        "#}
    }

    fn short_help(&self) -> &'static str {
        "List memos for this wallet."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if args.len() > 1 {
            return "Error: invalid arguments\nTry 'help messages' for correct usage and examples"
                .to_string();
        }

        RT.block_on(async move {
            match lightclient.messages_containing(args.first().copied()).await {
                Ok(value_transfers) => json::JsonValue::from(value_transfers).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    }
}

struct TransactionsCommand {}
impl Command for TransactionsCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Provides a list of transaction summaries related to this wallet in order of blockheight.

            Usage:
            transactions
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Provides a list of transaction summaries related to this wallet in order of blockheight."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if !args.is_empty() {
            return "Error: invalid arguments\nTry 'help transactions' for correct usage and examples"
                .to_string();
        }
        RT.block_on(async move {
            match lightclient.transaction_summaries().await {
                Ok(transactions) => transactions.to_string(),
                Err(e) => format!("Error: {e}"),
            }
        })
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

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if args.len() > 1 {
            return format!("didn't understand arguments\n{}", self.help());
        }

        RT.block_on(async move {
            match lightclient.do_total_memobytes_to_address().await {
                Ok(total_memo_bytes) => json::JsonValue::from(total_memo_bytes).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
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

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if args.len() > 1 {
            return format!("didn't understand arguments\n{}", self.help());
        }

        RT.block_on(async move {
            match lightclient.do_total_value_to_address().await {
                Ok(total_values) => json::JsonValue::from(total_values).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
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

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if args.len() > 1 {
            return format!("didn't understand arguments\n{}", self.help());
        }

        RT.block_on(async move {
            match lightclient.do_total_spends_to_address().await {
                Ok(total_spends) => json::JsonValue::from(total_spends).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    }
}

// TODO: zingo2
// struct SetOptionCommand {}
// impl Command for SetOptionCommand {
//     fn help(&self) -> &'static str {
//         indoc! {r#"
//             Set a wallet option
//             Usage:
//             setoption <optionname>=<optionvalue>
//             List of available options:
//             download_memos : none | wallet | all

//         "#}
//     }

//     fn short_help(&self) -> &'static str {
//         "Set a wallet option"
//     }

//     fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
//         if args.len() != 1 {
//             return format!("Error: Need exactly 1 argument\n\n{}", self.help());
//         }

//         let option = args[0];
//         let values: Vec<&str> = option.split('=').collect();

//         if values.len() != 2 {
//             return "Error: Please set option value like: <optionname>=<optionvalue>".to_string();
//         }

//         let option_name = values[0];
//         let option_value = values[1];

//         RT.block_on(async move {
//             match option_name {
//                 "download_memos" => match option_value {
//                     "none" => {
//                         lightclient
//                             .wallet
//                             .set_download_memo(MemoDownloadOption::NoMemos)
//                             .await
//                     }
//                     "wallet" => {
//                         lightclient
//                             .wallet
//                             .set_download_memo(MemoDownloadOption::WalletMemos)
//                             .await
//                     }
//                     "all" => {
//                         lightclient
//                             .wallet
//                             .set_download_memo(MemoDownloadOption::AllMemos)
//                             .await
//                     }
//                     _ => {
//                         return format!(
//                             "Error: Couldn't understand {} value {}",
//                             option_name, option_value
//                         )
//                     }
//                 },
//                 "transaction_filter_threshold" => match option_value.parse() {
//                     Ok(number) => {
//                         lightclient
//                             .wallet
//                             .wallet_options
//                             .write()
//                             .await
//                             .transaction_size_filter = Some(number)
//                     }
//                     Err(e) => return format!("Error {e}, couldn't parse {option_value} as number"),
//                 },
//                 _ => return format!("Error: Couldn't understand {}", option_name),
//             }

//             let r = object! {
//                 "success" => true
//             };

//             r.pretty(2)
//         })
//     }
// }

// TODO: zingo2
// struct GetOptionCommand {}
// impl Command for GetOptionCommand {
//     fn help(&self) -> &'static str {
//         indoc! {r#"
//             Get a wallet option
//             Argument is either "download_memos" and "transaction_filter_threshold"

//             Usage:
//             getoption <optionname>

//         "#}
//     }

//     fn short_help(&self) -> &'static str {
//         "Get a wallet option"
//     }

//     fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
//         if args.len() != 1 {
//             return format!("Error: Need exactly 1 argument\n\n{}", self.help());
//         }

//         let option_name = args[0];

//         RT.block_on(async move {
//             let value = match option_name {
//                 "download_memos" => match lightclient
//                     .wallet
//                     .wallet_options
//                     .read()
//                     .await
//                     .download_memos
//                 {
//                     MemoDownloadOption::NoMemos => "none".to_string(),
//                     MemoDownloadOption::WalletMemos => "wallet".to_string(),
//                     MemoDownloadOption::AllMemos => "all".to_string(),
//                 },
//                 "transaction_filter_threshold" => lightclient
//                     .wallet
//                     .wallet_options
//                     .read()
//                     .await
//                     .transaction_size_filter
//                     .map(|filter| filter.to_string())
//                     .unwrap_or("No filter".to_string()),
//                 _ => return format!("Error: Couldn't understand {}", option_name),
//             };

//             let r = object! {
//                 option_name => value
//             };

//             r.pretty(2)
//         })
//     }
// }

struct HeightCommand {}
impl Command for HeightCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Returns the blockchain height at the time the wallet last requested the latest block height from the server.

            Usage:
            height

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Returns the blockchain height at the time the wallet last requested the latest block height from the server."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        RT.block_on(async move {
            object! { "height" => json::JsonValue::from(lightclient.wallet.lock().await.sync_state.wallet_height().map(u32::from).unwrap_or(0))}.pretty(2)
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

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
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
            Show all notes (shielded outputs) in this wallet
            Usage:
            notes [all]
            If you supply the "all" parameter, all spent notes are also included
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Show all notes (shielded outputs) in this wallet"
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
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
                        "Invalid argument \"{}\". Specify 'all' to include spent notes",
                        a
                    );
                }
            }
        } else {
            false
        };

        RT.block_on(async move {
            let wallet = lightclient.wallet.lock().await;

            json::object! {
                "orchard_notes" => json::JsonValue::from(wallet.note_summaries::<OrchardNote>(all_notes)),
                "sapling_notes" => json::JsonValue::from(wallet.note_summaries::<SaplingNote>(all_notes)),
            }
            .pretty(2)
        })
    }
}

struct CoinsCommand {}
impl Command for CoinsCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Show all coins (transparent outputs) in this wallet
            Usage:
            notes [all]
            If you supply the "all" parameter, all spent coins are also included
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Show all coins (transparent outputs) in this wallet"
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        // Parse the args.
        if args.len() > 1 {
            return self.short_help().to_string();
        }

        // Make sure we can parse the amount
        let all_coins = if args.len() == 1 {
            match args[0] {
                "all" => true,
                a => {
                    return format!(
                        "Invalid argument \"{}\". Specify 'all' to include spent coins",
                        a
                    );
                }
            }
        } else {
            false
        };

        RT.block_on(async move {
            json::object! {
                "transparent_coins" => json::JsonValue::from(lightclient.wallet.lock().await.coin_summaries(all_coins)),
            }
            .pretty(2)
        })
    }
}

struct RemoveTransactionCommand {}
impl Command for RemoveTransactionCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Removes an unconfirmed transaction from the wallet with the given txid.
            This is useful when a send fails and the pending spent outputs should be reset to unspent instead of using
            the "resend" command to attempt to re-transmit.
            This is a manual operation so important information such as memos are retained in the case of send failure
            until the user decides to remove them or resend.

            usage:
            remove_transaction <txid>

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Removes an unconfirmed transaction from the wallet with the given txid."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if args.len() != 1 {
            return "Error: remove command expects 1 argument. Type \"help remove\" for usage."
                .to_string();
        }

        let txid = match txid_from_hex_encoded_str(args[0]) {
            Ok(txid) => txid,
            Err(e) => return format!("Error: {e}"),
        };

        RT.block_on(async move {
            match lightclient
                .wallet
                .lock()
                .await
                .remove_unconfirmed_transaction(txid)
            {
                Ok(_) => "Successfully removed transaction.".to_string(),
                Err(e) => format!("Error: {e}"),
            }
        })
    }
}

struct SaveCommand {}
impl Command for SaveCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Launches a save task which saves the wallet to persistance when the wallet state changes.
            Not intended to be called manually.

            usage:
            save run
            save check
            save shutdown

        "#}
    }

    fn short_help(&self) -> &'static str {
        "Launches a save task. Not intended to be called manually."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> String {
        if args.len() != 1 {
            return "Error: save command expects 1 argument. Type \"help save\" for usage."
                .to_string();
        }

        match args[0] {
            "run" => {
                RT.block_on(async move { lightclient.save_task().await });
                "Launching save task...".to_string()
            }
            "check" => match RT.block_on(async move { lightclient.check_save_error().await }) {
                Ok(_) => "".to_string(),
                Err(e) => {
                    format!("Error: save failed. {}\nRestarting save task...", e)
                }
            },
            "shutdown" => {
                match RT.block_on(async move { lightclient.shutdown_save_task().await }) {
                    Ok(_) => "Save task shutdown successfully.".to_string(),
                    Err(e) => {
                        format!("Error: save failed. {}", e)
                    }
                }
            }
            _ => "Error: invalid sub-command. Type \"help save\" for usage.".to_string(),
        }
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

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> String {
        let save_shutdown = do_user_command("save", &["shutdown"], lightclient);

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
        format!("{}\nZingo CLI quit successfully.", save_shutdown)
    }
}

/// TODO: Add Doc Comment Here!
pub fn get_commands() -> HashMap<&'static str, Box<dyn Command>> {
    let mut entries: Vec<(&'static str, Box<dyn Command>)> = vec![
        ("version", Box::new(GetVersionCommand {})),
        ("sync", Box::new(SyncCommand {})),
        ("parse_address", Box::new(ParseAddressCommand {})),
        ("parse_viewkey", Box::new(ParseViewKeyCommand {})),
        ("changeserver", Box::new(ChangeServerCommand {})),
        ("rescan", Box::new(RescanCommand {})),
        ("clear", Box::new(ClearCommand {})),
        ("help", Box::new(HelpCommand {})),
        ("balance", Box::new(BalanceCommand {})),
        ("addresses", Box::new(AddressCommand {})),
        ("height", Box::new(HeightCommand {})),
        ("sendprogress", Box::new(SendProgressCommand {})),
        ("valuetransfers", Box::new(ValueTransfersCommand {})),
        ("transactions", Box::new(TransactionsCommand {})),
        ("value_to_address", Box::new(ValueToAddressCommand {})),
        ("sends_to_address", Box::new(SendsToAddressCommand {})),
        ("messages", Box::new(MessagesFilterCommand {})),
        (
            "memobytes_to_address",
            Box::new(MemoBytesToAddressCommand {}),
        ),
        ("exportufvk", Box::new(ExportUfvkCommand {})),
        ("info", Box::new(InfoCommand {})),
        ("update_price", Box::new(UpdatePriceCommand {})),
        ("current_price", Box::new(CurrentPriceCommand {})),
        ("set_price_api_key", Box::new(SetPriceApiKeyCommand {})),
        ("send", Box::new(SendCommand {})),
        ("resend", Box::new(ResendCommand {})),
        ("shield", Box::new(ShieldCommand {})),
        ("save", Box::new(SaveCommand {})),
        ("quit", Box::new(QuitCommand {})),
        ("notes", Box::new(NotesCommand {})),
        ("coins", Box::new(CoinsCommand {})),
        ("new", Box::new(NewAddressCommand {})),
        ("recovery_info", Box::new(RecoveryInfoCommand {})),
        ("get_birthday", Box::new(GetBirthdayCommand {})),
        ("wallet_kind", Box::new(WalletKindCommand {})),
        ("delete", Box::new(DeleteCommand {})),
        ("remove_transaction", Box::new(RemoveTransactionCommand {})),
    ];
    {
        entries.push(("spendablebalance", Box::new(SpendableBalanceCommand {})));
        entries.push(("sendall", Box::new(SendAllCommand {})));
        entries.push(("quicksend", Box::new(QuickSendCommand {})));
        entries.push(("quickshield", Box::new(QuickShieldCommand {})));
        entries.push(("confirm", Box::new(ConfirmCommand {})));
    }
    entries.into_iter().collect()
}

/// TODO: Add Doc Comment Here!
pub fn do_user_command(cmd: &str, args: &[&str], lightclient: &mut LightClient) -> String {
    match get_commands().get(cmd.to_ascii_lowercase().as_str()) {
        Some(cmd) => cmd.exec(args, lightclient),
        None => format!(
            "Unknown command : {}. Type 'help' for a list of commands",
            cmd
        ),
    }
}
