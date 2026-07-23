//! Cross-session, per-indexer attempt history.
//!
//! Every transmission arm and every diagnostic probe appends one record to a
//! line-oriented file beside the wallet (`indexer-history.tsv`), so indexer
//! reliability accumulates across sessions instead of dying with each error
//! message: which hosts deliver, over which route, how fast, and how each
//! failure presented. The file is append-only and best-effort — recording
//! must never fail a send — and the format is a tab-separated line per
//! attempt, parseable without any serialization dependency:
//!
//! ```text
//! unix_secs<TAB>host<TAB>route<TAB>kind<TAB>millis<TAB>outcome
//! ```
//!
//! `outcome` is `ok` or `err <detail>` with tabs and newlines flattened to
//! spaces. Malformed lines are skipped on load, so a corrupt tail never
//! poisons the history.
#![forbid(unsafe_code)]

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// One recorded attempt against one indexer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexerAttempt {
    /// Seconds since the Unix epoch when the attempt finished.
    pub unix_secs: u64,
    /// The indexer host the attempt targeted.
    pub host: String,
    /// The route the attempt used.
    pub route: AttemptRoute,
    /// Whether the attempt was a send or a probe.
    pub kind: AttemptKind,
    /// How long the attempt took, in milliseconds.
    pub millis: u64,
    /// `Ok(())` on success, or the failure detail.
    pub outcome: Result<(), String>,
}

impl IndexerAttempt {
    fn to_line(&self) -> String {
        let outcome = match &self.outcome {
            Ok(()) => "ok".to_string(),
            Err(detail) => format!("err {}", flatten(detail)),
        };
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            self.unix_secs,
            flatten(&self.host),
            self.route.as_str(),
            self.kind.as_str(),
            self.millis,
            outcome
        )
    }

    fn parse(line: &str) -> Option<Self> {
        let mut fields = line.splitn(6, '\t');
        let unix_secs = fields.next()?.parse().ok()?;
        let host = fields.next()?.to_string();
        let route = AttemptRoute::parse(fields.next()?)?;
        let kind = AttemptKind::parse(fields.next()?)?;
        let millis = fields.next()?.parse().ok()?;
        let outcome = match fields.next()? {
            "ok" => Ok(()),
            other => Err(other.strip_prefix("err ")?.to_string()),
        };
        Some(IndexerAttempt {
            unix_secs,
            host,
            route,
            kind,
            millis,
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
/// path-less clients (synthetic test wallets), which record nowhere and load
/// empty.
#[derive(Clone, Debug, Default)]
pub struct IndexerHistoryHandle {
    path: Option<PathBuf>,
}

impl IndexerHistoryHandle {
    /// A handle writing beside the wallet file: `wallet_dir/indexer-history.tsv`.
    pub(crate) fn beside_wallet(wallet_path: &std::path::Path) -> Self {
        IndexerHistoryHandle {
            path: wallet_path
                .parent()
                .map(|dir| dir.join("indexer-history.tsv")),
        }
    }

    /// Appends one attempt. Best-effort by contract: an unwritable history
    /// must never fail the send or probe it describes, so I/O errors are
    /// swallowed after a log line.
    pub(crate) fn record(&self, attempt: &IndexerAttempt) {
        let Some(path) = &self.path else {
            return;
        };
        let appended = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(attempt.to_line().as_bytes()));
        if let Err(e) = appended {
            log::warn!("indexer history not recorded ({}): {e}", path.display());
        }
    }

    /// Loads every parseable attempt, oldest first. Malformed lines are
    /// skipped; a missing file is an empty history.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn an_attempt(host: &str, outcome: Result<(), String>) -> IndexerAttempt {
        IndexerAttempt {
            unix_secs: 1_700_000_000,
            host: host.to_string(),
            route: AttemptRoute::Mixnet,
            kind: AttemptKind::Send,
            millis: 1234,
            outcome,
        }
    }

    /// HYPOTHESIS: records round-trip through the file byte-exactly.
    /// Falsified if any field is lost or reordered across append + load.
    #[test]
    fn attempts_round_trip_through_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let handle = IndexerHistoryHandle::beside_wallet(&dir.path().join("zingo-wallet.dat"));

        let ok = an_attempt("zec.rocks", Ok(()));
        let failed = an_attempt(
            "carover0.xyz",
            Err("the mixnet exit could not reach carover0.xyz:9067 (timed out)".to_string()),
        );
        handle.record(&ok);
        handle.record(&failed);

        assert_eq!(handle.load(), vec![ok, failed]);
    }

    /// A tab or newline inside a failure detail must not corrupt the line
    /// format for later records.
    #[test]
    fn hostile_detail_text_cannot_break_the_format() {
        let dir = tempfile::tempdir().expect("temp dir");
        let handle = IndexerHistoryHandle::beside_wallet(&dir.path().join("zingo-wallet.dat"));

        handle.record(&an_attempt("a", Err("tab\there\nand newline".to_string())));
        handle.record(&an_attempt("b", Ok(())));

        let loaded = handle.load();
        assert_eq!(loaded.len(), 2, "both records survive: {loaded:?}");
        assert_eq!(loaded[0].outcome, Err("tab here and newline".to_string()));
    }

    /// A corrupt tail (torn write) is skipped rather than poisoning the load.
    #[test]
    fn malformed_lines_are_skipped() {
        let dir = tempfile::tempdir().expect("temp dir");
        let wallet = dir.path().join("zingo-wallet.dat");
        let handle = IndexerHistoryHandle::beside_wallet(&wallet);
        handle.record(&an_attempt("zec.rocks", Ok(())));
        std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join("indexer-history.tsv"))
            .and_then(|mut f| f.write_all(b"17000000\tzec.rocks\ttruncat"))
            .expect("append a torn line");

        assert_eq!(handle.load().len(), 1);
    }

    /// A path-less handle records nowhere and loads empty — the synthetic
    /// wallet contract.
    #[test]
    fn a_pathless_handle_is_inert() {
        let handle = IndexerHistoryHandle::default();
        handle.record(&an_attempt("zec.rocks", Ok(())));
        assert!(handle.load().is_empty());
    }
}
