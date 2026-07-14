use crate::build_clap_app;
use crate::examples;

/// Helper: parse the given args through the clap definition and return matches.
fn parse(args: &[&str]) -> clap::ArgMatches {
    build_clap_app()
        .try_get_matches_from(args)
        .expect("valid args")
}

mod mode_of_operation {
    use super::*;
    use crate::{ModeOfOperation, get_mode_of_operation};

    fn assert_interactive(args: &[&str]) {
        let matches = parse(args);
        assert_eq!(
            get_mode_of_operation(&matches),
            ModeOfOperation::Interactive
        );
    }

    fn assert_command(args: &[&str], expected_name: &str, expected_args: &[&str]) {
        let matches = parse(args);
        assert_eq!(
            get_mode_of_operation(&matches),
            ModeOfOperation::Command {
                name: expected_name.to_string(),
                args: expected_args.iter().map(|s| s.to_string()).collect(),
            }
        );
    }

    #[test]
    fn no_command_yields_interactive() {
        assert_interactive(&[examples::BIN_NAME]);
    }

    #[test]
    fn command_without_extra_args() {
        assert_command(&[examples::BIN_NAME, "balance"], "balance", &[]);
    }

    #[test]
    fn command_with_extra_args() {
        assert_command(
            &[
                examples::BIN_NAME,
                "send",
                examples::SAPLING_ADDRESS,
                examples::AMOUNT_ZATOSHIS,
            ],
            "send",
            &[examples::SAPLING_ADDRESS, examples::AMOUNT_ZATOSHIS],
        );
    }

    #[test]
    fn help_command_is_still_command_variant() {
        // `help` is handled by `parse_args_or_exit_for_help` in main.rs before
        // `get_mode_of_operation` is called, but if it were passed through
        // it would produce a normal Command variant.
        assert_command(&[examples::BIN_NAME, "help"], "help", &[]);
    }

    #[test]
    fn flags_do_not_affect_mode_interactive() {
        assert_interactive(&[examples::BIN_NAME, "--nosync"]);
    }

    #[test]
    fn flags_do_not_affect_mode_command() {
        assert_command(&[examples::BIN_NAME, "--nosync", "balance"], "balance", &[]);
    }

    mod commands {
        use super::*;

        /// Assert that a command with no extra args parses correctly.
        fn assert_no_arg_command(name: &str) {
            assert_command(&[examples::BIN_NAME, name], name, &[]);
        }

        #[test]
        fn send() {
            assert_command(
                &[
                    examples::BIN_NAME,
                    "send",
                    examples::SAPLING_ADDRESS,
                    examples::AMOUNT_ZATOSHIS,
                    examples::MEMO,
                ],
                "send",
                &[
                    examples::SAPLING_ADDRESS,
                    examples::AMOUNT_ZATOSHIS,
                    examples::MEMO,
                ],
            );
        }

        #[test]
        fn send_all() {
            assert_command(
                &[
                    examples::BIN_NAME,
                    "send_all",
                    examples::SAPLING_ADDRESS,
                    examples::SEND_ALL_MEMO,
                ],
                "send_all",
                &[examples::SAPLING_ADDRESS, examples::SEND_ALL_MEMO],
            );
        }

        #[test]
        fn quicksend() {
            assert_command(
                &[
                    examples::BIN_NAME,
                    "quicksend",
                    examples::SAPLING_ADDRESS,
                    examples::AMOUNT_ZATOSHIS,
                    examples::MEMO,
                ],
                "quicksend",
                &[
                    examples::SAPLING_ADDRESS,
                    examples::AMOUNT_ZATOSHIS,
                    examples::MEMO,
                ],
            );
        }

        #[test]
        fn parse_address() {
            assert_command(
                &[
                    examples::BIN_NAME,
                    "parse_address",
                    examples::TRANSPARENT_ADDRESS,
                ],
                "parse_address",
                &[examples::TRANSPARENT_ADDRESS],
            );
        }

        #[test]
        fn parse_viewkey() {
            assert_command(
                &[
                    examples::BIN_NAME,
                    "parse_viewkey",
                    examples::UNIFIED_VIEWING_KEY,
                ],
                "parse_viewkey",
                &[examples::UNIFIED_VIEWING_KEY],
            );
        }

        #[test]
        fn change_server() {
            assert_command(
                &[examples::BIN_NAME, "change_server", examples::SERVER_URI],
                "change_server",
                &[examples::SERVER_URI],
            );
        }

        #[test]
        fn sync() {
            assert_command(&[examples::BIN_NAME, "sync", "run"], "sync", &["run"]);
        }

        #[test]
        fn new_address() {
            assert_command(
                &[examples::BIN_NAME, "new_address", "o"],
                "new_address",
                &["o"],
            );
        }

        #[test]
        fn balance() {
            assert_no_arg_command("balance");
        }

        #[test]
        fn confirm() {
            assert_no_arg_command("confirm");
        }

        #[test]
        fn calculate() {
            assert_no_arg_command("calculate");
        }

        #[test]
        fn transmit() {
            assert_no_arg_command("transmit");
        }

        #[test]
        fn transmit_with_txids() {
            assert_command(
                &[examples::BIN_NAME, "transmit", examples::TXID],
                "transmit",
                &[examples::TXID],
            );
        }

        #[test]
        fn shield() {
            assert_no_arg_command("shield");
        }

        #[test]
        fn height() {
            assert_no_arg_command("height");
        }

        #[test]
        fn info() {
            assert_no_arg_command("info");
        }

        #[test]
        fn addresses() {
            assert_no_arg_command("addresses");
        }

        #[test]
        fn save() {
            assert_no_arg_command("save");
        }

        #[test]
        fn quit() {
            assert_no_arg_command("quit");
        }

        #[test]
        fn notes() {
            assert_no_arg_command("notes");
        }

        #[test]
        fn version() {
            assert_no_arg_command("version");
        }

        #[test]
        fn rescan() {
            assert_no_arg_command("rescan");
        }

        #[test]
        fn export_ufvk() {
            assert_no_arg_command("export_ufvk");
        }

        #[test]
        fn settings() {
            assert_no_arg_command("settings");
        }

        #[test]
        fn value_transfers() {
            assert_no_arg_command("value_transfers");
        }

        #[test]
        fn transactions() {
            assert_no_arg_command("transactions");
        }

        #[test]
        fn quickshield() {
            assert_no_arg_command("quickshield");
        }

        #[test]
        fn wallet_kind() {
            assert_no_arg_command("wallet_kind");
        }

        #[test]
        fn birthday() {
            assert_no_arg_command("birthday");
        }

        #[test]
        fn delete() {
            assert_no_arg_command("delete");
        }
    }
}

mod communication_mode {
    use super::*;
    use crate::{CommunicationMode, get_communication_mode};

    #[test]
    fn default_is_online() {
        let matches = parse(&[examples::BIN_NAME]);
        assert_eq!(get_communication_mode(&matches), CommunicationMode::Online);
    }

    #[test]
    fn offline_flag_pins_offline() {
        let matches = parse(&[examples::BIN_NAME, "--offline"]);
        assert_eq!(get_communication_mode(&matches), CommunicationMode::Offline);
    }

    #[test]
    fn offline_conflicts_with_server() {
        assert!(
            build_clap_app()
                .try_get_matches_from([
                    examples::BIN_NAME,
                    "--offline",
                    "--server",
                    examples::SERVER_URI
                ])
                .is_err()
        );
    }

    #[test]
    fn offline_conflicts_with_waitsync() {
        assert!(
            build_clap_app()
                .try_get_matches_from([examples::BIN_NAME, "--offline", "--waitsync"])
                .is_err()
        );
    }
}

mod is_interactive {
    use super::*;
    use crate::is_interactive;

    #[test]
    fn no_command_is_interactive() {
        let matches = parse(&[examples::BIN_NAME]);
        assert!(is_interactive(&matches));
    }

    #[test]
    fn with_command_is_not_interactive() {
        let matches = parse(&[examples::BIN_NAME, "balance"]);
        assert!(!is_interactive(&matches));
    }

    #[test]
    fn flags_without_command_is_interactive() {
        let matches = parse(&[examples::BIN_NAME, "--nosync"]);
        assert!(is_interactive(&matches));
    }
}

mod log_file_path {
    use super::*;
    use crate::log_file_path;
    use std::path::PathBuf;

    #[test]
    fn default_path() {
        let matches = parse(&[examples::BIN_NAME]);
        assert_eq!(log_file_path(&matches), PathBuf::from(".zingo-cli/cli.log"));
    }

    #[test]
    fn custom_path() {
        let matches = parse(&[examples::BIN_NAME, "--log-file", "/tmp/my.log"]);
        assert_eq!(log_file_path(&matches), PathBuf::from("/tmp/my.log"));
    }
}

mod sync {
    use crate::{ScanProgress, idle_indicator, synced_indicator, syncing_indicator};

    fn progress(outputs_scanned: u64, total_outputs: u64, complete: bool) -> Option<ScanProgress> {
        Some(ScanProgress {
            outputs_scanned,
            total_outputs,
            complete,
        })
    }

    #[test]
    fn in_progress_with_available_status() {
        assert_eq!(
            syncing_indicator(progress(4_520, 10_000, false)),
            " [Syncing 4520 / 10000 outputs]"
        );
    }

    #[test]
    fn in_progress_with_unavailable_status() {
        assert_eq!(syncing_indicator(None), " [Syncing]");
    }

    #[test]
    fn in_progress_with_output_free_range() {
        assert_eq!(syncing_indicator(progress(0, 0, false)), " [Syncing]");
    }

    #[test]
    fn not_launched_not_synced() {
        assert_eq!(
            idle_indicator(progress(0, 10_000, false)),
            " [Sync stopped at 0 / 10000 outputs]"
        );
    }

    #[test]
    fn not_launched_fully_synced() {
        assert_eq!(
            idle_indicator(progress(10_000, 10_000, true)),
            " [Synced 10000 / 10000 outputs]"
        );
    }

    #[test]
    fn synced_with_unavailable_status() {
        assert_eq!(synced_indicator(None), " [Synced]");
    }

    #[test]
    fn all_outputs_scanned_but_refetch_pending_is_not_synced() {
        assert_eq!(
            idle_indicator(progress(10_000, 10_000, true)),
            " [Synced 10000 / 10000 outputs]"
        );
    }

    #[test]
    fn synced_with_unavailable_status() {
        assert_eq!(synced_indicator(None), " [Synced]");
    }

    #[test]
    fn all_outputs_scanned_but_refetch_pending_is_not_synced() {
        assert_eq!(
            idle_indicator(progress(10_000, 10_000, false)),
            " [Sync stopped at 10000 / 10000 outputs]"
        );
    }

    #[test]
    fn not_launched_with_unavailable_status() {
        assert_eq!(idle_indicator(None), " [Sync stopped]");
    }
}

mod config_template {
    use super::*;
    use crate::{
        ConfigTemplate, ModeOfOperation, build_zingo_config, get_communication_mode,
        get_mode_of_operation,
    };
    use std::path::PathBuf;
    use zingolib::config::ChainType;

    /// Helper: parse args, determine mode and communication mode, and call fill.
    fn fill(args: &[&str]) -> Result<ConfigTemplate, String> {
        let matches = parse(args);
        let mode = get_mode_of_operation(&matches);
        let communication_mode = get_communication_mode(&matches);
        ConfigTemplate::fill(mode, communication_mode, matches)
    }

    /// Helper: parse args, fill config, and build ZingoConfig in one step.
    fn fill_and_build(args: &[&str]) -> zingolib::config::ClientConfig {
        build_zingo_config(&fill(args).unwrap()).unwrap()
    }

    mod offline {
        use super::*;
        use crate::CommunicationMode;

        #[test]
        fn fill_resolves_no_server_and_disables_sync() {
            let config = fill(&[examples::BIN_NAME, "--offline"]).unwrap();
            assert_eq!(config.communication_mode, CommunicationMode::Offline);
            assert!(config.server.is_none());
            assert!(!config.sync, "an Offline-mode session cannot sync");
        }

        #[test]
        fn restored_wallet_builds_an_indexerless_client_config() {
            let data_dir = std::env::temp_dir().join("zingo-cli-offline-indexerless-test");
            let config = fill_and_build(&[
                examples::BIN_NAME,
                "--offline",
                "--seed",
                examples::SEED_PHRASE,
                "--birthday",
                "1",
                "--data-dir",
                data_dir.to_str().expect("temp dir path is valid unicode"),
            ]);
            assert!(
                config.indexer_uri().is_none(),
                "an Offline-mode session never configures an Indexer"
            );
        }

        /// A new wallet's birthday normally comes from the server's chain
        /// tip; Offline mode has no server, so the Library Birthday — a
        /// release-stamped height no new seed can predate — stands in
        /// (ADR 0007). No --birthday is demanded.
        #[test]
        fn new_wallet_without_birthday_uses_library_birthday() {
            let data_dir = std::env::temp_dir().join("zingo-cli-offline-lib-birthday-test");
            let filled = fill(&[
                examples::BIN_NAME,
                "--offline",
                "--data-dir",
                data_dir.to_str().unwrap(),
            ])
            .unwrap();
            let config = build_zingo_config(&filled).unwrap();
            assert!(matches!(
                config.wallet_config(),
                zingolib::config::WalletConfig::NewSeed { chain_height, .. }
                    if chain_height == zingolib::config::lib_birthday(ChainType::Mainnet)
            ));
        }

        /// --birthday remains available for a new Offline-mode wallet as an
        /// expert override of the Library Birthday floor (ADR 0007).
        #[test]
        fn new_wallet_birthday_overrides_library_birthday() {
            let data_dir = std::env::temp_dir().join("zingo-cli-offline-birthday-override-test");
            let filled = fill(&[
                examples::BIN_NAME,
                "--offline",
                "--birthday",
                "3500000",
                "--data-dir",
                data_dir.to_str().unwrap(),
            ])
            .unwrap();
            let config = build_zingo_config(&filled).unwrap();
            assert!(matches!(
                config.wallet_config(),
                zingolib::config::WalletConfig::NewSeed { chain_height, .. }
                    if chain_height == 3_500_000
            ));
        }
    }

    mod happy_paths {
        use super::*;
        use crate::CommunicationMode;

        #[test]
        fn defaults() {
            let config = fill(&[examples::BIN_NAME, "--server", examples::SERVER_URI]).unwrap();
            assert_eq!(config.data_dir, PathBuf::from("wallets"));
            assert_eq!(config.chaintype, ChainType::Mainnet);
            assert_eq!(config.communication_mode, CommunicationMode::Online);
            assert!(config.sync);
            assert!(!config.waitsync);
            assert!(config.seed.is_none());
            assert!(config.ufvk.is_none());
            assert_eq!(config.birthday, 0);
            assert!(matches!(config.mode, ModeOfOperation::Interactive));
        }

        #[test]
        fn nosync_flag() {
            let config = fill(&[examples::BIN_NAME, "--nosync"]).unwrap();
            assert!(!config.sync);
        }

        #[test]
        fn waitsync_flag() {
            let config = fill(&[examples::BIN_NAME, "--waitsync"]).unwrap();
            assert!(config.waitsync);
        }

        #[test]
        fn custom_data_dir() {
            let config = fill(&[examples::BIN_NAME, "--data-dir", examples::DATA_DIR]).unwrap();
            assert_eq!(config.data_dir, PathBuf::from(examples::DATA_DIR));
        }

        #[test]
        fn testnet_chain() {
            let config = fill(&[examples::BIN_NAME, "--chain", "testnet"]).unwrap();
            assert_eq!(config.chaintype, ChainType::Testnet);
        }

        #[test]
        fn seed_with_birthday() {
            let config = fill(&[
                examples::BIN_NAME,
                "--seed",
                examples::SEED_PHRASE,
                "--birthday",
                examples::BIRTHDAY,
            ])
            .unwrap();
            assert!(config.seed.is_some());
            assert_eq!(config.birthday, examples::BIRTHDAY.parse::<u64>().unwrap());
        }

        #[test]
        fn command_mode_preserved() {
            let config = fill(&[examples::BIN_NAME, "balance"]).unwrap();
            assert_eq!(
                config.mode,
                ModeOfOperation::Command {
                    name: "balance".to_string(),
                    args: vec![],
                }
            );
        }
    }

    mod error_cases {
        use super::*;

        #[test]
        fn seed_and_viewkey_both_provided() {
            let err = fill(&[
                examples::BIN_NAME,
                "--seed",
                examples::SEED_PHRASE,
                "--viewkey",
                examples::UNIFIED_VIEWING_KEY,
                "--birthday",
                examples::BIRTHDAY,
            ])
            .unwrap_err();
            assert!(err.contains("Cannot load a wallet from both seed phrase and viewkey"));
        }

        #[test]
        fn seed_without_birthday() {
            let err = fill(&[examples::BIN_NAME, "--seed", examples::SEED_PHRASE]).unwrap_err();
            assert!(err.contains("block height"));
        }

        #[test]
        fn viewkey_without_birthday() {
            let err = fill(&[
                examples::BIN_NAME,
                "--viewkey",
                examples::UNIFIED_VIEWING_KEY,
            ])
            .unwrap_err();
            assert!(err.contains("block height"));
        }

        #[test]
        fn invalid_chain_type() {
            let err = fill(&[examples::BIN_NAME, "--chain", "bogus"]).unwrap_err();
            assert!(err.contains("bogus"));
        }

        #[test]
        fn server_missing_port() {
            let err = fill(&[examples::BIN_NAME, "--server", "https://example.com"]).unwrap_err();
            assert!(err.contains("scheme"));
        }
    }

    mod zingo_config {
        use super::*;
        use pepper_sync::config::PerformanceLevel;
        use std::num::NonZeroU32;
        use zingolib::{
            config::WalletConfig,
            wallet::{SyncConfig, TransparentAddressDiscovery},
        };

        const HOSPITAL_MUSEUM_SEED: &str = "hospital museum valve antique skate museum \
     unfold vocal weird milk scale social vessel identify \
     crowd hospital control album rib bulb path oven civil tank";

        #[test]
        fn default_server_is_propagated() {
            let zc = fill_and_build(&[
                examples::BIN_NAME,
                "--seed",
                HOSPITAL_MUSEUM_SEED,
                "--birthday",
                "1",
            ]);
            let uri = zc.indexer_uri().expect("indexer_uri set").to_string();
            assert!(
                uri.starts_with(zingolib::config::DEFAULT_INDEXER_URI),
                "expected URI to start with default server, got: {uri}"
            );
        }

        #[test]
        fn custom_server_is_propagated() {
            let zc = fill_and_build(&[
                examples::BIN_NAME,
                "--server",
                examples::SERVER_URI,
                "--seed",
                HOSPITAL_MUSEUM_SEED,
                "--birthday",
                "1",
            ]);
            let uri = zc.indexer_uri().expect("indexer_uri set").to_string();
            assert!(
                uri.starts_with(examples::SERVER_URI),
                "expected URI to start with {}, got: {uri}",
                examples::SERVER_URI
            );
        }

        #[test]
        fn chain_type_is_propagated() {
            let zc = fill_and_build(&[
                examples::BIN_NAME,
                "--chain",
                "testnet",
                "--seed",
                HOSPITAL_MUSEUM_SEED,
                "--birthday",
                "1",
            ]);
            assert_eq!(zc.chain_type(), ChainType::Testnet);
        }

        #[test]
        fn data_dir_is_propagated() {
            let zc = fill_and_build(&[
                examples::BIN_NAME,
                "--data-dir",
                examples::DATA_DIR,
                "--seed",
                HOSPITAL_MUSEUM_SEED,
                "--birthday",
                "1",
            ]);
            assert_eq!(zc.wallet_dir(), PathBuf::from(examples::DATA_DIR));
        }

        #[test]
        fn default_wallet_config() {
            let zc = fill_and_build(&[
                examples::BIN_NAME,
                "--seed",
                HOSPITAL_MUSEUM_SEED,
                "--birthday",
                "1",
            ]);
            let ws = zc.wallet_config();
            assert_eq!(
                ws,
                WalletConfig::MnemonicPhrase {
                    mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
                    no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
                    birthday: 1,
                    wallet_settings: zingolib::wallet::WalletSettings {
                        sync_config: SyncConfig {
                            transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                            performance_level: PerformanceLevel::High,
                        },
                        min_confirmations: NonZeroU32::try_from(3).unwrap(),
                    },
                }
            );
        }
    }
}
