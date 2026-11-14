use super::*;

const LISTING: &str = "\
zingolib v5.0.0 (/repo/crates/zingolib)
zingolib feature \"nym\"
zingolib feature \"perspective\" (*)
zl-netutils v5.0.1 (/repo/crates/netutils)
zl-netutils feature \"socks5-transmit\"
zl-memo v0.1.1 (/repo/crates/memo)
[dev-dependencies]
";

/// HYPOTHESIS: a crate carrying no feature line is still present in the
/// graph, so it reads as unchanged rather than as dropped. Falsified if
/// only feature-bearing crates are recorded, which would report every
/// featureless crate as a saving.
#[test]
fn a_featureless_crate_is_present_and_empty() {
    let graph = resolve(LISTING);
    assert_eq!(graph.get("zl-memo"), Some(&BTreeSet::new()));
}

/// HYPOTHESIS: cargo's `(*)` deduplication marker is not read as part of
/// the feature name, so a repeated activation matches its first
/// appearance. Falsified if the marker survives into the set.
#[test]
fn the_deduplication_marker_is_not_a_feature_name() -> Result<(), Vec<String>> {
    let graph = resolve(LISTING);
    let zingolib = workbench::required(graph.get("zingolib"), "zingolib in the graph")?;
    assert!(zingolib.contains("perspective"), "{zingolib:?}");
    assert!(zingolib.contains("nym"), "{zingolib:?}");
    assert_eq!(zingolib.len(), 2, "{zingolib:?}");
    Ok(())
}

#[test]
fn a_non_member_crate_is_audited() {
    let graph = resolve(LISTING);
    assert_eq!(
        graph.get("zl-netutils").map(BTreeSet::len),
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
