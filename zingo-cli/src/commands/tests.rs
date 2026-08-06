#[cfg(test)]
mod table_invariants {
    //! Pins the properties [`super::super::COMMANDS`] relies on but the
    //! compiler does not check: declaration order is the help listing's
    //! order, and dispatch takes the first name match.

    use super::super::COMMANDS;

    /// HYPOTHESIS: the table's names are strictly increasing — sorted, so
    /// the help listing stays alphabetical, and therefore unique, so no
    /// row can shadow another at dispatch.
    #[test]
    fn names_are_strictly_increasing() {
        for pair in COMMANDS.windows(2) {
            assert!(
                pair[0].name < pair[1].name,
                "COMMANDS must stay sorted and duplicate-free: {:?} precedes {:?}",
                pair[0].name,
                pair[1].name
            );
        }
    }

    /// HYPOTHESIS: wherever a help text offers a Usage section, at least
    /// one of its lines invokes the command by its minted name, so the
    /// prose cannot silently drift from the table's single mint. The
    /// duplication itself retires when clap generates usage from typed
    /// arguments (the ratified follow-up arc); until then it is pinned.
    #[test]
    fn usage_lines_invoke_the_minted_name() {
        for spec in COMMANDS {
            let Some((_, usage)) = spec.help.split_once("Usage:") else {
                continue;
            };
            assert!(
                usage.lines().map(str::trim).any(|line| {
                    line == spec.name || line.starts_with(&format!("{} ", spec.name))
                }),
                "`{}`'s Usage section never invokes it by its minted name:\n{}",
                spec.name,
                spec.help
            );
        }
    }
}

#[cfg(test)]
mod transmit_heartbeat {
    //! Paused-clock falsifiers for the transmit heartbeat's contract: silence
    //! for fast transmissions, a narrated line on the ratified 20-40s cadence
    //! for slow ones, always carrying the side channel's latest detail.
    //!
    //! Seam justification (ADR 0030): the `block_on` here is the one
    //! `#[tokio::test]` generates to drive each async test body; a test
    //! driver is a sync frontend, so it is an audited crossing.
    #![allow(clippy::disallowed_methods)]

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::super::*;

    /// HYPOTHESIS: a transmission finishing before the first tick emits
    /// nothing, because the heartbeat must not add noise to a normal fast send.
    #[tokio::test(start_paused = true)]
    async fn a_fast_transmission_stays_silent() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = lines.clone();
        let out = with_transmit_heartbeat(
            "confirm",
            || Some("submitting".to_string()),
            move |line| sink.lock().expect("line sink poisoned").push(line),
            tokio::time::sleep(zingo_netutils::time::test::SIMULATED_TRANSMIT),
        )
        .await;
        let () = out;
        assert!(
            lines.lock().expect("line sink poisoned").is_empty(),
            "no heartbeat before the first interval"
        );
    }

    /// HYPOTHESIS: a slow transmission is narrated on the interval cadence,
    /// each line carrying the label, the side channel's latest detail, and
    /// the elapsed seconds. Falsified if the wait stays silent or drops the
    /// detail.
    #[tokio::test(start_paused = true)]
    async fn a_slow_transmission_heartbeats_the_latest_detail() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = lines.clone();
        with_transmit_heartbeat(
            "confirm",
            || Some("witness zec.rocks: submitting".to_string()),
            move |line| sink.lock().expect("line sink poisoned").push(line),
            tokio::time::sleep(Duration::from_secs(95)),
        )
        .await;
        let lines = lines.lock().expect("line sink poisoned").clone();
        assert_eq!(
            lines,
            vec![
                "confirm: witness zec.rocks: submitting (30s elapsed)".to_string(),
                "confirm: witness zec.rocks: submitting (60s elapsed)".to_string(),
                "confirm: witness zec.rocks: submitting (90s elapsed)".to_string(),
            ]
        );
    }

    /// An empty side channel still heartbeats, falling back to a generic
    /// line rather than skipping the tick.
    #[tokio::test(start_paused = true)]
    async fn an_empty_side_channel_still_heartbeats() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = lines.clone();
        with_transmit_heartbeat(
            "transmit",
            || None,
            move |line| sink.lock().expect("line sink poisoned").push(line),
            tokio::time::sleep(Duration::from_secs(35)),
        )
        .await;
        assert_eq!(
            lines.lock().expect("line sink poisoned").clone(),
            vec!["transmit: transmitting (30s elapsed)".to_string()]
        );
    }
}

#[cfg(test)]
mod migration_command_parsing {
    //! Pins the pure argument parser and the byte-identity of the typed
    //! errors' rendering with the in-band strings they replaced.

    use super::super::*;

    #[test]
    fn start_parses_hash_and_per_bucket() {
        let hash_hex = "11".repeat(32);
        let parsed = parse_migration_args(&["start", &hash_hex, "--per-bucket", "3"])
            .expect("well-formed arguments parse");
        assert_eq!(
            parsed,
            MigrationSubCommand::Start {
                plan_hash: [0x11; 32],
                per_bucket: Some(3),
            }
        );
    }

    #[test]
    fn malformed_plan_hash_is_typed() {
        assert!(matches!(
            parse_migration_args(&["start", "abc"]),
            Err(MigrationCommandError::MalformedPlanHash)
        ));
    }

    #[test]
    fn continue_parses_bare() {
        assert_eq!(
            parse_migration_args(&["continue"]).expect("bare continue parses"),
            MigrationSubCommand::Continue
        );
    }

    #[test]
    fn windows_parses_bare() {
        assert_eq!(
            parse_migration_args(&["windows"]).expect("bare windows parses"),
            MigrationSubCommand::Windows
        );
    }

    #[test]
    fn cadence_requires_a_count() {
        assert_eq!(
            parse_migration_args(&["cadence", "4"]).expect("well-formed cadence parses"),
            MigrationSubCommand::Cadence { per_bucket: 4 }
        );
        assert!(matches!(
            parse_migration_args(&["cadence"]),
            Err(MigrationCommandError::MalformedCadence)
        ));
    }

    #[test]
    fn execute_defaults_spacing_to_thirty_seconds() {
        assert_eq!(
            parse_migration_args(&["execute"]).expect("bare execute parses"),
            MigrationSubCommand::Execute {
                spacing: std::time::Duration::from_secs(30),
            }
        );
        assert_eq!(
            parse_migration_args(&["execute", "5"]).expect("spaced execute parses"),
            MigrationSubCommand::Execute {
                spacing: std::time::Duration::from_secs(5),
            }
        );
    }

    #[test]
    fn catchup_defaults_spacing_to_thirty_seconds() {
        assert_eq!(
            parse_migration_args(&["catchup"]).expect("bare catchup parses"),
            MigrationSubCommand::Catchup {
                spacing: std::time::Duration::from_secs(30),
            }
        );
    }

    #[test]
    fn errors_render_byte_identically_to_the_replaced_strings() {
        assert_eq!(
            format!("Error: {}", MigrationCommandError::MissingSubCommand),
            "Error: migration command expects a sub-command. Type \"help migration\" for usage."
        );
        assert_eq!(
            format!("Error: {}", MigrationCommandError::MalformedPerBucket),
            "Error: --per-bucket expects a positive integer."
        );
    }
}

#[cfg(test)]
mod drain_and_split_command_parsing {
    //! Pins the pure argument parsers of the mobile-parity migration
    //! commands: `drain` and `split` accept exactly `plan` or `now`.

    use super::super::*;

    #[test]
    fn drain_accepts_exactly_plan_or_now() {
        assert_eq!(
            parse_drain_args(&["plan"]).expect("drain plan parses"),
            DrainSubCommand::Plan
        );
        assert_eq!(
            parse_drain_args(&["now"]).expect("drain now parses"),
            DrainSubCommand::Now
        );
        for junk in [&[][..], &["bogus"][..], &["now", "extra"][..]] {
            assert!(matches!(
                parse_drain_args(junk),
                Err(MigrationCommandError::DrainUsage)
            ));
        }
    }

    #[test]
    fn split_accepts_exactly_plan_or_now() {
        assert_eq!(
            parse_split_args(&["plan"]).expect("split plan parses"),
            SplitSubCommand::Plan
        );
        assert_eq!(
            parse_split_args(&["now"]).expect("split now parses"),
            SplitSubCommand::Now
        );
        for junk in [&[][..], &["bogus"][..], &["plan", "extra"][..]] {
            assert!(matches!(
                parse_split_args(junk),
                Err(MigrationCommandError::SplitUsage)
            ));
        }
    }

    #[test]
    fn usage_errors_render_with_help_pointers() {
        assert_eq!(
            format!("Error: {}", MigrationCommandError::DrainUsage),
            "Error: drain expects a sub-command: plan | now. Type \"help drain\" for usage."
        );
        assert_eq!(
            format!("Error: {}", MigrationCommandError::SplitUsage),
            "Error: split expects a sub-command: plan | now. Type \"help split\" for usage."
        );
    }
}

#[cfg(test)]
mod nym_command_parsing {
    //! Pins the pure argument parser and the byte-identity of the typed
    //! errors' rendering with the in-band strings they replaced.

    use super::super::*;

    #[cfg(feature = "nym")]
    #[test]
    fn bare_and_status_both_parse_to_status() {
        assert_eq!(
            parse_nym_args(&[]).expect("a bare nym parses"),
            NymSubCommand::Status
        );
        assert_eq!(
            parse_nym_args(&["status"]).expect("nym status parses"),
            NymSubCommand::Status
        );
    }

    #[cfg(feature = "nym")]
    #[test]
    fn on_captures_the_optional_path() {
        assert_eq!(
            parse_nym_args(&["on"]).expect("bare nym on parses"),
            NymSubCommand::On { path: None }
        );
        assert_eq!(
            parse_nym_args(&["on", "/opt/nym-proxy"]).expect("nym on with a path parses"),
            NymSubCommand::On {
                path: Some("/opt/nym-proxy".to_string()),
            }
        );
    }

    #[cfg(feature = "nym")]
    #[test]
    fn unknown_subcommand_renders_byte_identically_to_the_replaced_string() {
        assert_eq!(
            parse_nym_args(&["bogus"])
                .expect_err("an unknown subcommand is typed")
                .to_string(),
            "unknown nym subcommand 'bogus'. Use: nym status | nym on [path] | nym off | \
             nym probe [uri] | nym history"
        );
    }

    #[cfg(feature = "nym")]
    #[test]
    fn probe_parses_its_optional_target_and_rejects_junk() {
        assert_eq!(
            parse_nym_args(&["probe"]).expect("bare probe parses"),
            NymSubCommand::Probe { target: None }
        );
        assert_eq!(
            parse_nym_args(&["probe", "https://zec.rocks:443"]).expect("probe with a uri parses"),
            NymSubCommand::Probe {
                target: Some("https://zec.rocks:443".parse().expect("static uri")),
            }
        );
        assert!(matches!(
            parse_nym_args(&["probe", "not a uri"]),
            Err(NymCommandError::InvalidProbeTarget(_))
        ));
        assert!(
            matches!(
                parse_nym_args(&["probe", "http://zec.rocks:9067"]),
                Err(NymCommandError::InvalidProbeTarget(_))
            ),
            "a plaintext http target is refused: mixnet transmission is https-only"
        );
        assert_eq!(
            parse_nym_args(&["history"]).expect("history parses"),
            NymSubCommand::History
        );
    }

    /// HYPOTHESIS: the paired-probe rendering makes a mixnet-specific failure
    /// legible at a glance: clearnet ok beside mixnet FAILED. Falsified if
    /// either leg's outcome, timing, or the not-ready skip is dropped.
    #[cfg(feature = "nym")]
    #[test]
    fn paired_probe_renders_both_legs_side_by_side() {
        use zingo_net_diag::{NetOpFailure, NetOpStage};
        use zingolib::nym::probe::{PairedProbe, ProbeLeg, ProbeSuccess};

        let tip = ProbeSuccess {
            chain: "main".to_string(),
            height: 3_420_400,
        };
        let mixnet_specific = PairedProbe {
            host: "carover0.xyz".to_string(),
            clearnet: ProbeLeg {
                outcome: Ok(tip.clone()),
                millis: 210,
            },
            mixnet: Some(ProbeLeg {
                outcome: Err(NetOpFailure {
                    stage: NetOpStage::SocksHandshake,
                    target: "carover0.xyz".to_string(),
                    cause_chain: vec![
                        "the mixnet exit could not reach carover0.xyz:9067 (timed out after 20.0s)"
                            .to_string(),
                    ],
                }),
                millis: 20_000,
            }),
        };
        assert_eq!(
            render_paired_probe(&mixnet_specific),
            "carover0.xyz\n  clearnet: ok in 210ms: chain main, height 3420400\n  mixnet:   FAILED after 20000ms: failed at socks-handshake to carover0.xyz: the mixnet exit could not reach carover0.xyz:9067 (timed out after 20.0s)"
        );

        let proxy_not_ready = PairedProbe {
            host: "zec.rocks".to_string(),
            clearnet: ProbeLeg {
                outcome: Ok(tip),
                millis: 180,
            },
            mixnet: None,
        };
        assert_eq!(
            render_paired_probe(&proxy_not_ready),
            "zec.rocks\n  clearnet: ok in 180ms: chain main, height 3420400\n  mixnet:   skipped (mixnet proxy not ready)"
        );
    }

    /// HYPOTHESIS: the history rendering aggregates per host and route with
    /// the most recent outcome and its age. Falsified if counts mix routes
    /// or the last outcome reflects file order rather than timestamps.
    #[cfg(all(feature = "nym", feature = "nym-diary"))]
    #[test]
    fn history_aggregates_per_host_and_route() {
        use zingolib::lightclient::indexer_history::{
            AttemptKind, AttemptRoute, FailureKind, IndexerAttempt,
        };

        let attempt = |host: &str, route, unix_secs, outcome| IndexerAttempt {
            unix_secs,
            host: host.to_string(),
            route,
            kind: AttemptKind::Send,
            millis: 10,
            outcome,
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

    #[cfg(not(feature = "nym"))]
    #[test]
    fn feature_absent_renders_byte_identically_to_the_replaced_string() {
        assert_eq!(
            NymCommandError::FeatureAbsent.to_string(),
            "This build has no Nym mixnet support. Rebuild zingo-cli with `--features nym`."
        );
    }

    /// Pins the `nym status` mode strings via the pure renderer.
    #[cfg(feature = "nym")]
    #[test]
    fn status_lines_render_byte_identically_to_the_replaced_strings() {
        use zingolib::nym::MixnetMode;

        assert_eq!(
            render_status(MixnetMode::Unattached, None, None),
            "Mixnet Mode: unattached. The mixnet has not been enabled, and no consent to \
             clearnet has been given: send and price-fetch refuse. Run `nym on` to enable \
             the mixnet, or `nym off` to use clearnet.",
            "absence is not consent: unattached names refusal, never clearnet"
        );
        assert_eq!(
            render_status(MixnetMode::SwitchedOff, None, None),
            "Mixnet Mode: switched off (send and price-fetch use clearnet)"
        );
        assert_eq!(
            render_status(MixnetMode::Bootstrapping, None, None),
            "Mixnet Mode: bootstrapping (send and price-fetch are unavailable until ready)"
        );
        assert_eq!(
            render_status(MixnetMode::Ready, Some("127.0.0.1:43210"), None),
            "Mixnet Mode: ready (SOCKS5 127.0.0.1:43210)"
        );
        assert_eq!(
            render_status(MixnetMode::Ready, None, None),
            "Mixnet Mode: ready",
            "ready with no address yet still renders (the route resolver, \
             not the renderer, refuses that state)"
        );
        assert_eq!(
            render_status(MixnetMode::Died, None, None),
            "Mixnet Mode: died. The proxy exited unexpectedly. Send and price-fetch \
             refuse and will not fall back to clearnet. Run `nym on` to restart the proxy.",
            "a died proxy is reported distinctly from switched off, and tells the user how to \
             recover"
        );
    }

    /// HYPOTHESIS: live bootstrap progress reaches the `nym status` line, so
    /// the connect race is narrated rather than an opaque wait. Falsified if
    /// the detail is dropped by the renderer. The detail is shown only while
    /// bootstrapping: a ready proxy has no bootstrap left to narrate.
    #[cfg(feature = "nym")]
    #[test]
    fn bootstrap_detail_reaches_the_status_line_only_while_bootstrapping() {
        use zingolib::nym::MixnetMode;

        assert_eq!(
            render_status(
                MixnetMode::Bootstrapping,
                None,
                Some("attempt 2/10: 2 in flight, 0 failed")
            ),
            "Mixnet Mode: bootstrapping, attempt 2/10: 2 in flight, 0 failed \
             (send and price-fetch are unavailable until ready)"
        );
        assert_eq!(
            render_status(MixnetMode::Ready, Some("127.0.0.1:1"), Some("stale")),
            "Mixnet Mode: ready (SOCKS5 127.0.0.1:1)",
            "a stale detail must not leak into the ready line"
        );
    }

    /// HYPOTHESIS: `nym status` always carries the IP-correlation disclaimer in
    /// every mode, so a "ready" mixnet is never mistaken for end-to-end IP
    /// protection while synchronization stays on clearnet (ZIP-0318). The mode
    /// line is preserved verbatim as the first line. Falsified if the
    /// disclaimer is dropped in any mode, no longer leads with the mode line,
    /// or omits the sync/IP/indexer/balance risk it must name.
    #[cfg(feature = "nym")]
    #[test]
    fn status_always_carries_the_ip_correlation_disclaimer() {
        use zingolib::nym::MixnetMode;

        for mode in [
            MixnetMode::Unattached,
            MixnetMode::SwitchedOff,
            MixnetMode::Bootstrapping,
            MixnetMode::Ready,
            MixnetMode::Died,
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
    //! case would buy another proving run, not another guarantee); `nym on`
    //! and `nym probe` currently carry NO offline gate (they would emit
    //! traffic from an Offline session — a known gap tracked for the ADR
    //! 0024 session driver), and the REPL-owned `servers` command likewise
    //! probes the network unguarded.

    #![allow(clippy::disallowed_methods)]

    use zingolib::lightclient::LightClient;
    use zingolib::testutils::synthetic_wallet::SyntheticWalletBuilder;

    use super::super::{CommandError, RT, do_user_command_result};

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
        RT.block_on(do_user_command_result(command, args, client))
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
            Err(error) => error.to_string(),
        };
        assert!(
            !rendered.contains(OFFLINE_REFUSAL),
            "`{command}` must not be blocked by Offline mode: {rendered}"
        );
        rendered
    }

    /// Asserts `command` refuses offline through its `Err` channel with the
    /// typed Offline refusal.
    fn assert_refuses_offline_via_err(client: &mut LightClient, command: &str, args: &[&str]) {
        let error = exec(client, command, args).expect_err(command);
        assert!(
            error.to_string().contains(OFFLINE_REFUSAL),
            "`{command}` must refuse with the typed Offline error: {error}"
        );
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

        /// `nym status` reads the wallet's mode: an offline session never
        /// bootstraps the mixnet, so a fresh client reports unattached.
        #[cfg(feature = "nym")]
        #[test]
        fn nym_status_reports_unattached() {
            let output = assert_works_offline(&mut offline_client(), "nym", &["status"]);
            assert!(output.contains("unattached"), "{output}");
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
