#[cfg(test)]
mod table_invariants {
    //! Pins the properties [`super::super::CliCommand`] relies on but the
    //! compiler does not check: declaration order is the help listing's
    //! order, and no two variants mint the same name.

    use clap::CommandFactory as _;

    use super::super::{CliCommand, CommandLine, every_command, format_help};

    /// HYPOTHESIS: the derived names of one-sample-per-variant equal the
    /// clap model's subcommand names exactly, so `name` can never diverge
    /// from the mint and the sample list can never go stale.
    #[test]
    fn every_variant_names_the_mint() {
        let model = CommandLine::command();
        let mut derived: Vec<String> = every_command().iter().map(CliCommand::name).collect();
        derived.sort();
        let mut minted: Vec<String> = model
            .get_subcommands()
            .map(|sub| sub.get_name().to_string())
            .collect();
        minted.sort();
        assert_eq!(derived, minted);
    }

    /// HYPOTHESIS: `help` files every command in the section
    /// `requires_wallet` dictates, proven by the rendered listing itself
    /// rather than any debug-only assertion.
    #[test]
    fn help_sections_agree_with_requires_wallet() {
        let listing = format_help(crate::Communications::Online, None);
        let wallet_header = listing
            .find("Wallet commands:")
            .expect("the listing carries a wallet section");
        for command in every_command() {
            let name = command.name();
            let entry = format!("  {name} - ");
            let position = listing
                .find(&entry)
                .unwrap_or_else(|| panic!("`{name}` must appear in the help listing"));
            assert_eq!(
                position > wallet_header,
                command.requires_wallet(),
                "`{name}` sits in the wrong help section"
            );
        }
    }

    /// HYPOTHESIS: every rendered usage line, hand-written or generated,
    /// invokes its command by the minted name, so an `override_usage`
    /// cannot drift from a renamed variant.
    #[test]
    fn usage_lines_invoke_the_minted_name() {
        let mut model = CommandLine::command();
        let names: Vec<String> = model
            .get_subcommands()
            .map(|sub| sub.get_name().to_string())
            .collect();
        for name in names {
            let usage = model
                .find_subcommand_mut(&name)
                .expect("the name was just minted")
                .render_usage()
                .to_string();
            for line in usage.lines() {
                let line = line.trim().trim_start_matches("Usage:").trim_start();
                if line.is_empty() {
                    continue;
                }
                assert!(
                    line == name || line.starts_with(&format!("{name} ")),
                    "`{name}`'s usage line drifts from the mint: {line}"
                );
            }
        }
    }

    /// HYPOTHESIS: `name` derives exactly the minted subcommand name, so a
    /// log line names the command without carrying its arguments.
    #[test]
    fn name_matches_the_mint_and_drops_the_arguments() {
        let model = CommandLine::command();
        for (command, expected) in [
            (CliCommand::Quit, "quit"),
            (
                CliCommand::SendAll {
                    args: vec!["a-secret-memo".to_string()],
                },
                "send_all",
            ),
            (CliCommand::NewTaddressAllowGap, "new_taddress_allow_gap"),
        ] {
            assert_eq!(command.name(), expected);
            assert!(
                model.find_subcommand(expected).is_some(),
                "`{expected}` must be a minted name"
            );
        }
    }

    /// HYPOTHESIS: the grammar's minted names are strictly increasing —
    /// sorted, so the help listing stays alphabetical, and therefore
    /// unique, so no variant can shadow another at dispatch.
    #[test]
    fn names_are_strictly_increasing() {
        let model = CommandLine::command();
        let names: Vec<&str> = model
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect();
        for pair in names.windows(2) {
            assert!(
                pair[0] < pair[1],
                "CliCommand must stay sorted and duplicate-free: {:?} precedes {:?}",
                pair[0],
                pair[1]
            );
        }
    }
}

#[cfg(test)]
mod progress_heartbeat {
    //! Paused-clock falsifiers for the dispatch-seam progress heartbeat's
    //! contract: silence for fast commands, a narrated line on the shared
    //! eight-second cadence for slow ones, always carrying the side
    //! channels' latest detail.
    //!
    //! Seam justification (ADR 0030): the `block_on` here is the one
    //! `#[tokio::test]` generates to drive each async test body; a test
    //! driver is a sync frontend, so it is an audited crossing.
    #![allow(clippy::disallowed_methods)]

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use zingo_netutils::time::PROGRESS_HEARTBEAT_INTERVAL;

    use super::super::*;

    /// HYPOTHESIS: a command finishing before the first tick emits nothing,
    /// because the heartbeat must not add noise to a normal fast command.
    #[tokio::test(start_paused = true)]
    async fn a_fast_command_stays_silent() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = lines.clone();
        let out = with_heartbeat(
            "confirm",
            PROGRESS_HEARTBEAT_INTERVAL,
            "working",
            || Some("submitting".to_string()),
            move |line| sink.lock().expect("line sink poisoned").push(line),
            tokio::time::sleep(PROGRESS_HEARTBEAT_INTERVAL / 2),
        )
        .await;
        let () = out;
        assert!(
            lines.lock().expect("line sink poisoned").is_empty(),
            "no heartbeat before the first interval"
        );
    }

    /// HYPOTHESIS: a slow command is narrated on the interval cadence, each
    /// line carrying the label, the side channels' latest detail, and the
    /// elapsed seconds. Falsified if the wait stays silent or drops the
    /// detail. Expectations derive from the interval so the pin holds at
    /// any ratified cadence.
    #[tokio::test(start_paused = true)]
    async fn a_slow_command_heartbeats_the_latest_detail() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = lines.clone();
        with_heartbeat(
            "confirm",
            PROGRESS_HEARTBEAT_INTERVAL,
            "working",
            || Some("correspondent zec.rocks: submitting".to_string()),
            move |line| sink.lock().expect("line sink poisoned").push(line),
            tokio::time::sleep(PROGRESS_HEARTBEAT_INTERVAL * 3 + Duration::from_millis(500)),
        )
        .await;
        let lines = lines.lock().expect("line sink poisoned").clone();
        let expected: Vec<String> = (1..=3)
            .map(|tick| {
                format!(
                    "confirm: correspondent zec.rocks: submitting ({}s elapsed)",
                    PROGRESS_HEARTBEAT_INTERVAL.as_secs() * tick
                )
            })
            .collect();
        assert_eq!(lines, expected);
    }

    /// A phase that publishes no detail still heartbeats, falling back to
    /// the generic line rather than going silent past an interval.
    #[tokio::test(start_paused = true)]
    async fn an_empty_side_channel_still_heartbeats() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = lines.clone();
        with_heartbeat(
            "transmit",
            PROGRESS_HEARTBEAT_INTERVAL,
            "working",
            || None,
            move |line| sink.lock().expect("line sink poisoned").push(line),
            tokio::time::sleep(PROGRESS_HEARTBEAT_INTERVAL + Duration::from_millis(500)),
        )
        .await;
        assert_eq!(
            lines.lock().expect("line sink poisoned").clone(),
            vec![format!(
                "transmit: working ({}s elapsed)",
                PROGRESS_HEARTBEAT_INTERVAL.as_secs()
            )]
        );
    }
}

#[cfg(test)]
mod migration_command_parsing {
    //! Pins the clap derive grammar of the `migration` family: typed
    //! outcomes, the spacing defaults, and refusal at the parse boundary.

    use clap::Parser as _;

    use super::super::*;

    fn parse(args: &[&str]) -> Result<MigrationSubCommand, clap::Error> {
        let line = std::iter::once("migration").chain(args.iter().copied());
        CommandLine::try_parse_from(line).map(|line| match line.command {
            CliCommand::Migration { sub } => sub,
            other => panic!("`migration` must parse to the migration family: {other:?}"),
        })
    }

    #[test]
    fn start_parses_hash_and_per_bucket() {
        let hash_hex = "11".repeat(32);
        assert_eq!(
            parse(&["start", &hash_hex, "--per-bucket", "3"]).expect("well-formed arguments parse"),
            MigrationSubCommand::Start {
                plan_hash: [0x11; 32],
                per_bucket: Some(3),
            }
        );
    }

    #[test]
    fn malformed_plan_hash_is_refused_at_parse() {
        assert!(parse(&["start", "abc"]).is_err());
    }

    #[test]
    fn continue_parses_bare() {
        assert_eq!(
            parse(&["continue"]).expect("bare continue parses"),
            MigrationSubCommand::Continue
        );
    }

    #[test]
    fn windows_parses_bare() {
        assert_eq!(
            parse(&["windows"]).expect("bare windows parses"),
            MigrationSubCommand::Windows
        );
    }

    #[test]
    fn cadence_requires_a_count() {
        assert_eq!(
            parse(&["cadence", "4"]).expect("well-formed cadence parses"),
            MigrationSubCommand::Cadence { per_bucket: 4 }
        );
        assert!(parse(&["cadence"]).is_err());
    }

    #[test]
    fn execute_defaults_spacing_to_thirty_seconds() {
        assert_eq!(
            parse(&["execute"]).expect("bare execute parses"),
            MigrationSubCommand::Execute {
                spacing: std::time::Duration::from_secs(30),
            }
        );
        assert_eq!(
            parse(&["execute", "5"]).expect("spaced execute parses"),
            MigrationSubCommand::Execute {
                spacing: std::time::Duration::from_secs(5),
            }
        );
    }

    #[test]
    fn catchup_defaults_spacing_to_thirty_seconds() {
        assert_eq!(
            parse(&["catchup"]).expect("bare catchup parses"),
            MigrationSubCommand::Catchup {
                spacing: std::time::Duration::from_secs(30),
            }
        );
    }

    #[test]
    fn missing_and_unknown_subcommands_are_refused_at_parse() {
        assert!(parse(&[]).is_err());
        assert!(parse(&["bogus"]).is_err());
    }
}

#[cfg(test)]
mod drain_and_split_command_parsing {
    //! Pins the clap derive grammars of the mobile-parity migration
    //! commands: `drain` and `split` accept exactly `plan` or `now`,
    //! refusing everything else at the parse boundary.

    use clap::Parser as _;

    use super::super::*;

    #[test]
    fn drain_accepts_exactly_plan_or_now() {
        let parse = |args: &[&str]| {
            let line = std::iter::once("drain").chain(args.iter().copied());
            CommandLine::try_parse_from(line).map(|line| match line.command {
                CliCommand::Drain { sub } => sub,
                other => panic!("`drain` must parse to the drain family: {other:?}"),
            })
        };
        assert_eq!(
            parse(&["plan"]).expect("drain plan parses"),
            DrainSubCommand::Plan
        );
        assert_eq!(
            parse(&["now"]).expect("drain now parses"),
            DrainSubCommand::Now
        );
        for junk in [&[][..], &["bogus"][..], &["now", "extra"][..]] {
            assert!(parse(junk).is_err());
        }
    }

    #[test]
    fn split_accepts_exactly_plan_or_now() {
        let parse = |args: &[&str]| {
            let line = std::iter::once("split").chain(args.iter().copied());
            CommandLine::try_parse_from(line).map(|line| match line.command {
                CliCommand::Split { sub } => sub,
                other => panic!("`split` must parse to the split family: {other:?}"),
            })
        };
        assert_eq!(
            parse(&["plan"]).expect("split plan parses"),
            SplitSubCommand::Plan
        );
        assert_eq!(
            parse(&["now"]).expect("split now parses"),
            SplitSubCommand::Now
        );
        for junk in [&[][..], &["bogus"][..], &["plan", "extra"][..]] {
            assert!(parse(junk).is_err());
        }
    }
}

#[cfg(test)]
mod typed_argument_parsing {
    //! Pins the typed payloads the top-level grammar owns: the arguments
    //! that once reached a body as `&[&str]` now refuse, or arrive whole,
    //! at the parse boundary.

    use clap::Parser as _;

    use super::super::*;

    fn parse(args: &[&str]) -> Result<CliCommand, clap::Error> {
        CommandLine::try_parse_from(args.iter().copied()).map(|line| line.command)
    }

    /// HYPOTHESIS: `settings` parses its whole grammar before the body
    /// takes the wallet lock, so a malformed level or confirmation count
    /// never reaches the wallet.
    #[test]
    fn settings_parses_before_it_takes_the_wallet_lock() {
        assert!(matches!(
            parse(&["settings"]).expect("a bare settings parses"),
            CliCommand::Settings { sub: None }
        ));
        assert!(matches!(
            parse(&["settings", "performance", "high"]).expect("a level parses"),
            CliCommand::Settings {
                sub: Some(SettingsSubCommand::Performance {
                    level: PerformanceLevelArg::High
                })
            }
        ));
        assert!(matches!(
            parse(&["settings", "min_confirmations", "3"]).expect("a count parses"),
            CliCommand::Settings {
                sub: Some(SettingsSubCommand::MinConfirmations { .. })
            }
        ));
        assert!(matches!(
            parse(&["settings", "transparent_gap_limit", "20"]).expect("a gap limit parses"),
            CliCommand::Settings {
                sub: Some(SettingsSubCommand::TransparentGapLimit { gap_limit: 20, .. })
            }
        ));
        for junk in [
            &["settings", "bogus"][..],
            &["settings", "performance"][..],
            &["settings", "performance", "blazing"][..],
            &["settings", "min_confirmations", "0"][..],
            &["settings", "min_confirmations", "many"][..],
        ] {
            assert!(parse(junk).is_err(), "{junk:?} must refuse at the parse");
        }
    }

    /// HYPOTHESIS: `notes` and `coins` take `all` and nothing else.
    #[test]
    fn the_spent_output_scope_is_all_or_nothing() {
        assert!(matches!(
            parse(&["notes", "all"]).expect("notes all parses"),
            CliCommand::Notes {
                scope: Some(OutputScope::All)
            }
        ));
        assert!(matches!(
            parse(&["coins"]).expect("bare coins parses"),
            CliCommand::Coins { scope: None }
        ));
        assert!(parse(&["notes", "spent"]).is_err());
        assert!(parse(&["coins", "all", "extra"]).is_err());
    }

    /// HYPOTHESIS: `new_address` takes the receivers as a type, so a
    /// receiver set the wallet cannot build refuses at the parse.
    #[test]
    fn new_address_parses_its_receivers() {
        assert!(matches!(
            parse(&["new_address", "oz"]).expect("oz parses"),
            CliCommand::NewAddress {
                receivers: ReceiverSelection {
                    orchard: true,
                    sapling: true,
                }
            }
        ));
        assert!(matches!(
            parse(&["new_address", "z"]).expect("z parses"),
            CliCommand::NewAddress {
                receivers: ReceiverSelection {
                    orchard: false,
                    sapling: true,
                }
            }
        ));
        for junk in [
            &["new_address"][..],
            &["new_address", "t"][..],
            &["new_address", "o", "z"][..],
        ] {
            assert!(parse(junk).is_err(), "{junk:?} must refuse at the parse");
        }
    }

    /// HYPOTHESIS: the transaction-id arguments arrive decoded, so a
    /// malformed id never reaches the wallet.
    #[test]
    fn transaction_ids_parse_at_the_boundary() {
        let txid = "ab".repeat(32);
        assert!(matches!(
            parse(&["transmit"]).expect("a bare transmit parses"),
            CliCommand::Transmit { txids } if txids.is_empty()
        ));
        assert!(matches!(
            parse(&["transmit", &txid, &txid]).expect("two txids parse"),
            CliCommand::Transmit { txids } if txids.len() == 2
        ));
        assert!(parse(&["transmit", "nonsense"]).is_err());
        assert!(parse(&["remove_transaction", "nonsense"]).is_err());
        assert!(parse(&["remove_transaction"]).is_err());
    }

    /// HYPOTHESIS: `change_server` takes a uri, and an empty argument
    /// still names the default one.
    #[test]
    fn change_server_parses_its_uri() {
        assert!(matches!(
            parse(&["change_server"]).expect("a bare change_server parses"),
            CliCommand::ChangeServer { uri: None }
        ));
        assert!(matches!(
            parse(&["change_server", ""]).expect("an empty uri parses"),
            CliCommand::ChangeServer { uri: Some(uri) } if uri == http::Uri::default()
        ));
        assert!(matches!(
            parse(&["change_server", "https://zec.rocks:443"]).expect("a uri parses"),
            CliCommand::ChangeServer { uri: Some(_) }
        ));
        assert!(parse(&["change_server", "zec rocks"]).is_err());
    }

    /// HYPOTHESIS: a name the grammar does not know refuses at the parse,
    /// where clap's unknown-subcommand error replaces the hand-written
    /// one the table used to raise.
    #[test]
    fn an_unknown_command_refuses_at_the_parse() {
        assert!(parse(&["bogus"]).is_err());
        assert!(parse(&[]).is_err());
    }

    /// HYPOTHESIS: a flag-shaped token after send-family arguments refuses
    /// at the parse instead of becoming the transaction's memo.
    #[test]
    fn a_flag_after_send_arguments_refuses_at_the_parse() {
        for family in ["send", "send_all", "quicksend", "max_send_value"] {
            assert!(
                parse(&[family, "zs1exampleaddress", "50000", "--nosync"]).is_err(),
                "`{family}` must refuse a flag-shaped trailing token"
            );
        }
    }

    /// HYPOTHESIS: the standard `--` escape carries a dash-leading memo
    /// into the send arguments.
    #[test]
    fn the_escape_carries_a_dash_leading_memo() {
        assert_eq!(
            parse(&["send", "zs1exampleaddress", "50000", "--", "-memo"])
                .expect("an escaped dash-leading memo parses"),
            CliCommand::Send {
                args: ["zs1exampleaddress", "50000", "-memo"]
                    .map(String::from)
                    .to_vec(),
            }
        );
    }

    /// HYPOTHESIS: `--help` on `messages` renders help, never a filter,
    /// because defined flags outrank hyphen values.
    #[test]
    fn messages_help_outranks_the_hyphen_filter() {
        let error = parse(&["messages", "--help"]).expect_err("--help renders help");
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    /// HYPOTHESIS: a memo filter beginning with a dash rides the standard
    /// `--` escape, while a bare flag-shaped token refuses.
    #[test]
    fn a_dash_leading_messages_filter_rides_the_escape() {
        assert_eq!(
            parse(&["messages", "--", "-1ZEC"]).expect("an escaped dash-leading filter parses"),
            CliCommand::Messages {
                filter: Some("-1ZEC".to_string()),
            }
        );
        assert!(parse(&["messages", "-1ZEC"]).is_err());
    }

    /// HYPOTHESIS: the `network` command exists only with the mixnet
    /// capability compiled in, so its subcommands parse there and nowhere
    /// else.
    #[cfg(feature = "nym")]
    #[test]
    fn network_subcommands_parse_with_the_capability() {
        assert_eq!(
            parse(&["network", "status"]).expect("network status parses"),
            CliCommand::Network {
                sub: Some(NetworkSubCommand::Status),
            }
        );
    }

    /// HYPOTHESIS: the opt-out build's grammar has no `network` command,
    /// so no command exists that could change the session's posture.
    #[cfg(not(feature = "nym"))]
    #[test]
    fn network_is_absent_from_the_opt_out_grammar() {
        assert!(parse(&["network", "status"]).is_err());
    }
}

#[cfg(all(test, feature = "nym"))]
mod network_command_parsing {
    //! Pins the clap derive grammar of the `network` family and the pure
    //! renderers whose strings every frontend shares.

    use super::super::*;

    #[cfg(feature = "nym")]
    fn parse(args: &[&str]) -> Result<Option<NetworkSubCommand>, clap::Error> {
        use clap::Parser as _;
        let line = std::iter::once("network").chain(args.iter().copied());
        CommandLine::try_parse_from(line).map(|line| match line.command {
            CliCommand::Network { sub } => sub,
            other => panic!("`network` must parse to the network family: {other:?}"),
        })
    }

    #[cfg(feature = "nym")]
    #[test]
    fn bare_parses_to_no_subcommand_and_status_to_status() {
        assert_eq!(parse(&[]).expect("a bare nym parses"), None);
        assert_eq!(
            parse(&["status"]).expect("network status parses"),
            Some(NetworkSubCommand::Status)
        );
    }

    #[cfg(feature = "nym")]
    #[test]
    fn on_captures_the_optional_path() {
        assert_eq!(
            parse(&["on"]).expect("bare network on parses"),
            Some(NetworkSubCommand::On { path: None })
        );
        assert_eq!(
            parse(&["on", "/opt/nym-proxy"]).expect("network on with a path parses"),
            Some(NetworkSubCommand::On {
                path: Some("/opt/nym-proxy".to_string()),
            })
        );
    }

    #[cfg(feature = "nym")]
    #[test]
    fn unknown_subcommands_are_refused_at_parse() {
        assert!(parse(&["bogus"]).is_err());
    }

    #[cfg(feature = "nym")]
    #[test]
    fn probe_parses_its_optional_target_and_rejects_junk() {
        assert_eq!(
            parse(&["probe"]).expect("bare probe parses"),
            Some(NetworkSubCommand::Probe { target: None })
        );
        assert_eq!(
            parse(&["probe", "https://zec.rocks:443"]).expect("probe with a uri parses"),
            Some(NetworkSubCommand::Probe {
                target: Some("https://zec.rocks:443".parse().expect("static uri")),
            })
        );
        assert!(parse(&["probe", "not a uri"]).is_err());
        assert!(
            parse(&["probe", "http://zec.rocks:9067"]).is_err(),
            "a plaintext http target is refused: mixnet transmission is https-only"
        );
        assert!(
            parse(&["probe", "https://zec.rocks:9067"]).is_err(),
            "an https target off port 443 is refused: the exit policy carries only 443"
        );
        assert_eq!(
            parse(&["history"]).expect("history parses"),
            Some(NetworkSubCommand::History)
        );
    }

    /// HYPOTHESIS: the mixnet-probe rendering carries the outcome, its
    /// timing, and the typed failure's full text. Falsified if any of the
    /// three is dropped.
    #[cfg(feature = "nym")]
    #[test]
    fn mixnet_probe_rendering_carries_outcome_timing_and_failure() {
        use zingo_net_diag::{NetOpFailure, NetOpStage};
        use zingolib::mixnet::probe::{MixnetProbe, ProbeLeg, ProbeSuccess};

        let live = MixnetProbe {
            host: zingolib::correspondent::Host::of_host_str("zec.rocks"),
            leg: ProbeLeg {
                outcome: Ok(ProbeSuccess {
                    chain: "main".to_string(),
                    height: 3_420_400,
                }),
                millis: 180,
            },
        };
        assert_eq!(
            render_mixnet_probe(&live),
            "zec.rocks\n  mixnet:   ok in 180ms: height 3420400"
        );

        let dead = MixnetProbe {
            host: zingolib::correspondent::Host::of_host_str("carover0.xyz"),
            leg: ProbeLeg {
                outcome: Err(NetOpFailure {
                    stage: NetOpStage::SocksHandshake,
                    target: "carover0.xyz".to_string(),
                    cause_chain: vec![
                        "the mixnet exit could not reach carover0.xyz:9067 (timed out after 20.0s)"
                            .to_string(),
                    ],
                }),
                millis: 20_000,
            },
        };
        assert_eq!(
            render_mixnet_probe(&dead),
            "carover0.xyz\n  mixnet:   FAILED after 20000ms: failed at socks-handshake to carover0.xyz: the mixnet exit could not reach carover0.xyz:9067 (timed out after 20.0s)"
        );
    }

    /// HYPOTHESIS: the history rendering aggregates per host and route with
    /// the most recent outcome and its age. Falsified if counts mix routes
    /// or the last outcome reflects insertion order rather than timestamps.
    #[cfg(feature = "nym")]
    #[test]
    fn history_aggregates_per_host_and_route() {
        use zingolib::lightclient::indexer_history::{
            AttemptKind, AttemptRoute, FailureKind, IndexerAttempt,
        };

        let attempt = |host: &str, route, unix_secs, outcome| IndexerAttempt {
            unix_secs,
            host: zingolib::correspondent::Host::of_host_str(host),
            route,
            kind: AttemptKind::Send,
            millis: 10,
            outcome,
            phase: None,
        };
        let tunnel = Err(FailureKind::Unreachable);
        let attempts = vec![
            attempt("zec.rocks", AttemptRoute::Mixnet, 1_000, tunnel),
            attempt("zec.rocks", AttemptRoute::Mixnet, 2_000, Ok(())),
            attempt("zec.rocks", AttemptRoute::Clearnet, 1_500, Ok(())),
            attempt("carover0.xyz", AttemptRoute::Mixnet, 1_800, tunnel),
        ];

        assert_eq!(
            render_history(&attempts, 2_060),
            "Indexer history (all sessions):\n  \
             carover0.xyz: mixnet 0/1 ok, last failed 4m ago\n  \
             zec.rocks: clearnet 1/1 ok, last ok 9m ago; mixnet 1/2 ok, last ok 1m ago"
        );
        assert_eq!(render_history(&[], 0), "No indexer history recorded yet.");
    }

    /// Pins the `network status` mode strings via the pure renderer.
    #[cfg(feature = "nym")]
    #[test]
    fn status_lines_render_byte_identically_to_the_replaced_strings() {
        use zingolib::mixnet::Indicator;

        assert_eq!(
            render_status(Indicator::Unattached, None, None),
            "Mixnet Mode: unattached. The mixnet has not been enabled, and no consent to \
             clearnet has been given: send and price-fetch refuse. Run `network on` to enable \
             the mixnet, or `network off` to use clearnet.",
            "absence is not consent: unattached names refusal, never clearnet"
        );
        assert_eq!(
            render_status(Indicator::SwitchedOff, None, None),
            "Mixnet Mode: switched off (send and price-fetch use clearnet)"
        );
        assert_eq!(
            render_status(Indicator::Bootstrapping, None, None),
            "Mixnet Mode: bootstrapping (send and price-fetch are unavailable until ready)"
        );
        assert_eq!(
            render_status(Indicator::Ready, Some("127.0.0.1:43210"), None),
            "Mixnet Mode: ready (SOCKS5 127.0.0.1:43210)"
        );
        assert_eq!(
            render_status(Indicator::Ready, None, None),
            "Mixnet Mode: ready",
            "ready with no address yet still renders (the route resolver, \
             not the renderer, refuses that state)"
        );
        assert_eq!(
            render_status(Indicator::Died, None, None),
            "Mixnet Mode: died. The proxy exited unexpectedly. Send and price-fetch \
             refuse and will not fall back to clearnet. Run `network on` to restart the proxy.",
            "a died proxy is reported distinctly from switched off, and tells the user how to \
             recover"
        );
    }

    /// HYPOTHESIS: live bootstrap progress reaches the `network status` line, so
    /// the connect race is narrated rather than an opaque wait. Falsified if
    /// the detail is dropped by the renderer. The detail is shown only while
    /// bootstrapping: a ready proxy has no bootstrap left to narrate.
    #[cfg(feature = "nym")]
    #[test]
    fn bootstrap_detail_reaches_the_status_line_only_while_bootstrapping() {
        use zingolib::mixnet::Indicator;

        assert_eq!(
            render_status(
                Indicator::Bootstrapping,
                None,
                Some("attempt 2/10: 2 in flight, 0 failed")
            ),
            "Mixnet Mode: bootstrapping, attempt 2/10: 2 in flight, 0 failed \
             (send and price-fetch are unavailable until ready)"
        );
        assert_eq!(
            render_status(Indicator::Ready, Some("127.0.0.1:1"), Some("stale")),
            "Mixnet Mode: ready (SOCKS5 127.0.0.1:1)",
            "a stale detail must not leak into the ready line"
        );
    }

    /// HYPOTHESIS: `network status` always carries the IP-correlation disclaimer in
    /// every mode, so a "ready" mixnet is never mistaken for end-to-end IP
    /// protection while synchronization stays on clearnet (ZIP-0318). The mode
    /// line is preserved verbatim as the first line. Falsified if the
    /// disclaimer is dropped in any mode, no longer leads with the mode line,
    /// or omits the sync/IP/indexer/balance risk it must name.
    #[cfg(feature = "nym")]
    #[test]
    fn status_always_carries_the_ip_correlation_disclaimer() {
        use zingolib::mixnet::Indicator;

        for mode in [
            Indicator::Unattached,
            Indicator::SwitchedOff,
            Indicator::Bootstrapping,
            Indicator::Ready,
            Indicator::Died,
        ] {
            let addr = Some("127.0.0.1:43210");
            let out = render_status_with_disclaimer(mode, addr, None);
            assert!(
                out.starts_with(&render_status(mode, addr, None)),
                "the mode line must lead the status output: {out}"
            );
            for phrase in [
                "IP-correlation risk",
                "synchronization",
                "sync indexer",
                "total balance",
                "ZIP-0318",
            ] {
                assert!(
                    out.contains(phrase),
                    "the disclaimer must name {phrase:?} in mode {mode:?}: {out}"
                );
            }
        }
    }
}

#[cfg(all(test, feature = "nym"))]
mod bootstrap_wait {
    //! Falsifiers for the `network on` bootstrap wait's outcome reader over
    //! the status subscription; the narration itself is the dispatch seam's
    //! progress heartbeat, pinned in `progress_heartbeat`.
    //!
    //! Seam justification (ADR 0030): the `block_on` here is the one
    //! `#[tokio::test]` generates to drive each async test body; a test
    //! driver is a sync frontend, so it is an audited crossing.
    #![allow(clippy::disallowed_methods)]

    use zingolib::mixnet::{Indicator, MixnetStatus};

    use super::super::{BootstrapOutcome, await_bootstrap_outcome};

    fn status(mode: Indicator) -> MixnetStatus {
        MixnetStatus {
            mode,
            socks5_addr: None,
            exits: Vec::new(),
            bootstrap_detail: None,
            death: None,
        }
    }

    /// HYPOTHESIS: the wait resolves `Ready` when the subscription reaches
    /// the ready mode, carrying the bound Exit Nodes, even from an initial
    /// unattached snapshot.
    #[tokio::test]
    async fn ready_resolves_the_wait_carrying_the_exits() {
        let (tx, rx) = tokio::sync::watch::channel(status(Indicator::Unattached));
        let waiter = tokio::spawn(await_bootstrap_outcome(rx));
        tokio::task::yield_now().await;
        tx.send(status(Indicator::Bootstrapping))
            .expect("the waiter holds the receiver");
        tokio::task::yield_now().await;
        let mut ready = status(Indicator::Ready);
        let exit_alpha =
            zingolib::mixnet::ExitNodeId::parse("exit-alpha").expect("the test identity parses");
        ready.exits = vec![exit_alpha.clone()];
        tx.send(ready).expect("the waiter holds the receiver");
        assert_eq!(
            waiter.await.expect("the waiter must not panic"),
            BootstrapOutcome::Ready {
                exits: vec![exit_alpha]
            }
        );
    }

    /// HYPOTHESIS: the success report names each bound Exit Node, shortened
    /// for the terminal, and stays silent when none was announced.
    #[test]
    fn exit_nodes_render_shortened_by_count() {
        assert_eq!(super::super::render_exit_nodes(&[]), "");
        let parsed = |identity: &str| {
            zingolib::mixnet::ExitNodeId::parse(identity).expect("the test identity parses")
        };
        assert_eq!(
            super::super::render_exit_nodes(&[parsed("short-exit")]),
            " Exit Node bound: short-exit."
        );
        assert_eq!(
            super::super::render_exit_nodes(&[
                parsed("AlphaBetaGammaDeltaEpsilon.ZetaEtaTheta"),
                parsed("short-exit"),
            ]),
            " Exit Nodes bound: AlphaBetaGam…, short-exit."
        );
    }

    /// HYPOTHESIS: a death during the wait resolves `Failed` with the died
    /// report rather than hanging until a timeout.
    #[tokio::test]
    async fn death_resolves_the_wait_as_failed() {
        let (tx, rx) = tokio::sync::watch::channel(status(Indicator::Bootstrapping));
        let waiter = tokio::spawn(await_bootstrap_outcome(rx));
        tokio::task::yield_now().await;
        tx.send(status(Indicator::Died))
            .expect("the waiter holds the receiver");
        let outcome = waiter.await.expect("the waiter must not panic");
        assert_eq!(
            outcome,
            BootstrapOutcome::Failed {
                report: "the mixnet transport died".to_string()
            }
        );
    }

    /// HYPOTHESIS: a fall back to unattached after bootstrapping began is a
    /// failure, but the initial unattached snapshot is not — the wait must
    /// survive subscribing before the driver flips to bootstrapping.
    #[tokio::test]
    async fn unattached_fails_only_after_bootstrapping_began() {
        let (tx, rx) = tokio::sync::watch::channel(status(Indicator::Bootstrapping));
        let waiter = tokio::spawn(await_bootstrap_outcome(rx));
        tokio::task::yield_now().await;
        tx.send(status(Indicator::Unattached))
            .expect("the waiter holds the receiver");
        let outcome = waiter.await.expect("the waiter must not panic");
        assert_eq!(
            outcome,
            BootstrapOutcome::Failed {
                report: "the bootstrap ended in mode unattached".to_string()
            }
        );
    }

    /// HYPOTHESIS: a closed status channel resolves `Failed` instead of
    /// waiting forever on a sender that will never speak again.
    #[tokio::test]
    async fn a_closed_channel_resolves_the_wait_as_failed() {
        let (tx, rx) = tokio::sync::watch::channel(status(Indicator::Bootstrapping));
        let waiter = tokio::spawn(await_bootstrap_outcome(rx));
        tokio::task::yield_now().await;
        drop(tx);
        let outcome = waiter.await.expect("the waiter must not panic");
        assert_eq!(
            outcome,
            BootstrapOutcome::Failed {
                report: "the mixnet status channel closed".to_string()
            }
        );
    }
}

#[cfg(test)]
mod offline_contract {
    //! The Offline-mode contract at the command surface (issue #2286,
    //! ADR 0006, ADR 0025). Every client here is genuinely Indexerless —
    //! built by [`LightClient::new_for_test`] from a synthetic wallet, with
    //! no server configured and no network object constructed — so these
    //! tests run with zero traffic and prove two halves of one contract:
    //!
    //! - every wallet-local command completes offline, meaning connectivity
    //!   is never the obstacle (a domain failure such as "nothing to
    //!   shield" is legitimate; the Offline refusal is not); and
    //! - every connectivity-requiring command refuses offline with the one
    //!   typed refusal, [`zingolib::lightclient::error::LightClientError::Offline`],
    //!   carried through the command's `Err` channel (ADR 0031) — never a
    //!   hang, a panic, or a silent clearnet fallback.
    //!
    //! The `change_server` pin lives at the REPL dispatch, not here: see
    //! `offline_mode_refusal` and its tests in `crate::tests`.
    //!
    //! Deliberately untested, with the reasoning on record: `drain now`,
    //! `split now`, and `migration catchup` refuse at the transmit stage,
    //! whose pre-flight `transmit` and `quicksend` pin below (each extra
    //! case would buy another proving run, not another guarantee).
    //! `network probe` refuses offline and is pinned below. `network on`
    //! is deliberately untested here: it is the consent act that switches
    //! an offline session to Online Mode (ADR 0026), and both its indexer
    //! selection and the proxy bootstrap emit real traffic. The REPL-owned
    //! `servers` command still probes the network unguarded; that gap
    //! remains open.

    #![allow(clippy::disallowed_methods)]

    use zingolib::lightclient::LightClient;
    use zingolib::testutils::synthetic_wallet::SyntheticWalletBuilder;

    use super::super::{
        CommandError, RT, dispatch_parsed, parse_command_tokens, render_error_chain,
    };

    /// The Display of `LightClientError::Offline`: the single refusal every
    /// connectivity-requiring command must surface, and the string no
    /// offline-capable command may ever emit.
    const OFFLINE_REFUSAL: &str =
        "Offline: no indexer configured. Call set_indexer_uri() to connect.";

    /// An Indexerless client over an empty synthetic wallet.
    fn offline_client() -> LightClient {
        RT.block_on(LightClient::new_for_test(
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build(),
        ))
    }

    /// An Indexerless client whose wallet holds a spendable orchard note
    /// and a transparent coin, so proposing, planning, and shielding have
    /// material to work with.
    fn funded_offline_client() -> LightClient {
        RT.block_on(LightClient::new_for_test(
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(100_000_000)
                .transparent_coin(50_000_000)
                .build(),
        ))
    }

    fn exec(
        client: &mut LightClient,
        command: &str,
        args: &[&str],
    ) -> Result<String, CommandError> {
        let tokens: Vec<String> = std::iter::once(command)
            .chain(args.iter().copied())
            .map(String::from)
            .collect();
        let parsed = parse_command_tokens(&tokens).unwrap_or_else(|error| {
            panic!("`{command}` must parse before its offline contract can be judged: {error}")
        });
        RT.block_on(dispatch_parsed(parsed, client))
    }

    /// Asserts `command` succeeds offline and returns its output.
    fn assert_works_offline(client: &mut LightClient, command: &str, args: &[&str]) -> String {
        let output = exec(client, command, args)
            .unwrap_or_else(|error| panic!("`{command}` must work offline: {error}"));
        assert!(
            !output.contains(OFFLINE_REFUSAL),
            "`{command}` must not surface the Offline refusal: {output}"
        );
        output
    }

    /// Asserts connectivity is never `command`'s obstacle: it may succeed
    /// or fail on domain grounds, but must not surface the Offline refusal.
    fn assert_unblocked_offline(client: &mut LightClient, command: &str, args: &[&str]) -> String {
        let rendered = match exec(client, command, args) {
            Ok(output) => output,
            Err(error) => render_error_chain(&error),
        };
        assert!(
            !rendered.contains(OFFLINE_REFUSAL),
            "`{command}` must not be blocked by Offline mode: {rendered}"
        );
        rendered
    }

    /// HYPOTHESIS: `assert_unblocked_offline` rejects an invocation that
    /// never reached dispatch, so a parse error cannot pass as an
    /// offline-capable command.
    #[test]
    #[should_panic(expected = "must parse")]
    fn a_parse_error_cannot_pass_as_unblocked() {
        let mut client = offline_client();
        assert_unblocked_offline(&mut client, "balance", &["surplus-argument"]);
    }

    /// Asserts `command` refuses offline through its `Err` channel with the
    /// typed Offline refusal, judged over the whole rendered chain.
    fn assert_refuses_offline_via_err(client: &mut LightClient, command: &str, args: &[&str]) {
        let error = exec(client, command, args).expect_err(command);
        let rendered = render_error_chain(&error);
        assert!(
            rendered.contains(OFFLINE_REFUSAL),
            "`{command}` must refuse with the typed Offline error: {rendered}"
        );
    }

    /// A build without the nym feature has no `network` command at all,
    /// so the grammar itself refuses before any body could run.
    #[cfg(not(feature = "nym"))]
    #[test]
    fn network_is_unknown_to_the_opt_out_build() {
        let tokens: Vec<String> = ["network", "status"].map(String::from).into();
        let error = parse_command_tokens(&tokens).expect_err("network status must refuse");
        assert!(error.contains("unrecognized subcommand"), "{error}");
    }

    /// A fresh unified address from the wallet itself, so send-family tests
    /// stay on the wallet's own chain.
    fn own_unified_address(client: &mut LightClient) -> String {
        let output = assert_works_offline(client, "new_address", &["o"]);
        let parsed = json::parse(&output).expect("new_address returns JSON");
        parsed["encoded_address"]
            .as_str()
            .expect("encoded_address is a string")
            .to_string()
    }

    mod works_offline {
        //! The offline-capable surface: every command here must complete
        //! against an Indexerless client.

        use super::*;

        #[test]
        fn version() {
            let output = assert_works_offline(&mut offline_client(), "version", &[]);
            assert!(!output.is_empty());
        }

        #[test]
        fn help() {
            let output = assert_works_offline(&mut offline_client(), "help", &[]);
            assert!(output.contains("Wallet commands"), "{output}");
        }

        #[test]
        fn parse_address_of_the_wallets_own() {
            let mut client = offline_client();
            let address = own_unified_address(&mut client);
            let output = assert_works_offline(&mut client, "parse_address", &[&address]);
            assert!(output.contains("success"), "{output}");
        }

        #[test]
        fn parse_viewkey_of_the_exported_ufvk() {
            let mut client = offline_client();
            let exported = assert_works_offline(&mut client, "export_ufvk", &[]);
            let ufvk = json::parse(&exported).expect("export_ufvk returns JSON")["ufvk"]
                .as_str()
                .expect("ufvk is a string")
                .to_string();
            let output = assert_works_offline(&mut client, "parse_viewkey", &[&ufvk]);
            assert!(output.contains("success"), "{output}");
        }

        #[test]
        fn addresses() {
            let output = assert_works_offline(&mut offline_client(), "addresses", &[]);
            json::parse(&output).expect("addresses returns JSON");
        }

        #[test]
        fn t_addresses() {
            let output = assert_works_offline(&mut offline_client(), "t_addresses", &[]);
            json::parse(&output).expect("t_addresses returns JSON");
        }

        #[test]
        fn new_address() {
            let output = assert_works_offline(&mut offline_client(), "new_address", &["oz"]);
            assert!(output.contains("encoded_address"), "{output}");
        }

        /// Funded, because the address-gap rule (a new transparent address
        /// only after the latest one received funds) is a domain rule that
        /// applies offline exactly as it does online.
        #[test]
        fn new_taddress() {
            let output = assert_works_offline(&mut funded_offline_client(), "new_taddress", &[]);
            assert!(output.contains("encoded_address"), "{output}");
        }

        #[test]
        fn balance() {
            let output = assert_works_offline(&mut funded_offline_client(), "balance", &[]);
            assert!(!output.is_empty());
        }

        #[test]
        fn spendable_balance() {
            let output =
                assert_works_offline(&mut funded_offline_client(), "spendable_balance", &[]);
            assert!(output.contains("spendable_balance"), "{output}");
        }

        #[test]
        fn max_send_value() {
            let mut client = funded_offline_client();
            let address = own_unified_address(&mut client);
            let output = assert_works_offline(&mut client, "max_send_value", &[&address]);
            assert!(output.contains("max_send_value"), "{output}");
        }

        #[test]
        fn birthday() {
            let output = assert_works_offline(&mut offline_client(), "birthday", &[]);
            assert!(!output.is_empty());
        }

        #[test]
        fn height_reports_the_wallets_own_view() {
            let output = assert_works_offline(&mut offline_client(), "height", &[]);
            assert!(output.contains("20"), "the synthetic tip is 20: {output}");
        }

        #[test]
        fn notes() {
            assert_works_offline(&mut funded_offline_client(), "notes", &[]);
        }

        #[test]
        fn coins() {
            assert_works_offline(&mut funded_offline_client(), "coins", &[]);
        }

        #[test]
        fn transactions() {
            assert_works_offline(&mut offline_client(), "transactions", &[]);
        }

        #[test]
        fn value_transfers() {
            assert_works_offline(&mut offline_client(), "value_transfers", &[]);
        }

        #[test]
        fn messages() {
            assert_works_offline(&mut offline_client(), "messages", &[]);
        }

        #[test]
        fn sends_to_address() {
            assert_works_offline(&mut offline_client(), "sends_to_address", &[]);
        }

        #[test]
        fn value_to_address() {
            assert_works_offline(&mut offline_client(), "value_to_address", &[]);
        }

        #[test]
        fn memobytes_to_address() {
            assert_works_offline(&mut offline_client(), "memobytes_to_address", &[]);
        }

        #[test]
        fn wallet_kind() {
            let output = assert_works_offline(&mut offline_client(), "wallet_kind", &[]);
            assert!(output.contains("mnemonic"), "{output}");
        }

        #[test]
        fn settings() {
            assert_unblocked_offline(&mut offline_client(), "settings", &[]);
        }

        #[test]
        fn recovery_info() {
            let output = assert_works_offline(&mut offline_client(), "recovery_info", &[]);
            assert!(!output.is_empty());
        }

        #[test]
        fn export_ufvk() {
            let output = assert_works_offline(&mut offline_client(), "export_ufvk", &[]);
            assert!(output.contains("ufvk"), "{output}");
        }

        #[test]
        fn check_address_recognizes_the_wallets_own() {
            let mut client = offline_client();
            let address = own_unified_address(&mut client);
            let output = assert_works_offline(&mut client, "check_address", &[&address]);
            assert!(output.contains("is_wallet_address"), "{output}");
        }

        #[test]
        fn clear() {
            let output = assert_works_offline(&mut offline_client(), "clear", &[]);
            assert!(output.contains("success"), "{output}");
        }

        /// `sync status` reads wallet state only; on a synthetic wallet it
        /// reports a wallet-shape error (no wallet blocks are fabricated),
        /// which is a domain outcome — connectivity is never the obstacle.
        #[test]
        fn sync_status() {
            assert_unblocked_offline(&mut offline_client(), "sync", &["status"]);
        }

        #[test]
        fn sync_poll() {
            let output = assert_works_offline(&mut offline_client(), "sync", &["poll"]);
            assert_eq!(output, "Sync task has not been launched.");
        }

        #[test]
        fn quit() {
            let output = assert_works_offline(&mut offline_client(), "quit", &[]);
            assert!(output.contains("quit successfully"), "{output}");
        }

        /// Proposing is an Indexerless capability (ADR 0006): `send` shows
        /// the fee offline, for a later `calculate`/`transmit`.
        #[test]
        fn send_proposes_offline() {
            let mut client = funded_offline_client();
            let address = own_unified_address(&mut client);
            let output = assert_works_offline(&mut client, "send", &[&address, "50000"]);
            assert!(output.contains("fee"), "{output}");
        }

        #[test]
        fn send_all_proposes_offline() {
            let mut client = funded_offline_client();
            let address = own_unified_address(&mut client);
            let output = assert_works_offline(&mut client, "send_all", &[&address]);
            assert!(output.contains("fee"), "{output}");
        }

        #[test]
        fn shield_proposes_offline() {
            let output = assert_works_offline(&mut funded_offline_client(), "shield", &[]);
            assert!(output.contains("fee"), "{output}");
        }

        /// Offline signing (ADR 0006): `calculate` signs the stored
        /// proposal with no Indexer, leaving Calculated transactions for a
        /// connected `transmit`.
        #[test]
        fn calculate_signs_offline() {
            let mut client = funded_offline_client();
            let address = own_unified_address(&mut client);
            assert_works_offline(&mut client, "send", &[&address, "50000"]);
            let output = assert_works_offline(&mut client, "calculate", &[]);
            assert!(output.contains("txids"), "{output}");
        }

        #[test]
        fn drain_plan() {
            let output = assert_works_offline(&mut funded_offline_client(), "drain", &["plan"]);
            assert!(output.contains("transactions"), "{output}");
        }

        #[test]
        fn split_plan() {
            let output = assert_works_offline(&mut funded_offline_client(), "split", &["plan"]);
            assert!(output.contains("split_rounds"), "{output}");
        }

        #[test]
        fn migration_plan() {
            let output = assert_works_offline(&mut funded_offline_client(), "migration", &["plan"]);
            assert!(output.contains("plan_hash"), "{output}");
        }

        #[test]
        fn migration_status() {
            let output =
                assert_works_offline(&mut funded_offline_client(), "migration", &["status"]);
            assert!(output.contains("phase"), "{output}");
        }

        #[test]
        fn migration_windows() {
            assert_unblocked_offline(&mut funded_offline_client(), "migration", &["windows"]);
        }

        /// `network status` reads the wallet's mode: an offline session never
        /// bootstraps the mixnet, so a fresh client reports unattached.
        #[cfg(feature = "nym")]
        #[test]
        fn network_status_reports_unattached() {
            let output = assert_works_offline(&mut offline_client(), "network", &["status"]);
            assert!(output.contains("unattached"), "{output}");
        }

        /// `network probe` runs only over the mixnet route: a session whose
        /// mixnet is unattached refuses with the mixnet refusal, never by
        /// falling back to a clearnet probe.
        #[cfg(feature = "nym")]
        #[test]
        fn network_probe_refuses_without_the_mixnet() {
            let error = exec(&mut offline_client(), "network", &["probe"])
                .expect_err("probe must refuse without the mixnet");
            assert!(
                error.to_string().contains("the Nym mixnet is not enabled"),
                "the refusal names the mixnet state: {error}"
            );
        }

        #[test]
        fn remove_transaction_fails_on_the_txid_never_on_connectivity() {
            let unknown_txid = "ab".repeat(32);
            assert_unblocked_offline(
                &mut offline_client(),
                "remove_transaction",
                &[&unknown_txid],
            );
        }

        #[test]
        fn delete_fails_on_the_file_never_on_connectivity() {
            assert_unblocked_offline(&mut offline_client(), "delete", &[]);
        }
    }

    mod refuses_offline {
        //! The connectivity-requiring surface: every command here must
        //! refuse offline with the typed Offline error, through its `Err`
        //! channel.

        use super::*;

        #[test]
        fn sync_run() {
            assert_refuses_offline_via_err(&mut offline_client(), "sync", &["run"]);
        }

        #[test]
        fn rescan() {
            assert_refuses_offline_via_err(&mut offline_client(), "rescan", &[]);
        }

        /// `info` carries the typed failure whole: the error's rendering IS
        /// the refusal, byte for byte.
        #[test]
        fn info() {
            let error = exec(&mut offline_client(), "info", &[]).expect_err("info refuses offline");
            assert_eq!(error.to_string(), OFFLINE_REFUSAL);
        }

        /// `confirm` pre-flights the Indexer before touching the stored
        /// proposal, so the refusal needs no proposal and costs no proving.
        #[test]
        fn confirm() {
            assert_refuses_offline_via_err(&mut offline_client(), "confirm", &[]);
        }

        /// `transmit` pre-flights the Indexer before resolving txids, so
        /// even an unknown txid refuses on connectivity first.
        #[test]
        fn transmit() {
            let txid = "ab".repeat(32);
            assert_refuses_offline_via_err(&mut offline_client(), "transmit", &[&txid]);
        }

        /// `quicksend` proposes and signs offline (both Indexerless
        /// capabilities), then refuses at the transmit stage: the wallet
        /// does the work it can and leaks nothing.
        #[test]
        fn quicksend() {
            let mut client = funded_offline_client();
            let address = own_unified_address(&mut client);
            assert_refuses_offline_via_err(&mut client, "quicksend", &[&address, "50000"]);
        }

        /// `quickshield` mirrors `quicksend`: the shield proposal and its
        /// signing succeed offline, and the transmit stage refuses.
        #[test]
        fn quickshield() {
            assert_refuses_offline_via_err(&mut funded_offline_client(), "quickshield", &[]);
        }

        /// `migrate` syncs before building anything, so the refusal
        /// arrives from the sync pre-flight with no proving spent.
        #[test]
        fn migrate() {
            assert_refuses_offline_via_err(&mut funded_offline_client(), "migrate", &[]);
        }

        #[test]
        fn migration_continue() {
            assert_refuses_offline_via_err(
                &mut funded_offline_client(),
                "migration",
                &["continue"],
            );
        }

        #[test]
        fn migration_execute() {
            assert_refuses_offline_via_err(&mut funded_offline_client(), "migration", &["execute"]);
        }

        #[test]
        fn migration_auto() {
            assert_refuses_offline_via_err(&mut funded_offline_client(), "migration", &["auto"]);
        }

        /// `current_price` is mixnet-only (ADR 0011): an offline session
        /// never bootstraps the mixnet, so the fetch refuses from the
        /// unattached mode — a typed refusal, zero traffic.
        #[cfg(feature = "nym")]
        #[test]
        fn current_price_refuses_from_the_unattached_mixnet() {
            let error = exec(&mut offline_client(), "current_price", &[])
                .expect_err("current_price refuses offline");
            assert!(
                error.to_string().contains("the Nym mixnet is not enabled"),
                "{error}"
            );
        }

        /// Without the nym feature there is no price fetch at all: the
        /// command says so instead of touching the network.
        #[cfg(not(feature = "nym"))]
        #[test]
        fn current_price_has_no_fetch_to_offer() {
            let output = exec(&mut offline_client(), "current_price", &[])
                .expect("current_price explains the absent fetch");
            assert!(output.contains("no price fetch"), "{output}");
        }
    }
}

#[cfg(test)]
mod pure_helpers {
    //! Runtime-free checks of the pure rendering vocabulary: every function
    //! here takes already-fetched values and returns its whole result.

    use pepper_sync::error::SyncModeError;
    use zingolib::lightclient::error::{LightClientError, SendError};
    use zingolib::wallet::keys::WalletAddressRef;

    use super::super::{
        JSON_INDENT, MigrationCommandError, address_check_json, not_yet_typed, render_error_chain,
        txids_json,
    };

    /// HYPOTHESIS: the wrapper stores the rendering verbatim, without an
    /// "Error: " prefix, so the edge renderer adds it exactly once.
    #[test]
    fn not_yet_typed_renders_the_message_unprefixed() {
        assert_eq!(
            not_yet_typed(std::io::Error::other("no such wallet file")).to_string(),
            "no such wallet file"
        );
    }

    /// HYPOTHESIS: the wrapper carries the failure itself rather than its
    /// outermost line, so the dispatch renderer walks the whole source
    /// chain; a rendering that drops the innermost detail falsifies it.
    #[test]
    fn not_yet_typed_keeps_the_source_chain_renderable() {
        let wrapped = not_yet_typed(LightClientError::SendError(SendError::NoStoredProposal));
        assert_eq!(
            render_error_chain(&wrapped),
            "Send error.\ncaused by: No proposal found in the wallet."
        );
    }

    /// HYPOTHESIS: the dispatch seam renders a two-link cause chain exactly
    /// as the one sanctioned chain walk joined by the seam's separator does,
    /// so the seam keeps no private copy of the walk. Falsified if the two
    /// renderings differ by a single byte.
    #[test]
    fn the_dispatch_rendering_matches_the_sanctioned_walk() {
        let wrapped = not_yet_typed(LightClientError::SendError(SendError::NoStoredProposal));

        assert_eq!(
            render_error_chain(&wrapped),
            zingo_net_diag::chain_texts(&wrapped).join("\ncaused by: ")
        );
    }

    /// HYPOTHESIS: a migration sync failure keeps the LightClient failure
    /// as its source, so the chain walk reaches the innermost detail; a
    /// rendering that stops at the wrapper line falsifies it.
    #[test]
    fn migration_sync_failure_keeps_its_source_chain() {
        let refused = MigrationCommandError::Sync(LightClientError::SyncModeError(
            SyncModeError::SyncAlreadyRunning,
        ));
        assert_eq!(
            render_error_chain(&refused),
            "sync failed\ncaused by: Sync mode error.\ncaused by: sync is already running"
        );
    }

    /// HYPOTHESIS: the ids render as a flat JSON array of their string
    /// forms, in the order given.
    #[test]
    fn txids_json_renders_a_flat_ordered_array() {
        assert_eq!(
            txids_json(&["first", "second"]).dump(),
            r#"["first","second"]"#
        );
    }

    /// HYPOTHESIS: an underived address renders as the single
    /// is_wallet_address=false field, with no address fields to mislead.
    #[test]
    fn address_check_json_renders_the_underived_case_bare() {
        assert_eq!(
            address_check_json(None).dump(),
            r#"{"is_wallet_address":"false"}"#
        );
    }

    /// HYPOTHESIS: a derived unified address renders its type, index, and
    /// receiver flags alongside the encoding.
    #[test]
    fn address_check_json_renders_a_unified_derivation() {
        let rendered = address_check_json(Some(WalletAddressRef::Unified {
            account_id: zip32::AccountId::ZERO,
            address_index: Some(3),
            has_orchard: true,
            has_sapling: false,
            has_transparent: true,
            encoded_address: "u1mocked".to_string(),
        }))
        .pretty(JSON_INDENT);
        for expected in [
            r#""is_wallet_address": "true""#,
            r#""address_type": "unified""#,
            r#""address_index": 3"#,
            r#""account_id": 0"#,
            r#""has_orchard": true"#,
            r#""has_sapling": false"#,
            r#""has_transparent": true"#,
            r#""encoded_address": "u1mocked""#,
        ] {
            assert!(rendered.contains(expected), "{rendered}");
        }
    }
}

#[cfg(test)]
mod finding_pins {
    //! Pins the contracts the review findings demanded, kept green by the
    //! fixes that closed them.
    #![allow(clippy::disallowed_methods)]

    use zingolib::lightclient::LightClient;
    use zingolib::testutils::synthetic_wallet::SyntheticWalletBuilder;

    use super::super::{CliCommand, RT, format_help, parse_command_tokens, wallet_free_commands};

    fn tokens(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| String::from(*word)).collect()
    }

    /// HYPOTHESIS: the standalone help section derives from
    /// `requires_wallet` alone: its rendered entries are exactly the
    /// derived wallet-free names, so no second statement of the set
    /// exists to drift.
    #[test]
    fn the_standalone_section_derives_from_requires_wallet() {
        let listing = format_help(crate::Communications::Online, None);
        let wallet_header = listing
            .find("Wallet commands:")
            .expect("the listing carries a wallet section");
        let rendered: Vec<String> = listing[..wallet_header]
            .lines()
            .filter_map(|line| line.strip_prefix("  "))
            .filter_map(|entry| entry.split(" - ").next())
            .map(String::from)
            .collect();
        let derived: Vec<String> = wallet_free_commands()
            .iter()
            .map(CliCommand::name)
            .collect();
        assert_eq!(rendered, derived);
    }

    const FAMILIES: &[&str] = &[
        "save",
        "settings",
        "sync",
        #[cfg(feature = "nym")]
        "network",
        "migration",
        "drain",
        "split",
    ];

    /// HYPOTHESIS: no family's long help advertises a nested `help` that
    /// the grammar refuses.
    #[test]
    fn family_long_help_never_advertises_nested_help() {
        for &family in FAMILIES {
            let help = format_help(crate::Communications::Online, Some(family));
            assert!(
                !help
                    .lines()
                    .any(|line| line.split_whitespace().next() == Some("help")),
                "`{family}` long help advertises a nested help:\n{help}"
            );
            assert!(
                parse_command_tokens(&tokens(&[family, "help"])).is_err(),
                "`{family} help` must stay refused while unadvertised"
            );
        }
    }

    /// HYPOTHESIS: a REPL refusal speaks in the prompt's terms, never
    /// naming the binary or its process-oriented usage.
    #[test]
    fn repl_refusals_never_name_the_binary() {
        for line in [
            &["no_such_command"][..],
            &["balance", "--bogus"][..],
            &["save"][..],
        ] {
            let error = parse_command_tokens(&tokens(line)).expect_err("must refuse");
            assert!(!error.contains("zingo-cli"), "{line:?} refused as: {error}");
        }
    }

    /// HYPOTHESIS: every family sub-command carries an about line, so
    /// `help <family>` lists no bare names.
    #[test]
    fn family_sub_commands_all_carry_abouts() {
        for &family in FAMILIES {
            let help = format_help(crate::Communications::Online, Some(family));
            let listing = help
                .split("Commands:")
                .nth(1)
                .unwrap_or_else(|| panic!("`{family}` long help lists its sub-commands"));
            for entry in listing
                .lines()
                .skip(1)
                .take_while(|line| !line.trim().is_empty())
            {
                assert!(
                    entry.split_whitespace().count() > 1,
                    "`{family}` lists a bare sub-command: {entry}"
                );
            }
        }
    }

    #[allow(dead_code)]
    fn offline_client() -> LightClient {
        RT.block_on(LightClient::new_for_test(
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build(),
        ))
    }

    /// HYPOTHESIS: a malformed one-shot send fails at the parse, so no
    /// wallet work can begin on a mistyped amount.
    #[test]
    fn a_non_numeric_send_amount_refuses_at_the_parse() {
        assert!(
            parse_command_tokens(&tokens(&["send", "zs1exampleaddress", "one-hundred"])).is_err(),
            "a non-numeric amount must be a parse error, not a body error"
        );
    }

    /// HYPOTHESIS: a session flag written after `messages` is a usage
    /// error, as the CHANGELOG's flag-ordering rule promises.
    #[test]
    fn a_session_flag_after_messages_refuses_at_the_parse() {
        assert!(
            parse_command_tokens(&tokens(&["messages", "--nosync"])).is_err(),
            "a post-command session flag must be refused, never read as a memo filter"
        );
    }

    /// HYPOTHESIS: a command's Debug rendering, the string the name
    /// derivation materializes on the heap, never carries memos or key
    /// material out of the arguments.
    #[test]
    fn debug_rendering_never_materializes_memos_or_keys() {
        let memo = "SENTINEL-MEMO-must-not-materialize";
        let key = "SENTINEL-UFVK-must-not-materialize";
        let send = CliCommand::Send {
            args: vec![
                String::from("zs1exampleaddress"),
                String::from("50000"),
                String::from(memo),
            ],
        };
        let viewkey = CliCommand::ParseViewkey {
            viewkey: String::from(key),
        };
        for (command, sentinel) in [(send, memo), (viewkey, key)] {
            let rendered = format!("{command:?}");
            assert!(
                !rendered.contains(sentinel),
                "`{}` renders its secret argument onto the heap: {rendered}",
                command.name()
            );
        }
    }

    /// HYPOTHESIS: in a build without the nym feature the whole `network`
    /// family sits outside the grammar, so an invocation with arguments
    /// meets the unknown-command refusal and nothing ever grades them.
    #[cfg(not(feature = "nym"))]
    #[test]
    fn network_arguments_meet_the_unknown_command_refusal() {
        use super::super::{CommandError, dispatch_parsed};
        let rendered = match parse_command_tokens(&tokens(&["network", "probe", "http://x.com"]))
            .map_err(|error| CommandError::NotYetTyped(error.into()))
            .and_then(|parsed| RT.block_on(dispatch_parsed(parsed, &mut offline_client())))
        {
            Ok(output) => output,
            Err(error) => error.to_string(),
        };
        assert!(rendered.contains("unrecognized subcommand"), "{rendered}");
    }
}

#[cfg(test)]
mod posture_surface {
    //! ADR 0032's rendered surface: `help` offers only what the live
    //! posture leaves unsuppressed, and `network off` is a zero-emission
    //! teardown, never a clearnet fallback.
    #![allow(clippy::disallowed_methods)]

    use crate::Communications;

    use super::super::format_help;

    /// HYPOTHESIS: a deliberate `--offline` help hides the whole
    /// network-requiring surface, the network family included, while the
    /// Indexerless surface stays listed.
    #[test]
    fn a_deliberate_offline_help_hides_the_network_requiring_surface() {
        let listing = format_help(Communications::DeliberateOffline, None);
        for hidden in [
            "  confirm - ",
            "  transmit - ",
            "  rescan - ",
            "  network - ",
        ] {
            assert!(
                !listing.contains(hidden),
                "{hidden:?} must be hidden from a deliberate offline help:\n{listing}"
            );
        }
        for offered in ["  balance - ", "  send - ", "  migration - ", "  height - "] {
            assert!(
                listing.contains(offered),
                "{offered:?} must stay offered:\n{listing}"
            );
        }
    }

    /// HYPOTHESIS: an unconsented session's help keeps the network family,
    /// because `network on` is its consent act, while the rest of the
    /// network-requiring surface stays hidden.
    #[cfg(feature = "nym")]
    #[test]
    fn an_unconsented_help_keeps_the_network_family() {
        let listing = format_help(Communications::UnconsentedOffline, None);
        assert!(listing.contains("  network - "), "{listing}");
        assert!(!listing.contains("  confirm - "), "{listing}");
    }

    /// HYPOTHESIS: a suppressed command's long help reads as not found, so
    /// the command has disappeared rather than gone forbidden-but-visible.
    #[test]
    fn a_suppressed_commands_long_help_is_not_found() {
        assert_eq!(
            format_help(Communications::DeliberateOffline, Some("confirm")),
            "Command confirm not found"
        );
        assert!(
            format_help(Communications::Online, Some("confirm")).contains("Usage:"),
            "online help must still render the long help"
        );
    }

    /// HYPOTHESIS: `network off` reports the minted teardown, leaves the
    /// client Indexerless, and never mentions a clearnet fallback.
    #[cfg(feature = "nym")]
    #[test]
    fn network_off_tears_down_and_keeps_the_stored_consent() {
        use zingolib::lightclient::LightClient;
        use zingolib::testutils::synthetic_wallet::SyntheticWalletBuilder;

        use super::super::{CommandError, RT, dispatch_parsed, parse_command_tokens};

        let mut client = RT.block_on(LightClient::new_for_test(
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build(),
        ));
        let tokens: Vec<String> = ["network", "off"].map(String::from).into();
        let report = parse_command_tokens(&tokens)
            .map_err(|error| CommandError::NotYetTyped(error.into()))
            .and_then(|parsed| RT.block_on(dispatch_parsed(parsed, &mut client)))
            .expect("network off succeeds offline");
        assert!(report.contains("Network off"), "{report}");
        assert!(report.contains("`--forget-online` erases it"), "{report}");
        assert!(!report.contains("clearnet"), "{report}");
        assert!(client.indexer_uri().is_none());
    }
}

#[cfg(all(test, feature = "nym"))]
mod attached_exit_reporting {
    use zingolib::lightclient::LightClient;
    use zingolib::testutils::synthetic_wallet::SyntheticWalletBuilder;

    use super::super::{BootstrapOutcome, await_bootstrap_outcome};

    /// HYPOTHESIS: an attached endpoint that accepts TCP but carries no data
    /// fails closed — the readiness gate is a round trip through the tunnel,
    /// not a loopback dial, so a dead mixnet path never reports `Ready`.
    /// Falsified if a listener that answers no gRPC reaches `Ready` (the
    /// #2662 headline finding ran this red while the gate was a bare dial).
    // Seam justification (ADR 0030): the block_on is the tokio::test
    // harness's own crossing, not a new seam in the CLI.
    #[tokio::test]
    #[allow(clippy::disallowed_methods)]
    async fn an_attached_endpoint_that_carries_no_data_fails_closed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback listener binds");
        let addr = listener
            .local_addr()
            .expect("the bound listener has an address")
            .to_string();
        // A stand-in host that accepts the connection and answers nothing.
        let host = tokio::spawn(async move {
            loop {
                drop(listener.accept().await);
            }
        });

        let mut client = LightClient::new_for_test(
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build(),
        )
        .await;
        let receiver = client.subscribe_mixnet_status();
        client
            .attach_mixnet(
                &addr,
                &[zingolib::mixnet::ExitNodeId::parse("host-bound-exit")
                    .expect("the test identity parses")],
            )
            .await
            .expect("a valid loopback address attaches");

        let outcome = await_bootstrap_outcome(receiver).await;
        host.abort();
        match outcome {
            BootstrapOutcome::Failed { .. } => {}
            BootstrapOutcome::Ready { exits } => {
                panic!("a data-dead endpoint must never reach Ready; got exits {exits:?}")
            }
        }
    }
}
