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
