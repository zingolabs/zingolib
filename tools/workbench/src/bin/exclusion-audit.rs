//! `exclusion-audit` checks that dropping a workspace member from a build
//! costs nothing.
//!
//! `Makefile.toml`'s `packages` set excludes the members listed in its
//! `build_excludable` array from the build, not merely from the run. That is
//! only free when the excluded member activates no feature that no other
//! member activates. Exclude one that does and cargo re-unifies features,
//! compiling a second variant of every crate beneath it — which costs far
//! more than the test binaries the exclusion skipped.
//!
//! This tool decides that in two stages. First it resolves the whole feature
//! graph twice, with and without the candidate, and compares every crate that
//! survives the exclusion; crates that disappear entirely are the excluded
//! member's private dependencies, which is the saving rather than a cost. The
//! comparison spans third-party crates as well as local ones, because a
//! feature moving anywhere is a recompile, and because the crates that matter
//! most here are not all workspace members: `zingo-netutils` is its own
//! workspace root, and `zcash_address` is a registry crate, yet both sit
//! under everything the phases share.
//!
//! That first pass over-reports. A workspace member's own default features
//! are activated by the command line rather than by a dependent, and a
//! forward tree prints a feature only where some dependent activates it, so a
//! member that stops being depended upon appears to lose features it still
//! has. The second stage re-asks `cargo tree -e features -i <crate>` about
//! each flagged crate, which reports activations wherever they come from, and
//! keeps only the shifts that survive. Every real shift reaches stage one —
//! a dependent's activation is always printed — so narrowing there and
//! confirming here is sound as well as fast.
//!
//! Candidates come from `Makefile.toml`, so this audits what the repo
//! actually declares; extra candidates named on the command line are audited
//! too, which is how a proposed addition gets checked before it is written
//! down.
//!
//! Invoked as `makers exclusion-audit`, optionally with candidates:
//! `makers exclusion-audit zingo-cli`.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use workbench::{read, repo_root, run};

/// The bash array in `Makefile.toml` naming the members the `packages` set
/// drops from the build.
const DECLARED_ARRAY: &str = "build_excludable";

/// Run `cargo tree <args>` from `dir` and return its stdout.
fn cargo_tree(dir: &Path, args: &[&str]) -> Result<String, Vec<String>> {
    let output = Command::new("cargo")
        .arg("tree")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| vec![format!("failed to run cargo tree: {e}")])?;
    if !output.status.success() {
        return Err(vec![
            format!("`cargo tree {}` failed", args.join(" ")),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ]);
    }
    String::from_utf8(output.stdout).map_err(|e| vec![format!("cargo tree output not utf-8: {e}")])
}

/// Every crate a flat `cargo tree` listing reaches, mapped to the features
/// the resolution turns on for it.
///
/// The flat form makes each line self-describing: `name v1.2.3` names a
/// crate and `name feature "x"` names an activation, both optionally
/// trailing cargo's `(*)` deduplication marker.
fn resolve(listing: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in listing.lines() {
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else { continue };
        match fields.next() {
            Some("feature") => {
                if let Some(feature) = line.split_once('"').and_then(|o| o.1.split_once('"')) {
                    graph
                        .entry(name.to_string())
                        .or_default()
                        .insert(feature.0.to_string());
                }
            }
            // A version field is what marks a crate line; a crate with no
            // features still has to appear, or it reads as absent.
            Some(version) if version.starts_with('v') => {
                graph.entry(name.to_string()).or_default();
            }
            _ => {}
        }
    }
    graph
}

/// The feature graph the workspace resolves to, with `excluded` left out
/// when it is `Some`.
fn feature_graph(
    dir: &Path,
    excluded: Option<&str>,
) -> Result<BTreeMap<String, BTreeSet<String>>, Vec<String>> {
    let mut args = vec!["--workspace", "-e", "features", "--prefix", "none"];
    if let Some(excluded) = excluded {
        args.push("--exclude");
        args.push(excluded);
    }
    Ok(resolve(&cargo_tree(dir, &args)?))
}

/// The features activated on `crate_name`, asked of the inverted tree, which
/// reports an activation wherever it comes from — the command line included.
fn activations(
    dir: &Path,
    crate_name: &str,
    excluded: Option<&str>,
) -> Result<BTreeSet<String>, Vec<String>> {
    let mut args = vec!["--workspace", "-e", "features", "-i", crate_name];
    if let Some(excluded) = excluded {
        args.push("--exclude");
        args.push(excluded);
    }
    let marker = format!("{crate_name} feature \"");
    Ok(cargo_tree(dir, &args)?
        .lines()
        .filter_map(|line| Some(line.split_once(&marker)?.1.split_once('"')?.0.to_string()))
        .collect())
}

/// The members named in `Makefile.toml`'s `build_excludable` array.
fn declared_candidates(makefile: &str) -> Result<Vec<String>, Vec<String>> {
    let assignment = format!("{DECLARED_ARRAY}=(");
    let opened = makefile.split_once(&assignment).ok_or_else(|| {
        vec![format!(
            "Makefile.toml declares no `{assignment}...)`; this tool audits that array, so a \
             rename must reach it too"
        )]
    })?;
    let body = opened
        .1
        .split_once(')')
        .ok_or_else(|| vec![format!("`{assignment}` is never closed in Makefile.toml")])?;
    Ok(body.0.split_whitespace().map(str::to_string).collect())
}

/// What one crate's features would lose and gain under an exclusion.
struct Shift {
    crate_name: String,
    lost: Vec<String>,
    gained: Vec<String>,
}

/// One candidate's verdict: the shifts an exclusion would cause, and how
/// many crates left the graph entirely, which is the saving it buys.
struct Verdict {
    candidate: String,
    shifts: Vec<Shift>,
    dropped: usize,
}

/// Compare the graphs, reporting only crates present in both: one absent
/// after the exclusion left with the excluded member and cost nothing.
fn compare(
    candidate: &str,
    full: &BTreeMap<String, BTreeSet<String>>,
    excluded: &BTreeMap<String, BTreeSet<String>>,
) -> Verdict {
    let mut shifts = Vec::new();
    let mut dropped = 0;
    for (crate_name, before) in full {
        let Some(after) = excluded.get(crate_name) else {
            dropped += 1;
            continue;
        };
        let lost: Vec<String> = before.difference(after).cloned().collect();
        let gained: Vec<String> = after.difference(before).cloned().collect();
        if !lost.is_empty() || !gained.is_empty() {
            shifts.push(Shift {
                crate_name: crate_name.clone(),
                lost,
                gained,
            });
        }
    }
    Verdict {
        candidate: candidate.to_string(),
        shifts,
        dropped,
    }
}

/// Re-ask the inverted tree about each flagged crate, keeping the shifts it
/// confirms and dropping the rest as forward-tree artifacts.
fn confirm(dir: &Path, candidate: &str, flagged: Vec<Shift>) -> Result<Vec<Shift>, Vec<String>> {
    let mut confirmed = Vec::new();
    for shift in flagged {
        let before = activations(dir, &shift.crate_name, None)?;
        let after = activations(dir, &shift.crate_name, Some(candidate))?;
        let lost: Vec<String> = before.difference(&after).cloned().collect();
        let gained: Vec<String> = after.difference(&before).cloned().collect();
        if !lost.is_empty() || !gained.is_empty() {
            confirmed.push(Shift {
                crate_name: shift.crate_name,
                lost,
                gained,
            });
        }
    }
    Ok(confirmed)
}

fn main() -> ! {
    let extra: Vec<String> = std::env::args().skip(1).collect();
    run(
        "exclusion-audit",
        move || {
            let root = repo_root()?;
            let makefile = read(&root.join("Makefile.toml"))?;
            let mut candidates = declared_candidates(&makefile)?;
            for name in extra {
                if !candidates.contains(&name) {
                    candidates.push(name);
                }
            }
            // An empty array is a finding, not a fault: it says no member
            // has passed this audit, which is the state the repo is in.
            if candidates.is_empty() {
                return Ok(Vec::new());
            }
            let full = feature_graph(&root, None)?;
            let mut verdicts = Vec::new();
            for candidate in &candidates {
                if !full.contains_key(candidate) {
                    return Err(vec![format!(
                        "{candidate} is not in the workspace graph; `--exclude` would reject it"
                    )]);
                }
                let excluded = feature_graph(&root, Some(candidate))?;
                let mut verdict = compare(candidate, &full, &excluded);
                verdict.shifts = confirm(&root, candidate, verdict.shifts)?;
                verdicts.push(verdict);
            }
            Ok(verdicts)
        },
        |verdicts: Vec<Verdict>| {
            if verdicts.is_empty() {
                println!(
                    "  {DECLARED_ARRAY} is empty: no member is excluded from the packages \
                     phase's build. Name a candidate to audit one."
                );
                return;
            }
            let mut hazards = 0;
            for verdict in &verdicts {
                if verdict.shifts.is_empty() {
                    println!(
                        "  {:<20} safe: every surviving crate resolves identically, and {} crate(s) \
                         leave the graph with it",
                        verdict.candidate, verdict.dropped
                    );
                    continue;
                }
                hazards += 1;
                println!("  {:<20} UNSAFE:", verdict.candidate);
                for shift in &verdict.shifts {
                    println!("    {}:", shift.crate_name);
                    if !shift.lost.is_empty() {
                        println!("      loses:  {:?}", shift.lost);
                    }
                    if !shift.gained.is_empty() {
                        println!("      gains:  {:?}", shift.gained);
                    }
                }
            }
            println!();
            if hazards > 0 {
                println!(
                    "{hazards} candidate(s) would re-unify features, compiling a second variant of \
                     every crate beneath them. That costs more than the test binaries the \
                     exclusion skips."
                );
                std::process::exit(1);
            }
            println!("every audited exclusion leaves feature unification unchanged.");
        },
    )
}

#[cfg(test)]
mod declared_candidates {
    use super::*;

    /// HYPOTHESIS: the audit reads the array the Makefile actually declares,
    /// so a member added there is audited without this tool being touched.
    /// Falsified if the parse misses an entry or keeps the delimiters.
    #[test]
    fn every_declared_member_is_read() {
        let makefile = "noise\nbuild_excludable=(alpha beta)\nmore noise\n";
        assert_eq!(
            declared_candidates(makefile).expect("the array parses"),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    /// HYPOTHESIS: an empty array parses as no candidates rather than as one
    /// empty name, which `--exclude ""` would reject with a confusing error.
    /// Falsified if the split yields a blank entry.
    #[test]
    fn an_empty_array_yields_no_candidates() {
        assert!(declared_candidates("build_excludable=()\n")
            .expect("an empty array parses")
            .is_empty());
    }

    /// HYPOTHESIS: a renamed or deleted array is an error the reader sees,
    /// never a silent pass. Falsified if the audit reports success against a
    /// Makefile that declares nothing.
    #[test]
    fn a_missing_array_is_an_error() {
        assert!(declared_candidates("live_packages=(a b)\n").is_err());
        assert!(declared_candidates("build_excludable=(a b\n").is_err());
    }
}

#[cfg(test)]
mod resolve {
    use super::*;

    const LISTING: &str = "\
zingolib v5.0.0 (/repo/zingolib)
zingolib feature \"nym\"
zingolib feature \"perspective\" (*)
zingo-netutils v5.0.1 (/repo/zingo-netutils)
zingo-netutils feature \"socks5-transmit\"
zingo-memo v0.1.1 (/repo/zingo-memo)
[dev-dependencies]
";

    /// HYPOTHESIS: a crate carrying no feature line is still present in the
    /// graph, so it reads as unchanged rather than as dropped. Falsified if
    /// only feature-bearing crates are recorded, which would report every
    /// featureless crate as a saving.
    #[test]
    fn a_featureless_crate_is_present_and_empty() {
        let graph = resolve(LISTING);
        assert_eq!(graph.get("zingo-memo"), Some(&BTreeSet::new()));
    }

    /// HYPOTHESIS: cargo's `(*)` deduplication marker is not read as part of
    /// the feature name, so a repeated activation matches its first
    /// appearance. Falsified if the marker survives into the set.
    #[test]
    fn the_deduplication_marker_is_not_a_feature_name() {
        let graph = resolve(LISTING);
        let zingolib = graph.get("zingolib").expect("zingolib is in the graph");
        assert!(zingolib.contains("perspective"), "{zingolib:?}");
        assert!(zingolib.contains("nym"), "{zingolib:?}");
        assert_eq!(zingolib.len(), 2, "{zingolib:?}");
    }

    /// HYPOTHESIS: the graph reaches crates that are not workspace members,
    /// because the crate this audit most needs to watch is one of them:
    /// zingo-netutils is its own workspace root. Falsified if a
    /// members-only view is what gets compared.
    #[test]
    fn a_non_member_crate_is_audited() {
        let graph = resolve(LISTING);
        assert_eq!(
            graph.get("zingo-netutils").map(BTreeSet::len),
            Some(1),
            "a crate outside the workspace still shares the compile"
        );
    }

    /// HYPOTHESIS: a `[dev-dependencies]` header is not a crate, so it never
    /// enters the graph as one. Falsified if the section marker is counted.
    #[test]
    fn a_section_header_is_not_a_crate() {
        assert!(!resolve(LISTING).contains_key("[dev-dependencies]"));
    }
}

#[cfg(test)]
mod compare {
    use super::*;

    fn graph(entries: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<String>> {
        entries
            .iter()
            .map(|(name, features)| {
                (
                    name.to_string(),
                    features.iter().map(|f| f.to_string()).collect(),
                )
            })
            .collect()
    }

    /// HYPOTHESIS: a crate that vanishes with the excluded member is the
    /// saving and not a hazard, so it is counted rather than reported.
    /// Falsified if a dropped private dependency reads as a feature loss.
    #[test]
    fn a_vanished_crate_is_a_saving_not_a_shift() {
        let full = graph(&[("shared", &["a"]), ("private", &["b"])]);
        let excluded = graph(&[("shared", &["a"])]);
        let verdict = compare("candidate", &full, &excluded);
        assert!(verdict.shifts.is_empty(), "no surviving crate changed");
        assert_eq!(verdict.dropped, 1);
    }

    /// HYPOTHESIS: a surviving crate losing a feature is the hazard the
    /// audit exists for, and the feature is named. Falsified if the loss is
    /// silent or unattributed.
    #[test]
    fn a_surviving_crate_losing_a_feature_is_reported() {
        let full = graph(&[("shared", &["nym", "default"])]);
        let excluded = graph(&[("shared", &["default"])]);
        let verdict = compare("candidate", &full, &excluded);
        assert_eq!(verdict.shifts.len(), 1);
        assert_eq!(verdict.shifts[0].crate_name, "shared");
        assert_eq!(verdict.shifts[0].lost, vec!["nym".to_string()]);
        assert!(verdict.shifts[0].gained.is_empty());
    }
}
