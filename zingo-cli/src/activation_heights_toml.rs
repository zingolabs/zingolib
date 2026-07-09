//! Parsing of the regtest `--activation-heights` TOML.
//!
//! The schema is zcash-devtool's `data.rs::ActivationHeights`, the shared
//! contract for wallet binaries driven by the `zcash_local_net` harness:
//! one optional `<upgrade> = <height>` line per network upgrade. A missing
//! key means the upgrade never activates. An unknown key is a hard error,
//! which is also how a schedule from a newer chain (e.g. one activating an
//! upgrade this build does not know) is refused rather than silently
//! truncated. `nu7` is excluded from the schema to match the devtool,
//! which gates that key behind its `zcash_unstable` build.

use zingo_common_components::protocol::ActivationHeights;

/// The TOML document. Field names are the wire schema; do not rename
/// without coordinating with the harness and the devtool.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivationHeightsToml {
    overwinter: Option<u32>,
    sapling: Option<u32>,
    blossom: Option<u32>,
    heartwood: Option<u32>,
    canopy: Option<u32>,
    nu5: Option<u32>,
    nu6: Option<u32>,
    nu6_1: Option<u32>,
    nu6_2: Option<u32>,
}

/// Parses the contents of an `--activation-heights` file into the
/// schedule `ChainType::Regtest` carries.
///
/// A valid schedule is prefix-contiguous and non-decreasing: an active
/// upgrade requires its predecessor active at an equal or lower height.
/// The check runs here so bad CLI input surfaces as an error message —
/// the `ActivationHeights` builder enforces the same invariant by
/// panicking.
pub(crate) fn parse(toml_text: &str) -> Result<ActivationHeights, String> {
    let parsed: ActivationHeightsToml =
        toml::from_str(toml_text).map_err(|e| format!("invalid --activation-heights TOML: {e}"))?;
    let schedule = [
        ("overwinter", parsed.overwinter),
        ("sapling", parsed.sapling),
        ("blossom", parsed.blossom),
        ("heartwood", parsed.heartwood),
        ("canopy", parsed.canopy),
        ("nu5", parsed.nu5),
        ("nu6", parsed.nu6),
        ("nu6_1", parsed.nu6_1),
        ("nu6_2", parsed.nu6_2),
    ];
    for pair in schedule.windows(2) {
        let [(earlier_name, earlier), (later_name, later)] = pair else {
            unreachable!("windows(2) yields pairs");
        };
        if let Some(later_height) = later
            && !earlier.is_some_and(|earlier_height| earlier_height <= *later_height)
        {
            return Err(format!(
                "invalid --activation-heights schedule: {later_name} = {later_height} requires \
                 {earlier_name} active at an equal or lower height (got {earlier:?})"
            ));
        }
    }
    Ok(ActivationHeights::builder()
        .set_overwinter(parsed.overwinter)
        .set_sapling(parsed.sapling)
        .set_blossom(parsed.blossom)
        .set_heartwood(parsed.heartwood)
        .set_canopy(parsed.canopy)
        .set_nu5(parsed.nu5)
        .set_nu6(parsed.nu6)
        .set_nu6_1(parsed.nu6_1)
        .set_nu6_2(parsed.nu6_2)
        .build())
}

#[cfg(test)]
mod tests {
    use super::parse;

    /// The shape the harness writes for an NU6.2 regtest chain: pre-NU5
    /// upgrades at 1, NU5 through NU6.2 at 2, NU6.3 omitted.
    const HARNESS_NU6_2: &str = "\
overwinter = 1
sapling = 1
blossom = 1
heartwood = 1
canopy = 1
nu5 = 2
nu6 = 2
nu6_1 = 2
nu6_2 = 2
";

    #[test]
    fn parses_the_harness_nu6_2_shape() {
        let heights = parse(HARNESS_NU6_2).unwrap();
        assert_eq!(heights.overwinter(), Some(1));
        assert_eq!(heights.sapling(), Some(1));
        assert_eq!(heights.blossom(), Some(1));
        assert_eq!(heights.heartwood(), Some(1));
        assert_eq!(heights.canopy(), Some(1));
        assert_eq!(heights.nu5(), Some(2));
        assert_eq!(heights.nu6(), Some(2));
        assert_eq!(heights.nu6_1(), Some(2));
        assert_eq!(heights.nu6_2(), Some(2));
        assert_eq!(heights.nu7(), None);
    }

    #[test]
    fn omitted_trailing_keys_mean_never_activates() {
        let heights = parse("overwinter = 1\nsapling = 1\n").unwrap();
        assert_eq!(heights.overwinter(), Some(1));
        assert_eq!(heights.sapling(), Some(1));
        assert_eq!(heights.blossom(), None);
        assert_eq!(heights.nu5(), None);
        assert_eq!(heights.nu6_2(), None);
    }

    #[test]
    fn gap_in_the_schedule_is_a_clean_error() {
        let err = parse("sapling = 1\nnu5 = 2\n").unwrap_err();
        assert!(
            err.contains("sapling") || err.contains("overwinter"),
            "error should name the gap: {err}"
        );
    }

    #[test]
    fn decreasing_heights_are_a_clean_error() {
        let toml = "overwinter = 1\nsapling = 3\nblossom = 2\n";
        let err = parse(toml).unwrap_err();
        assert!(
            err.contains("blossom"),
            "error should name the offending upgrade: {err}"
        );
    }

    #[test]
    fn empty_document_is_a_schedule_with_no_activations() {
        let heights = parse("").unwrap();
        assert_eq!(heights.overwinter(), None);
        assert_eq!(heights.nu6_2(), None);
    }

    #[test]
    fn nu6_3_key_is_refused_at_nu6_2() {
        let err = parse("nu5 = 2\nnu6_3 = 2\n").unwrap_err();
        assert!(err.contains("nu6_3"), "error should name the key: {err}");
    }

    #[test]
    fn nu7_key_is_refused_like_the_devtool_release_schema() {
        let err = parse("nu7 = 10\n").unwrap_err();
        assert!(err.contains("nu7"), "error should name the key: {err}");
    }

    #[test]
    fn non_integer_height_is_refused() {
        assert!(parse("nu5 = \"two\"\n").is_err());
    }
}
