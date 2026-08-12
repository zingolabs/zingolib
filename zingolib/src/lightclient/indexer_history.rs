//! Cross-session, per-indexer attempt history (the "indexer diary").
//!
//! Transmission arms and diagnostic probes can append one record per attempt
//! to a line-oriented file beside the wallet (`indexer-history.tsv`), so
//! indexer reliability accumulates across sessions instead of dying with each
//! error message: which hosts deliver, over which route, how fast, and which
//! category of failure presented. The file is append-only and best-effort
//! (recording must never fail a send), and the format is a tab-separated line
//! per attempt, parseable without any serialization dependency:
//!
//! ```text
//! unix_secs<TAB>host<TAB>route<TAB>kind<TAB>millis<TAB>outcome
//! ```
//!
//! `outcome` is `ok` or `err <kind>` where `<kind>` is one token from the
//! closed [`FailureKind`] set. Malformed lines are skipped on load, so a
//! corrupt tail never poisons the history.
//!
//! # Privacy contract
//!
//! The diary is an at-rest record of when this wallet transmitted and to
//! which indexer, sitting in plaintext beside the wallet file, so three
//! guards bound it (ratified in the PR #2470 review):
//!
//! 1. **Compile gate.** Only the `nym-diary` feature lets the wallet open a
//!    disk-backed handle. The default build constructs an inert one.
//! 2. **Runtime opt-in.** Even when compiled, a handle records nothing until
//!    the session enables it ([`LightClient::set_indexer_diary`]). Loading
//!    for display works regardless.
//! 3. **Sanitized and capped.** The outcome column holds a [`FailureKind`]
//!    token, never raw server prose (which can embed txids), and the file is
//!    compacted to the newest [`MAX_DIARY_ATTEMPTS`] records once it doubles
//!    that count.
//!
//! [`LightClient::set_indexer_diary`]: crate::lightclient::LightClient
#![forbid(unsafe_code)]

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The diary keeps at most this many newest attempts after compaction, and
/// compacts whenever the file grows past twice this count.
pub const MAX_DIARY_ATTEMPTS: usize = 1024;

/// The network route an attempt used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptRoute {
    /// A direct clearnet connection.
    Clearnet,
    /// Through the mixnet's SOCKS5 proxy.
    Mixnet,
}

impl AttemptRoute {
    fn as_str(self) -> &'static str {
        match self {
            AttemptRoute::Clearnet => "clearnet",
            AttemptRoute::Mixnet => "mixnet",
        }
    }

    fn parse(token: &str) -> Option<Self> {
        match token {
            "clearnet" => Some(AttemptRoute::Clearnet),
            "mixnet" => Some(AttemptRoute::Mixnet),
            _ => None,
        }
    }
}

/// What the attempt was doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptKind {
    /// A real transaction submission.
    Send,
    /// A diagnostic `GetLightdInfo` probe.
    Probe,
}

impl AttemptKind {
    fn as_str(self) -> &'static str {
        match self {
            AttemptKind::Send => "send",
            AttemptKind::Probe => "probe",
        }
    }

    fn parse(token: &str) -> Option<Self> {
        match token {
            "send" => Some(AttemptKind::Send),
            "probe" => Some(AttemptKind::Probe),
            _ => None,
        }
    }
}

/// The closed set of failure categories the diary may store. Raw server or
/// transport prose never reaches disk, since it can embed transaction ids (zebrad
/// duplicate rejections quote them), so every failure is classified down to
/// one of these tokens at the recording boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// The attempt ran out of time (connect or request deadline).
    Timeout,
    /// The host could not be reached: connect, proxy, socks, or DNS failure.
    Unreachable,
    /// Delivered but unverified when patience ran out ("queued for download").
    Queued,
    /// The server answered and said no.
    Rejected,
    /// Anything the classifier does not recognize.
    Other,
}

impl FailureKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            FailureKind::Timeout => "timeout",
            FailureKind::Unreachable => "unreachable",
            FailureKind::Queued => "queued",
            FailureKind::Rejected => "rejected",
            FailureKind::Other => "other",
        }
    }

    fn parse_token(token: &str) -> Option<Self> {
        match token {
            "timeout" => Some(FailureKind::Timeout),
            "unreachable" => Some(FailureKind::Unreachable),
            "queued" => Some(FailureKind::Queued),
            "rejected" => Some(FailureKind::Rejected),
            "other" => Some(FailureKind::Other),
            _ => None,
        }
    }

    /// Classify a failure message into its diary category. Substring matches,
    /// like [`classify_rejection`], because the servers surface failures
    /// untyped (zingolabs/zaino#1392). The message itself is then discarded.
    ///
    /// [`classify_rejection`]: crate::lightclient::transmit
    pub(crate) fn classify(detail: &str) -> Self {
        let lowered = detail.to_ascii_lowercase();
        if lowered.contains("already queued for download") {
            return FailureKind::Queued;
        }
        if lowered.contains("timed out")
            || lowered.contains("timeout")
            || lowered.contains("deadlineexceeded")
            || lowered.contains("deadline exceeded")
        {
            return FailureKind::Timeout;
        }
        if lowered.contains("connect")
            || lowered.contains("refused")
            || lowered.contains("unreachable")
            || lowered.contains("proxy")
            || lowered.contains("socks")
            || lowered.contains("dns")
        {
            return FailureKind::Unreachable;
        }
        if lowered.contains("reject") || lowered.contains("invalid") {
            return FailureKind::Rejected;
        }
        FailureKind::Other
    }

    /// Parse a stored outcome detail: a current-format token directly, or a
    /// legacy raw-prose line (written before sanitization) re-classified so
    /// old files keep their counts without their prose ever being surfaced.
    fn parse_or_classify(detail: &str) -> Self {
        Self::parse_token(detail).unwrap_or_else(|| Self::classify(detail))
    }
}

/// One recorded attempt against one indexer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexerAttempt {
    /// Seconds since the Unix epoch when the attempt finished.
    pub unix_secs: u64,
    /// The indexer host the attempt targeted.
    pub host: crate::correspondent::Host,
    /// The route the attempt used.
    pub route: AttemptRoute,
    /// Whether the attempt was a send or a probe.
    pub kind: AttemptKind,
    /// How long the attempt took, in milliseconds.
    pub millis: u64,
    /// `Ok(())` on success, or the sanitized failure category.
    pub outcome: Result<(), FailureKind>,
    /// Which party a failure is charged against, when the evidence says.
    pub phase: Option<crate::correspondent::health::FailurePhase>,
    /// The Exit Node the attempt rode, when it rode one.
    pub exit: Option<crate::mixnet::ExitNodeId>,
}

impl IndexerAttempt {
    fn to_line(&self) -> String {
        let outcome = match self.outcome {
            Ok(()) => "ok".to_string(),
            Err(kind) => format!("err {}", kind.as_str()),
        };
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            self.unix_secs,
            flatten(self.host.as_str()),
            self.route.as_str(),
            self.kind.as_str(),
            self.millis,
            self.phase.map_or("-", |phase| phase.as_str()),
            self.exit
                .as_ref()
                .map_or("-".to_string(), |exit| flatten(exit.as_str())),
            outcome
        )
    }

    fn parse(line: &str) -> Option<Self> {
        let fields: Vec<&str> = line.split('\t').collect();
        // A six-field line predates the phase and exit columns; it loads
        // with neither rather than being skipped.
        let (phase, exit, outcome_token) = match fields.len() {
            6 => (None, None, fields[5]),
            8 => (
                (fields[5] != "-")
                    .then(|| crate::correspondent::health::FailurePhase::parse(fields[5])),
                match fields[6] {
                    "-" => None,
                    token => Some(crate::mixnet::ExitNodeId::parse(token).ok()?),
                },
                fields[7],
            ),
            _ => return None,
        };
        let unix_secs = fields[0].parse().ok()?;
        let host = crate::correspondent::Host::of_host_str(fields[1]);
        let route = AttemptRoute::parse(fields[2])?;
        let kind = AttemptKind::parse(fields[3])?;
        let millis = fields[4].parse().ok()?;
        let outcome = match outcome_token {
            "ok" => Ok(()),
            other => Err(FailureKind::parse_or_classify(other.strip_prefix("err ")?)),
        };
        Some(IndexerAttempt {
            unix_secs,
            host,
            route,
            kind,
            millis,
            phase,
            exit,
            outcome,
        })
    }
}

/// Tabs and newlines delimit the format, so field content flattens them.
fn flatten(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

/// The current time as seconds since the Unix epoch.
pub(crate) fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// A cloneable appender/loader for the per-indexer history file. `None` for
/// path-less clients (synthetic test wallets, and every build without the
/// `nym-diary` feature), which record nowhere and load empty. Recording is
/// additionally off until the session opts in, and clones share the switch.
#[derive(Clone, Debug, Default)]
pub struct IndexerHistoryHandle {
    path: Option<PathBuf>,
    recording: Arc<AtomicBool>,
    /// The session's Health, updated by every attempt whatever the diary's
    /// gates say, since a judgment kept in memory carries no at-rest risk.
    health: Arc<std::sync::Mutex<crate::correspondent::health::Health>>,
}

impl IndexerHistoryHandle {
    /// A handle writing beside the wallet file: `wallet_dir/indexer-history.tsv`.
    /// Starts with recording off. [`Self::set_recording`] opts the session in.
    #[cfg_attr(not(feature = "nym-diary"), allow(dead_code))]
    pub(crate) fn beside_wallet(wallet_path: &std::path::Path) -> Self {
        IndexerHistoryHandle {
            path: wallet_path
                .parent()
                .map(|dir| dir.join("indexer-history.tsv")),
            recording: Arc::new(AtomicBool::new(false)),
            health: Arc::default(),
        }
    }

    /// The session's Health, for the draws that consult it.
    #[cfg_attr(not(feature = "nym"), allow(dead_code))]
    pub(crate) fn health(&self) -> &std::sync::Mutex<crate::correspondent::health::Health> {
        &self.health
    }

    /// Turns recording on or off for every clone of this handle. A per-session
    /// switch: nothing persists it.
    #[cfg_attr(not(feature = "nym-diary"), allow(dead_code))]
    pub(crate) fn set_recording(&self, recording: bool) {
        self.recording.store(recording, Ordering::Relaxed);
    }

    /// Whether this session records attempts.
    pub fn is_recording(&self) -> bool {
        self.path.is_some() && self.recording.load(Ordering::Relaxed)
    }

    /// Appends one attempt when the session has opted in ([`Self::is_recording`]),
    /// then compacts the file if it outgrew twice [`MAX_DIARY_ATTEMPTS`].
    /// Best-effort by contract: an unwritable history must never fail the send
    /// or probe it describes, so I/O errors are swallowed after a log line.
    pub(crate) fn record(&self, attempt: &IndexerAttempt) {
        self.health.lock().expect("health mutex").note(
            &attempt.host,
            attempt.outcome.is_err(),
            attempt.phase,
        );
        if !self.is_recording() {
            return;
        }
        let Some(path) = &self.path else {
            return;
        };
        let appended = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(attempt.to_line().as_bytes()))
            .and_then(|()| enforce_cap(path));
        if let Err(e) = appended {
            log::warn!("indexer history not recorded ({}): {e}", path.display());
        }
    }

    /// Loads every parseable attempt, oldest first. Malformed lines are
    /// skipped. A missing file is an empty history. Loading works whether or
    /// not the session records, so past sessions' data stays displayable.
    pub fn load(&self) -> Vec<IndexerAttempt> {
        let Some(path) = &self.path else {
            return Vec::new();
        };
        let Ok(contents) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        contents.lines().filter_map(IndexerAttempt::parse).collect()
    }
}

/// Rewrites the file down to its newest [`MAX_DIARY_ATTEMPTS`] lines once it
/// holds more than twice that many, so the diary is bounded for the life of
/// the wallet. The rewrite goes through a sibling temp file and a rename, so
/// a crash mid-compaction leaves either the old file or the new one.
fn enforce_cap(path: &std::path::Path) -> std::io::Result<()> {
    let contents = std::fs::read_to_string(path)?;
    let total = contents.lines().count();
    if total <= MAX_DIARY_ATTEMPTS * 2 {
        return Ok(());
    }
    let mut kept: String = contents
        .lines()
        .skip(total - MAX_DIARY_ATTEMPTS)
        .collect::<Vec<_>>()
        .join("\n");
    kept.push('\n');
    let temp = path.with_extension("tsv.compacting");
    std::fs::write(&temp, kept)?;
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn an_attempt(host: &str, outcome: Result<(), FailureKind>) -> IndexerAttempt {
        IndexerAttempt {
            unix_secs: 1_700_000_000,
            host: crate::correspondent::Host::of_host_str(host),
            route: AttemptRoute::Mixnet,
            kind: AttemptKind::Send,
            millis: 1234,
            phase: outcome
                .is_err()
                .then_some(crate::correspondent::health::FailurePhase::Correspondent),
            exit: Some(
                crate::mixnet::ExitNodeId::parse("exit-alpha").expect("the test identity parses"),
            ),
            outcome,
        }
    }

    /// A disk-backed handle with recording already opted in, as production
    /// reaches it after `LightClient::set_indexer_diary(true)`.
    fn recording_handle(dir: &std::path::Path) -> IndexerHistoryHandle {
        let handle = IndexerHistoryHandle::beside_wallet(&dir.join("zingo-wallet.dat"));
        handle.set_recording(true);
        handle
    }

    /// HYPOTHESIS: records round-trip through the file byte-exactly.
    /// Falsified if any field is lost or reordered across append + load.
    #[test]
    fn attempts_round_trip_through_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let handle = recording_handle(dir.path());

        let ok = an_attempt("zec.rocks", Ok(()));
        let failed = an_attempt("carover0.xyz", Err(FailureKind::Timeout));
        handle.record(&ok);
        handle.record(&failed);

        assert_eq!(handle.load(), vec![ok, failed]);
    }

    /// HYPOTHESIS: recording is off until the session opts in, and the opt-in
    /// reaches every clone. Falsified if a fresh disk-backed handle writes.
    #[test]
    fn recording_is_off_until_the_session_opts_in() {
        let dir = tempfile::tempdir().expect("temp dir");
        let handle = IndexerHistoryHandle::beside_wallet(&dir.path().join("zingo-wallet.dat"));
        let clone = handle.clone();

        clone.record(&an_attempt("zec.rocks", Ok(())));
        assert!(!dir.path().join("indexer-history.tsv").exists());
        assert!(!clone.is_recording());

        handle.set_recording(true);
        clone.record(&an_attempt("zec.rocks", Ok(())));
        assert_eq!(clone.load().len(), 1, "the opt-in reaches clones");
    }

    /// HYPOTHESIS: no raw failure prose reaches disk, and the outcome column is
    /// a closed token. Falsified if the file contains anything but the token.
    #[test]
    fn failure_prose_never_reaches_disk() {
        let dir = tempfile::tempdir().expect("temp dir");
        let handle = recording_handle(dir.path());

        handle.record(&an_attempt("a.example", Err(FailureKind::Rejected)));

        let raw = std::fs::read_to_string(dir.path().join("indexer-history.tsv"))
            .expect("diary file exists");
        assert_eq!(
            raw,
            "1700000000\ta.example\tmixnet\tsend\t1234\tcorrespondent\texit-alpha\terr rejected\n"
        );
    }

    /// Legacy lines written before sanitization carried raw prose. They load
    /// as their classified category so counts survive without the prose.
    #[test]
    fn legacy_prose_lines_load_as_classified_categories() {
        let dir = tempfile::tempdir().expect("temp dir");
        let handle = recording_handle(dir.path());
        std::fs::write(
            dir.path().join("indexer-history.tsv"),
            "1700000000\tcarover0.xyz\tmixnet\tsend\t99\terr the mixnet exit could not reach \
             carover0.xyz:9067 (timed out)\n",
        )
        .expect("write a legacy line");

        assert_eq!(handle.load()[0].outcome, Err(FailureKind::Timeout));
    }

    /// The classifier maps each failure family onto its token and never
    /// panics on arbitrary prose.
    #[test]
    fn the_classifier_covers_the_failure_families() {
        assert_eq!(
            FailureKind::classify("transaction already queued for download"),
            FailureKind::Queued
        );
        assert_eq!(
            FailureKind::classify("clearnet connect timed out after 30s"),
            FailureKind::Timeout
        );
        assert_eq!(
            FailureKind::classify("status: DeadlineExceeded"),
            FailureKind::Timeout
        );
        assert_eq!(
            FailureKind::classify("SOCKS5 handshake failed"),
            FailureKind::Unreachable
        );
        assert_eq!(
            FailureKind::classify("connection refused"),
            FailureKind::Unreachable
        );
        assert_eq!(
            FailureKind::classify("transaction rejected: txid deadbeef"),
            FailureKind::Rejected
        );
        assert_eq!(
            FailureKind::classify("zebra said something novel"),
            FailureKind::Other
        );
    }

    /// HYPOTHESIS: the diary is bounded, and once past twice the cap it compacts
    /// to the newest [`MAX_DIARY_ATTEMPTS`] records. Falsified if the file
    /// grows without bound or drops the newest entries.
    #[test]
    fn the_diary_compacts_to_the_newest_records() {
        let dir = tempfile::tempdir().expect("temp dir");
        let handle = recording_handle(dir.path());

        let total = MAX_DIARY_ATTEMPTS * 2 + 3;
        for index in 0..total {
            let mut attempt = an_attempt("zec.rocks", Ok(()));
            attempt.unix_secs = index as u64;
            handle.record(&attempt);
        }

        let loaded = handle.load();
        assert!(
            loaded.len() <= MAX_DIARY_ATTEMPTS * 2,
            "bounded: {}",
            loaded.len()
        );
        assert_eq!(
            loaded.last().expect("nonempty").unix_secs,
            (total - 1) as u64,
            "the newest record survives compaction"
        );
        assert!(
            loaded.first().expect("nonempty").unix_secs > 0,
            "the oldest records are the ones dropped"
        );
    }

    /// A tab or newline inside a host must not corrupt the line format for
    /// later records.
    #[test]
    fn hostile_host_text_cannot_break_the_format() {
        let dir = tempfile::tempdir().expect("temp dir");
        let handle = recording_handle(dir.path());

        handle.record(&an_attempt("tab\there\nand newline", Ok(())));
        handle.record(&an_attempt("b", Ok(())));

        let loaded = handle.load();
        assert_eq!(loaded.len(), 2, "both records survive: {loaded:?}");
        assert_eq!(
            loaded[0].host,
            crate::correspondent::Host::of_host_str("tab here and newline")
        );
    }

    /// A corrupt tail (torn write) is skipped rather than poisoning the load.
    #[test]
    fn malformed_lines_are_skipped() {
        let dir = tempfile::tempdir().expect("temp dir");
        let handle = recording_handle(dir.path());
        handle.record(&an_attempt("zec.rocks", Ok(())));
        std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join("indexer-history.tsv"))
            .and_then(|mut f| f.write_all(b"17000000\tzec.rocks\ttruncat"))
            .expect("append a torn line");

        assert_eq!(handle.load().len(), 1);
    }

    /// A path-less handle records nowhere and loads empty (the synthetic
    /// wallet and diary-less build contract) even when told to record.
    #[test]
    fn a_pathless_handle_is_inert() {
        let handle = IndexerHistoryHandle::default();
        handle.set_recording(true);
        handle.record(&an_attempt("zec.rocks", Ok(())));
        assert!(handle.load().is_empty());
        assert!(!handle.is_recording());
    }
}
