use crate::build_clap_app;
use crate::examples;

/// Helper: parse the given args through the clap definition and return matches.
fn parse(args: &[&str]) -> clap::ArgMatches {
    build_clap_app()
        .try_get_matches_from(args)
        .expect("valid args")
}

mod config_template_refusals {
    /// HYPOTHESIS: the template's refusals are typed variants rendering the
    /// CLI's exact prose. Falsified if a variant or its rendering drifts.
    #[test]
    fn refusals_render_their_cli_prose() {
        assert_eq!(
            crate::ConfigTemplateError::BothSeedAndViewkey.to_string(),
            "Cannot load a wallet from both seed phrase and viewkey!"
        );
        assert_eq!(
            crate::ConfigTemplateError::OnlineGrantUnused {
                command: "balance".to_string()
            }
            .to_string(),
            "`balance` needs no network, so `--online` grants a connection it never \
             uses. Drop `--online`, or run it at the interactive prompt."
        );
    }
}

mod misplaced_session_option {
    use super::*;
    use crate::misplaced_session_option;

    fn args(parts: &[&str]) -> Vec<String> {
        std::iter::once(examples::BIN_NAME)
            .chain(parts.iter().copied())
            .map(str::to_string)
            .collect()
    }

    /// A session option after the command is caught and the fix names the
    /// option and the corrected order.
    #[test]
    fn a_long_option_after_the_command_is_caught() {
        let guidance = misplaced_session_option(&args(&["sync", "run", "--online"]))
            .expect("--online after the command is misplaced");
        assert!(
            guidance.contains("`--online` is a session option"),
            "{guidance}"
        );
        assert!(
            guidance.contains("zingo-cli --online sync run"),
            "the fix moves the option ahead of the command: {guidance}"
        );
    }

    /// The correctly ordered invocation is not flagged.
    #[test]
    fn a_long_option_before_the_command_is_accepted() {
        assert!(misplaced_session_option(&args(&["--online", "sync", "run"])).is_none());
    }

    /// A `--flag=value` form after the command is caught by its name.
    #[test]
    fn an_equals_form_after_the_command_is_caught() {
        let guidance = misplaced_session_option(&args(&["balance", "--data-dir=/tmp/w"]))
            .expect("--data-dir after the command is misplaced");
        assert!(
            guidance.contains("`--data-dir` is a session option"),
            "{guidance}"
        );
    }

    /// A short session flag after the command is caught.
    #[test]
    fn a_short_flag_after_the_command_is_caught() {
        // `-n` is --nosync's short form.
        let guidance = misplaced_session_option(&args(&["balance", "-n"]))
            .expect("-n after the command is misplaced");
        assert!(guidance.contains("`-n` is a session option"), "{guidance}");
    }

    /// With no command token there is nothing after a command to misplace.
    #[test]
    fn options_without_a_command_are_not_flagged() {
        assert!(misplaced_session_option(&args(&["--online"])).is_none());
    }

    /// A command's own value that follows `--` is never read as an option.
    #[test]
    fn tokens_after_the_end_of_options_marker_are_ignored() {
        assert!(misplaced_session_option(&args(&["sync", "run", "--", "--online"])).is_none());
    }
}

mod mode_of_operation {
    use super::*;
    use crate::commands::CliCommand;
    use crate::{ModeOfOperation, get_mode_of_operation};

    fn assert_interactive(args: &[&str]) {
        let matches = parse(args);
        assert_eq!(
            get_mode_of_operation(&matches),
            ModeOfOperation::Interactive
        );
    }

    fn assert_command(args: &[&str], expected: CliCommand) {
        let matches = parse(args);
        assert_eq!(
            get_mode_of_operation(&matches),
            ModeOfOperation::Command { command: expected }
        );
    }

    #[test]
    fn no_command_yields_interactive() {
        assert_interactive(&[examples::BIN_NAME]);
    }

    /// A session flag binds before the command name and refuses after it.
    #[test]
    fn session_flags_precede_the_command_name() {
        let matches = parse(&[examples::BIN_NAME, "--nosync", "balance"]);
        assert!(matches.get_flag("nosync"));
        assert_eq!(
            get_mode_of_operation(&matches),
            ModeOfOperation::Command {
                command: CliCommand::Balance
            }
        );
        assert!(
            build_clap_app()
                .try_get_matches_from([examples::BIN_NAME, "balance", "--nosync"])
                .is_err()
        );
    }

    #[test]
    fn command_without_extra_args() {
        assert_command(&[examples::BIN_NAME, "balance"], CliCommand::Balance);
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
            CliCommand::Send {
                args: vec![
                    examples::SAPLING_ADDRESS.to_string(),
                    examples::AMOUNT_ZATOSHIS.to_string(),
                ],
            },
        );
    }

    #[test]
    fn help_command_is_still_command_variant() {
        // `help` is handled by `parse_args_or_exit_for_help` in main.rs before
        // `get_mode_of_operation` is called, but if it were passed through
        // it would produce a normal Command variant.
        assert_command(
            &[examples::BIN_NAME, "help"],
            CliCommand::Help { command: None },
        );
    }

    #[test]
    fn flags_do_not_affect_mode_interactive() {
        assert_interactive(&[examples::BIN_NAME, "--nosync"]);
    }

    #[test]
    fn flags_do_not_affect_mode_command() {
        assert_command(
            &[examples::BIN_NAME, "--nosync", "balance"],
            CliCommand::Balance,
        );
    }

    mod commands {
        use super::*;
        use crate::commands::{SaveSubCommand, SyncSubCommand};
        use zingolib::wallet::keys::unified::ReceiverSelection;

        /// Assert that a command with no arguments parses to its variant.
        fn assert_no_arg_command(name: &str, expected: CliCommand) {
            assert_command(&[examples::BIN_NAME, name], expected);
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
                CliCommand::Send {
                    args: vec![
                        examples::SAPLING_ADDRESS.to_string(),
                        examples::AMOUNT_ZATOSHIS.to_string(),
                        examples::MEMO.to_string(),
                    ],
                },
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
                CliCommand::SendAll {
                    args: vec![
                        examples::SAPLING_ADDRESS.to_string(),
                        examples::SEND_ALL_MEMO.to_string(),
                    ],
                },
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
                CliCommand::Quicksend {
                    args: vec![
                        examples::SAPLING_ADDRESS.to_string(),
                        examples::AMOUNT_ZATOSHIS.to_string(),
                        examples::MEMO.to_string(),
                    ],
                },
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
                CliCommand::ParseAddress {
                    address: examples::TRANSPARENT_ADDRESS.to_string(),
                },
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
                CliCommand::ParseViewkey {
                    viewkey: examples::UNIFIED_VIEWING_KEY.to_string(),
                },
            );
        }

        #[test]
        fn change_server() {
            assert_command(
                &[examples::BIN_NAME, "change_server", examples::SERVER_URI],
                CliCommand::ChangeServer {
                    uri: Some(examples::SERVER_URI.parse().expect("a valid example uri")),
                },
            );
        }

        #[test]
        fn sync() {
            assert_command(
                &[examples::BIN_NAME, "sync", "run"],
                CliCommand::Sync {
                    sub: SyncSubCommand::Run,
                },
            );
        }

        #[test]
        fn new_address() {
            assert_command(
                &[examples::BIN_NAME, "new_address", "o"],
                CliCommand::NewAddress {
                    receivers: ReceiverSelection {
                        orchard: true,
                        sapling: false,
                    },
                },
            );
        }

        #[test]
        fn balance() {
            assert_no_arg_command("balance", CliCommand::Balance);
        }

        #[test]
        fn confirm() {
            assert_no_arg_command("confirm", CliCommand::Confirm);
        }

        #[test]
        fn calculate() {
            assert_no_arg_command("calculate", CliCommand::Calculate);
        }

        #[test]
        fn transmit() {
            assert_no_arg_command("transmit", CliCommand::Transmit { txids: vec![] });
        }

        #[test]
        fn transmit_with_txids() {
            let txid = zingolib::utils::conversion::txid_from_hex_encoded_str(examples::TXID)
                .expect("a valid example txid");
            assert_command(
                &[examples::BIN_NAME, "transmit", examples::TXID],
                CliCommand::Transmit { txids: vec![txid] },
            );
        }

        #[test]
        fn shield() {
            assert_no_arg_command("shield", CliCommand::Shield);
        }

        #[test]
        fn height() {
            assert_no_arg_command("height", CliCommand::Height);
        }

        #[test]
        fn info() {
            assert_no_arg_command("info", CliCommand::Info);
        }

        #[test]
        fn addresses() {
            assert_no_arg_command("addresses", CliCommand::Addresses);
        }

        /// `save` names its sub-command: the grammar requires one, so the
        /// process's own argument parse refuses a bare `save`.
        #[test]
        fn save() {
            assert_command(
                &[examples::BIN_NAME, "save", "run"],
                CliCommand::Save {
                    sub: SaveSubCommand::Run,
                },
            );
        }

        #[test]
        fn quit() {
            assert_no_arg_command("quit", CliCommand::Quit);
        }

        /// `exit` is an alias of `quit`, so it parses to the same variant
        /// and the one-shot flow quits cleanly instead of erroring.
        #[test]
        fn exit_is_an_alias_of_quit() {
            assert_no_arg_command("exit", CliCommand::Quit);
        }

        #[test]
        fn notes() {
            assert_no_arg_command("notes", CliCommand::Notes { scope: None });
        }

        #[test]
        fn version() {
            assert_no_arg_command("version", CliCommand::Version);
        }

        #[test]
        fn rescan() {
            assert_no_arg_command("rescan", CliCommand::Rescan);
        }

        #[test]
        fn export_ufvk() {
            assert_no_arg_command("export_ufvk", CliCommand::ExportUfvk);
        }

        #[test]
        fn settings() {
            assert_no_arg_command("settings", CliCommand::Settings { sub: None });
        }

        #[test]
        fn value_transfers() {
            assert_no_arg_command("value_transfers", CliCommand::ValueTransfers);
        }

        #[test]
        fn transactions() {
            assert_no_arg_command("transactions", CliCommand::Transactions);
        }

        #[test]
        fn quickshield() {
            assert_no_arg_command("quickshield", CliCommand::Quickshield);
        }

        #[test]
        fn wallet_kind() {
            assert_no_arg_command("wallet_kind", CliCommand::WalletKind);
        }

        #[test]
        fn birthday() {
            assert_no_arg_command("birthday", CliCommand::Birthday);
        }

        #[test]
        fn delete() {
            assert_no_arg_command("delete", CliCommand::Delete);
        }

        /// A command the grammar does not know now refuses at the process's
        /// own argument parse, before any wallet work begins.
        #[test]
        fn an_unknown_command_refuses_at_the_argument_parse() {
            assert!(
                build_clap_app()
                    .try_get_matches_from([examples::BIN_NAME, "nonesuch"])
                    .is_err()
            );
        }
    }
}

mod communication_mode {
    use super::*;
    use crate::{CommunicationMode, get_communication_mode};

    /// A scratch data directory per test, so no stored Connectivity
    /// Consent leaks between tests or into the developer's real store.
    fn scratch_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("a scratch directory")
    }

    /// Resolve the communication mode for `extra` launch arguments against
    /// the scratch store.
    fn mode_with_dir(dir: &tempfile::TempDir, extra: &[&str]) -> CommunicationMode {
        let mut args = vec![
            examples::BIN_NAME,
            "--data-dir",
            dir.path().to_str().expect("utf-8 temp path"),
        ];
        args.extend_from_slice(extra);
        get_communication_mode(&parse(&args)).expect("the mode resolves")
    }

    /// ADR 0025: first boot is offline. With no stored choice and no
    /// consent act, the session must not touch the network.
    #[test]
    fn first_boot_without_consent_is_offline() {
        let dir = scratch_dir();
        assert_eq!(
            mode_with_dir(&dir, &[]),
            CommunicationMode::UnconsentedOffline
        );
    }

    #[test]
    fn offline_flag_pins_offline() {
        let dir = scratch_dir();
        assert_eq!(
            mode_with_dir(&dir, &["--offline"]),
            CommunicationMode::DeliberateOffline
        );
    }

    /// The per-session consent act: --online takes this session online and
    /// stores nothing, so the next default launch is offline again.
    #[cfg(feature = "nym")]
    #[test]
    fn online_flag_consents_this_session_only() {
        let dir = scratch_dir();
        assert_eq!(
            mode_with_dir(&dir, &["--online"]),
            CommunicationMode::Online
        );
        assert_eq!(
            mode_with_dir(&dir, &[]),
            CommunicationMode::UnconsentedOffline,
            "an un-stored act must not outlive its session"
        );
    }

    /// Naming an endpoint is consenting to connect to it: an explicit
    /// --server is a consent act (ADR 0025).
    #[cfg(feature = "nym")]
    #[test]
    fn an_explicit_server_is_a_consent_act() {
        let dir = scratch_dir();
        assert_eq!(
            mode_with_dir(&dir, &["--server", examples::SERVER_URI]),
            CommunicationMode::Online
        );
    }

    /// The standing choice: --remember-online stores the consent, and a
    /// later launch with no acts attaches automatically.
    #[cfg(feature = "nym")]
    #[test]
    fn remember_online_stores_the_standing_choice() {
        let dir = scratch_dir();
        assert_eq!(
            mode_with_dir(&dir, &["--remember-online"]),
            CommunicationMode::Online
        );
        assert_eq!(
            mode_with_dir(&dir, &[]),
            CommunicationMode::Online,
            "the stored choice attaches later sessions automatically"
        );
    }

    /// --forget-online removes the standing choice: the forgetting session
    /// runs offline (no other act was expressed), and so does the next.
    #[cfg(feature = "nym")]
    #[test]
    fn forget_online_returns_the_store_to_first_boot() {
        let dir = scratch_dir();
        mode_with_dir(&dir, &["--remember-online"]);
        assert_eq!(
            mode_with_dir(&dir, &["--forget-online"]),
            CommunicationMode::UnconsentedOffline
        );
        assert_eq!(
            mode_with_dir(&dir, &[]),
            CommunicationMode::UnconsentedOffline
        );
    }

    /// Forgetting the store and consenting for the session compose: the
    /// launch goes online once while the standing choice dies.
    #[cfg(feature = "nym")]
    #[test]
    fn forget_online_composes_with_a_session_act() {
        let dir = scratch_dir();
        mode_with_dir(&dir, &["--remember-online"]);
        assert_eq!(
            mode_with_dir(&dir, &["--forget-online", "--online"]),
            CommunicationMode::Online
        );
        assert_eq!(
            mode_with_dir(&dir, &[]),
            CommunicationMode::UnconsentedOffline
        );
    }

    /// The deliberate --offline outranks even a stored standing choice.
    #[cfg(feature = "nym")]
    #[test]
    fn offline_flag_wins_over_the_stored_choice() {
        let dir = scratch_dir();
        mode_with_dir(&dir, &["--remember-online"]);
        assert_eq!(
            mode_with_dir(&dir, &["--offline"]),
            CommunicationMode::DeliberateOffline
        );
    }

    /// Without the mixnet capability, Offline Mode is the only mode (ADR
    /// 0026): every online act refuses, a stored standing consent is
    /// inert, and `--forget-online` still retires it.
    #[cfg(not(feature = "nym"))]
    mod offline_only {
        use super::*;

        #[test]
        fn every_online_act_refuses() {
            let dir = scratch_dir();
            for act in [
                vec!["--online"],
                vec!["--remember-online"],
                vec!["--server", examples::SERVER_URI],
            ] {
                let mut args = vec![
                    examples::BIN_NAME,
                    "--data-dir",
                    dir.path().to_str().expect("utf-8 temp path"),
                ];
                args.extend(act.iter());
                let err = get_communication_mode(&parse(&args))
                    .expect_err("an offline-only build must refuse every online act");
                assert!(
                    err.to_string().contains("Offline Mode is its only mode"),
                    "{err}"
                );
            }
        }

        #[test]
        fn a_stored_standing_consent_is_inert() {
            let dir = scratch_dir();
            zingolib::connectivity::store_standing_online(dir.path())
                .expect("the store writes in a scratch directory");
            assert_eq!(
                mode_with_dir(&dir, &[]),
                CommunicationMode::UnconsentedOffline
            );
        }

        #[test]
        fn forget_online_still_retires_a_stored_consent() {
            let dir = scratch_dir();
            zingolib::connectivity::store_standing_online(dir.path())
                .expect("the store writes in a scratch directory");
            assert_eq!(
                mode_with_dir(&dir, &["--forget-online"]),
                CommunicationMode::UnconsentedOffline
            );
            assert!(matches!(
                zingolib::connectivity::load_connectivity_consent(dir.path()),
                zingolib::connectivity::ConnectivityConsent::Unrecorded
            ));
        }
    }

    #[test]
    fn online_conflicts_with_offline() {
        assert!(
            build_clap_app()
                .try_get_matches_from([examples::BIN_NAME, "--offline", "--online"])
                .is_err()
        );
    }

    #[test]
    fn remember_online_conflicts_with_offline_and_forget() {
        assert!(
            build_clap_app()
                .try_get_matches_from([examples::BIN_NAME, "--offline", "--remember-online"])
                .is_err()
        );
        assert!(
            build_clap_app()
                .try_get_matches_from([examples::BIN_NAME, "--remember-online", "--forget-online"])
                .is_err()
        );
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
        let communication_mode = get_communication_mode(&matches).map_err(|e| e.to_string())?;
        ConfigTemplate::fill(mode, communication_mode, matches).map_err(|e| e.to_string())
    }

    /// Helper: build the ZingoConfig for a filled template. The builder is
    /// async, so the tests hold their own crossing into the runtime.
    #[allow(clippy::disallowed_methods)]
    fn build(filled: &ConfigTemplate) -> zingolib::config::ClientConfig {
        crate::commands::RT
            .block_on(build_zingo_config(filled))
            .unwrap()
    }

    /// Helper: parse args, fill config, and build ZingoConfig in one step.
    fn fill_and_build(args: &[&str]) -> zingolib::config::ClientConfig {
        build(&fill(args).unwrap())
    }

    mod offline {
        use super::*;
        use crate::CommunicationMode;

        #[test]
        fn fill_resolves_no_server_and_disables_sync() {
            let config = fill(&[examples::BIN_NAME, "--offline"]).unwrap();
            assert_eq!(
                config.communication_mode,
                CommunicationMode::DeliberateOffline
            );
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
        /// tip. Offline mode has no server, so the Library Birthday (a
        /// release-stamped height no new seed can predate) stands in
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
            let config = build(&filled);
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
            let config = build(&filled);
            assert!(matches!(
                config.wallet_config(),
                zingolib::config::WalletConfig::NewSeed { chain_height, .. }
                    if chain_height == 3_500_000
            ));
        }
    }

    mod happy_paths {
        use super::*;
        // Consumed only by the nym-gated `defaults` test: the offline-only
        // build never reaches an Online communication mode (ADR 0026).
        #[cfg(feature = "nym")]
        use crate::CommunicationMode;

        /// An explicit `--server` is an online act, which only nym builds
        /// accept (ADR 0026).
        #[cfg(feature = "nym")]
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

        /// `--online` exists as a consented act only in nym builds; the
        /// offline-only build refuses it (ADR 0026), pinned in
        /// `online_acts_refuse_in_the_offline_only_build`.
        #[cfg(feature = "nym")]
        #[test]
        fn nosync_flag() {
            // --online keeps this a test of the flag, not of the offline
            // default (an unconsented session disables sync by itself).
            let config = fill(&[examples::BIN_NAME, "--online", "--nosync"]).unwrap();
            assert!(!config.sync);
        }

        /// `--online` exists as a consented act only in nym builds; the
        /// offline-only build refuses it (ADR 0026).
        #[cfg(feature = "nym")]
        #[test]
        fn waitsync_flag() {
            let config = fill(&[examples::BIN_NAME, "--online", "--waitsync"]).unwrap();
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
                    command: crate::commands::CliCommand::Balance,
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

        /// The URI shape check sits on the online resolution path, which
        /// only nym builds reach: the offline-only build refuses the
        /// `--server` act before any URI is inspected (ADR 0026).
        #[cfg(feature = "nym")]
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

        /// Going online at all requires the mixnet capability (ADR 0026),
        /// so the propagation contract exists only in nym builds.
        #[cfg(feature = "nym")]
        #[test]
        fn unpinned_online_configures_no_indexer() {
            // --online is the consent act; with no --server there is no
            // default to fill in, and the sweep selects the sync indexer.
            let zc = fill_and_build(&[
                examples::BIN_NAME,
                "--online",
                "--seed",
                HOSPITAL_MUSEUM_SEED,
                "--birthday",
                "1",
            ]);
            assert!(
                zc.indexer_uri().is_none(),
                "an unpinned online session must configure no indexer, got: {:?}",
                zc.indexer_uri()
            );
        }

        /// Going online at all requires the mixnet capability (ADR 0026),
        /// so the propagation contract exists only in nym builds.
        #[cfg(feature = "nym")]
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

        /// Without the mixnet capability, Offline Mode is the only mode
        /// (ADR 0026): every online launch act refuses instead of
        /// configuring a server.
        #[cfg(not(feature = "nym"))]
        #[test]
        fn online_acts_refuse_in_the_offline_only_build() {
            for act in [
                vec!["--online"],
                vec!["--remember-online"],
                vec!["--server", examples::SERVER_URI],
            ] {
                let mut args = vec![examples::BIN_NAME];
                args.extend(act.iter());
                args.extend(["--seed", HOSPITAL_MUSEUM_SEED, "--birthday", "1"]);
                let err =
                    fill(&args).expect_err("an offline-only build must refuse every online act");
                assert!(err.contains("Offline Mode is its only mode"), "{err}");
            }
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
                            shutdown_on_completion: false,
                        },
                        min_confirmations: NonZeroU32::try_from(3).unwrap(),
                    },
                }
            );
        }
    }

    /// A one-shot `--online <command>` is valid only for a command that
    /// requires online; an offline-capable command after `--online` is
    /// refused at fill, before any network or wallet work. The flag is an
    /// online consent act, which only nym builds accept (ADR 0026).
    #[cfg(feature = "nym")]
    mod online_one_shot {
        use super::*;

        #[test]
        fn an_offline_capable_command_after_online_is_refused() {
            let error = fill(&[examples::BIN_NAME, "--online", "addresses"])
                .expect_err("addresses needs no network");
            assert!(error.contains("needs no network"), "{error}");
            assert!(error.contains("addresses"), "{error}");
        }

        #[test]
        fn an_online_requiring_command_after_online_is_accepted() {
            let config = fill(&[examples::BIN_NAME, "--online", "sync", "run"])
                .expect("sync run requires online");
            assert!(matches!(config.mode, ModeOfOperation::Command { .. }));
        }

        #[test]
        fn an_offline_capable_command_without_online_is_accepted() {
            // The gate keys on the flag, not the command: `addresses` is a
            // legitimate offline one-shot.
            fill(&[examples::BIN_NAME, "addresses"]).expect("addresses is a fine offline one-shot");
        }

        #[test]
        fn online_alone_starts_an_interactive_session() {
            // No command means no one-shot to judge; `--online` opens a
            // connected prompt.
            let config = fill(&[examples::BIN_NAME, "--online"]).expect("interactive online");
            assert!(matches!(config.mode, ModeOfOperation::Interactive));
        }
    }
}

mod offline_mode_pin {
    //! The dispatch half of the Offline-mode contract (ADR 0032): an
    //! offline posture refuses the whole network-requiring surface before
    //! command execution, naming the live remedy, and spares every
    //! Indexerless capability. The command-surface half lives in
    //! `commands::offline_contract`.

    use crate::commands::{
        CliCommand, DrainSubCommand, MigrationSubCommand, SplitSubCommand, SyncSubCommand,
    };
    use crate::{CommunicationMode, offline_mode_refusal};

    /// One sample per network-requiring shape the gate must refuse.
    fn network_requiring() -> Vec<CliCommand> {
        vec![
            CliCommand::ChangeServer { uri: None },
            CliCommand::Confirm,
            CliCommand::CurrentPrice,
            CliCommand::Info,
            CliCommand::Migrate,
            CliCommand::Quicksend { args: Vec::new() },
            CliCommand::Quickshield,
            CliCommand::Rescan,
            CliCommand::Transmit { txids: Vec::new() },
            CliCommand::Sync {
                sub: SyncSubCommand::Run,
            },
            CliCommand::Drain {
                sub: DrainSubCommand::Now,
            },
            CliCommand::Split {
                sub: SplitSubCommand::Now,
            },
            CliCommand::Migration {
                sub: MigrationSubCommand::Continue,
            },
        ]
    }

    /// One sample per Indexerless capability the gate must always spare.
    fn indexerless() -> Vec<CliCommand> {
        vec![
            CliCommand::Balance,
            CliCommand::Calculate,
            CliCommand::Height,
            CliCommand::Help { command: None },
            CliCommand::Send { args: Vec::new() },
            CliCommand::Servers,
            CliCommand::Sync {
                sub: SyncSubCommand::Status,
            },
            CliCommand::Drain {
                sub: DrainSubCommand::Plan,
            },
            CliCommand::Split {
                sub: SplitSubCommand::Plan,
            },
            CliCommand::Migration {
                sub: MigrationSubCommand::Plan,
            },
            CliCommand::Migration {
                sub: MigrationSubCommand::Status,
            },
            CliCommand::Migration {
                sub: MigrationSubCommand::Windows,
            },
            CliCommand::Version,
        ]
    }

    /// HYPOTHESIS: both offline postures refuse every network-requiring
    /// command at the gate, and each refusal names the posture's remedy.
    #[test]
    fn offline_postures_refuse_the_network_requiring_surface() {
        for command in network_requiring() {
            let deliberate = offline_mode_refusal(CommunicationMode::DeliberateOffline, &command)
                .unwrap_or_else(|| panic!("`{}` must be refused under --offline", command.name()));
            let unconsented = offline_mode_refusal(CommunicationMode::UnconsentedOffline, &command)
                .unwrap_or_else(|| panic!("`{}` must be refused without consent", command.name()));
            #[cfg(feature = "nym")]
            {
                assert!(
                    deliberate.contains("relaunch without --offline"),
                    "{deliberate}"
                );
                assert!(unconsented.contains("`network on`"), "{unconsented}");
            }
            #[cfg(not(feature = "nym"))]
            for refusal in [&deliberate, &unconsented] {
                assert!(
                    refusal.contains("Rebuild with default features"),
                    "{refusal}"
                );
            }
        }
    }

    /// HYPOTHESIS: the network family is suppressed only by the deliberate
    /// `--offline`; an unconsented session keeps `network on` as its act.
    #[cfg(feature = "nym")]
    #[test]
    fn the_network_family_survives_only_where_consent_can_be_granted() {
        let network = CliCommand::Network { sub: None };
        assert!(
            offline_mode_refusal(CommunicationMode::DeliberateOffline, &network).is_some(),
            "--offline suppresses the whole network family"
        );
        assert_eq!(
            offline_mode_refusal(CommunicationMode::UnconsentedOffline, &network),
            None,
            "`network on` is the unconsented session's consent act"
        );
        assert_eq!(
            offline_mode_refusal(CommunicationMode::Online, &network),
            None
        );
    }

    /// HYPOTHESIS: the gate spares every Indexerless capability in every
    /// posture, honoring the glossary's Offline-mode promise.
    #[test]
    fn every_indexerless_capability_passes_the_gate() {
        for command in indexerless() {
            for mode in [
                CommunicationMode::Online,
                CommunicationMode::DeliberateOffline,
                CommunicationMode::UnconsentedOffline,
            ] {
                assert_eq!(
                    offline_mode_refusal(mode, &command),
                    None,
                    "`{}` belongs to the Indexerless surface",
                    command.name()
                );
            }
        }
    }

    /// HYPOTHESIS: an Online session passes the whole surface.
    #[test]
    fn online_passes_the_whole_surface() {
        for command in network_requiring() {
            assert_eq!(
                offline_mode_refusal(CommunicationMode::Online, &command),
                None,
                "`{}` must pass online",
                command.name()
            );
        }
    }

    /// HYPOTHESIS: the launch notice names the only exit from a deliberate
    /// `--offline` session.
    #[test]
    fn the_launch_notice_names_the_only_exit() {
        assert!(
            crate::DELIBERATE_OFFLINE_NOTICE.contains("relaunch without --offline"),
            "{}",
            crate::DELIBERATE_OFFLINE_NOTICE
        );
    }
}

#[cfg(feature = "nym")]
mod sweep_refusal_notice {
    use zingolib::lightclient::select::ServerSelectionError;
    use zingolib::mixnet::MixnetProxyError;

    /// HYPOTHESIS: the refusal notice states the whole source chain of a
    /// source-only sweep failure, so a reader learns the cause; a notice
    /// carrying only the outermost line falsifies it.
    #[test]
    fn the_refusal_states_the_cause_of_a_source_only_failure() {
        let refused = ServerSelectionError::ProxyStart(MixnetProxyError::NoStdout);
        assert_eq!(
            crate::sweep_refusal_notice(&refused),
            "Server-Selection Sweep: no sync indexer selected: the sweep proxy could not \
             start\ncaused by: the nym-proxy child exposed no stdout. This Sync Session does \
             not open; the mixnet posture stands, and send and price-fetch continue."
        );
    }
}
