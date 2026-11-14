use super::*;

/// HYPOTHESIS: the audit reads the array the Makefile actually declares,
/// so a member added there is audited without this tool being touched.
/// Falsified if the parse misses an entry or keeps the delimiters.
#[test]
fn every_declared_member_is_read() -> Result<(), Vec<String>> {
    let makefile = "noise\nbuild_excludable=(alpha beta)\nmore noise\n";
    assert_eq!(
        declared_candidates(makefile)?,
        vec!["alpha".to_string(), "beta".to_string()]
    );
    Ok(())
}

/// HYPOTHESIS: an empty array parses as no candidates rather than as one
/// empty name, which `--exclude ""` would reject with a confusing error.
/// Falsified if the split yields a blank entry.
#[test]
fn an_empty_array_yields_no_candidates() -> Result<(), Vec<String>> {
    assert!(declared_candidates("build_excludable=()\n")?.is_empty());
    Ok(())
}

/// HYPOTHESIS: a renamed or deleted array is an error the reader sees,
/// never a silent pass. Falsified if the audit reports success against a
/// Makefile that declares nothing.
#[test]
fn a_missing_array_is_an_error() {
    assert!(declared_candidates("live_packages=(a b)\n").is_err());
    assert!(declared_candidates("build_excludable=(a b\n").is_err());
}
