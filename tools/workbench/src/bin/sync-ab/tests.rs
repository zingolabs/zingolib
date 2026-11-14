use super::*;

fn request(seconds: u64, seed: &str) -> Request {
    Request {
        specs: [Spec::parse("a"), Spec::parse("b")],
        birthday: 2_571_710,
        runs: DEFAULT_RUNS,
        seconds,
        seed: seed.to_string(),
        server: DEFAULT_INDEXER.to_string(),
        json: None,
    }
}

#[test]
fn benchmark_names_the_role_the_kind_and_the_window() {
    let bounded = request(90, GUARD_MNEMONIC);
    assert_eq!(
        benchmark(BASELINE_ROLE, &Spec::parse("a:nakednet"), &bounded),
        "baseline:nakednet (birthday 2571710, 90s)"
    );
    let whole = request(0, "some other mnemonic");
    assert_eq!(
        benchmark(CANDIDATE_ROLE, &Spec::parse("b"), &whole),
        "candidate (birthday 2571710, to completion, seeded)"
    );
}

#[test]
fn metric_format_bounds_a_reading_only_where_a_deviation_exists() {
    let series = vec![(
        "candidate (birthday 1, 90s)".to_string(),
        vec![
            metric(OUTPUTS_PER_SECOND, &[100.0, 110.0, 120.0]),
            metric(BOOT_SECONDS, &[2.5]),
        ],
    )];
    assert_eq!(
        metric_format(&series),
        "{\n  \"candidate (birthday 1, 90s)\": {\n    \"outputs-per-second\": \
             {\"value\": 110, \"lower_value\": 100, \"upper_value\": 120},\n    \
             \"boot-seconds\": {\"value\": 2.5}\n  }\n}\n"
    );
}

/// The launch as the beta tag writes it, five arguments on one line.
const EARLY_ERA: &str = "        let sync_handle = tokio::spawn(async move {
            pepper_sync::sync(client, &chain_type, wallet, sync_mode, sync_config).await
        });
        self.sync_handle = Some(sync_handle);
";

/// The launch as dev writes it, six arguments across several lines.
const LATER_ERA: &str = "        let sync_handle = tokio::spawn(async move {
            pepper_sync::sync(
                client,
                &chain_type,
                wallet,
                sync_mode,
                progress_sender,
                sync_config,
            )
            .await
        });
        self.sync_handle = Some(sync_handle);
";

/// HYPOTHESIS: the graft carries every era's call through untouched and
/// surrounds it with the span. Falsified if a call is altered or a
/// marker is missing.
#[test]
fn the_graft_surrounds_each_era_without_touching_its_call() -> Result<(), Vec<String>> {
    for era in [EARLY_ERA, LATER_ERA] {
        let grafted = graft(era)?;
        assert!(
            grafted.contains(SYNC_SPAN_OPEN) && grafted.contains(SYNC_SPAN_CLOSE),
            "the span must open and close: {grafted}"
        );
        assert!(
            grafted.contains(SYNC_SPAN_OUTPUTS),
            "the close must carry the output count: {grafted}"
        );
        let after_anchor = workbench::required(
            era.split_once(SPAWN_ANCHOR),
            "sync launch anchor in the era",
        )?
        .1;
        let call = workbench::required(
            after_anchor.rsplit_once("});"),
            "closing of the launch in the era",
        )?
        .0;
        assert!(
            grafted.contains(call.trim_end()),
            "the call must survive verbatim: {grafted}"
        );
        assert!(
            grafted.ends_with("        self.sync_handle = Some(sync_handle);\n"),
            "everything after the launch must survive: {grafted}"
        );
    }
    Ok(())
}

/// HYPOTHESIS: a graft that cannot find exactly one launch refuses
/// rather than instrumenting the wrong work. Falsified if either shape
/// returns a grafted source.
#[test]
fn an_unanchorable_source_is_refused() {
    assert!(graft("fn main() {}").is_err(), "no anchor must refuse");
    let doubled = format!("{EARLY_ERA}{LATER_ERA}");
    assert!(graft(&doubled).is_err(), "two anchors must refuse");
}

/// HYPOTHESIS: an anchored block that scans nothing is refused, since
/// instrumenting it would time work this tool does not mean.
#[test]
fn an_anchored_block_that_does_not_scan_is_refused() {
    let impostor = "        let sync_handle = tokio::spawn(async move {
            some_other_task().await
        });
";
    assert!(graft(impostor).is_err(), "a foreign block must refuse");
}

/// HYPOTHESIS: an arm's kind is read off its suffix, and the label
/// keeps it. Falsified if a plain commit reads as nakednet or the
/// suffix survives into the commit.
#[test]
fn an_arm_reads_its_build_kind_off_its_suffix() {
    let plain = Spec::parse("dev");
    assert_eq!(plain.commit, "dev");
    assert!(!plain.nakednet);
    let bare = Spec::parse("dev:nakednet");
    assert_eq!(bare.commit, "dev");
    assert!(bare.nakednet);
    assert_eq!(bare.label(), "dev:nakednet");
}

/// HYPOTHESIS: a closing line is read only once its duration and count
/// are both whole. Falsified if a truncated line yields a reading.
#[test]
fn a_truncated_closing_line_yields_no_reading() {
    assert_eq!(
        closing_span("SYNC_SPAN=close 1250ms ok outputs=8192\n"),
        Some((1250, 8192))
    );
    assert_eq!(closing_span("SYNC_SPAN=close 12"), None);
    assert_eq!(closing_span("SYNC_SPAN=close 1250ms err\n"), None);
}

// HYPOTHESIS: a span that closed on an error is told apart from one
// that finished, one still open, one whose close is cut short, and one
// the CLI relaunched after the error.
#[test]
fn a_failed_close_is_read_as_a_failure() {
    assert!(failed_span("INFO sync: SYNC_SPAN=close 137361ms err\n"));
    assert!(!failed_span(
        "INFO sync: SYNC_SPAN=close 1250ms ok outputs=8192\n"
    ));
    assert!(!failed_span("INFO sync: SYNC_SPAN=open\n"));
    assert!(!failed_span("INFO sync: SYNC_SPAN=close 1250ms er"));
    assert!(!failed_span(
        "INFO sync: SYNC_SPAN=close 1200ms err\nINFO sync: SYNC_SPAN=open\n"
    ));
    assert!(!failed_span(
        "INFO sync: SYNC_SPAN=close 1200ms err\nINFO sync: SYNC_SPAN=open\n\
             INFO sync: SYNC_SPAN=close 1250ms ok outputs=8192\n"
    ));
}

#[test]
fn a_bare_close_before_the_counted_one_is_skipped() {
    let log =
        "INFO sync: SYNC_SPAN=close 1250ms ok\nINFO sync: SYNC_SPAN=close 1250ms ok outputs=8192\n";
    assert_eq!(closing_span(log), Some((1250, 8192)));
}

// HYPOTHESIS: a stat line is matched on the command name alone, read up
// to its last closing parenthesis. Falsified if the proxy, the builder,
// or a name holding a parenthesis is taken for the CLI.
#[test]
fn a_stat_line_names_the_cli() {
    assert!(names_cli("4242 (zingolib-cli) S 4200 4100 4100 0 -1"));
    assert!(!names_cli("4243 (nym-proxy) S 4242 4243 4100 0 -1"));
    assert!(!names_cli("4244 (cargo) S 4200 4100 4100 0 -1"));
    assert!(!names_cli(
        "4245 (zingolib-cli (old)) S 4200 4100 4100 0 -1"
    ));
    assert!(!names_cli("no fields at all"));
}

// HYPOTHESIS: a byte mean is undefined as soon as one session gave no
// reading, so the arms are never averaged over different sessions.
#[test]
fn a_missing_reading_leaves_the_mean_undefined() {
    assert_eq!(mean_bytes(&[Some(KIB), Some(3 * KIB)]), Some(2 * KIB));
    assert_eq!(mean_bytes(&[Some(KIB), None]), None);
    assert_eq!(mean_bytes(&[]), None);
}

// HYPOTHESIS: the high-water mark is read in bytes off its own status
// line. Falsified if another memory line is taken for it or a status
// without one yields a reading.
#[test]
fn the_high_water_mark_is_read_in_bytes() {
    let status =
        "Name:\tzingolib-cli\nVmPeak:\t    2048 kB\nVmHWM:\t    1536 kB\nVmRSS:\t    1024 kB\n";
    assert_eq!(high_water(status), Some(1536 * KIB));
    assert_eq!(high_water("Name:\tzingolib-cli\nVmRSS:\t 1024 kB\n"), None);
}

const COMMIT_AS_NOBODY: [&str; 9] = [
    "-c",
    "user.name=sync-ab",
    "-c",
    "user.email=sync-ab@example.invalid",
    "-c",
    "commit.gpgsign=false",
    "commit",
    "--quiet",
    "--message=layout",
];

#[test]
fn an_arm_of_either_layout_is_prepared() -> Result<(), Vec<String>> {
    for layout in &LAYOUTS {
        let scratch = workbench::fresh_dir(&std::env::temp_dir().join(format!(
            "sync-ab-{}-{}",
            layout.cli,
            std::process::id()
        )))?;
        let root = scratch.join("repo");
        let source = root.join(layout.sync_source);
        workbench::create_parent(&source)?;
        workbench::write(&source, EARLY_ERA)?;
        git(&root, &["init", "--quiet"])?;
        git(&root, &["add", "--all"])?;
        git(&root, &COMMIT_AS_NOBODY)?;
        let prepared = prepare(&root, &Spec::parse("HEAD"));
        let _ = std::fs::remove_dir_all(&scratch);
        assert_eq!(prepared?.layout.cli, layout.cli);
    }
    Ok(())
}
