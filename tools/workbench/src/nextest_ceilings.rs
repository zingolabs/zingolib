use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::test_placement::Function;

pub const NEXTEST_CONFIG: &str = ".config/nextest.toml";

pub const PROFILES: [&str; 2] = ["default", "ci"];

const DIRECTORIES_WITHOUT_SOURCES: [&str; 4] = [".git", "target", "build", "node_modules"];

/// How a `test()` predicate matches a test's nextest name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestMatcher {
    Contains(String),
    Equals(String),
    Regex(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FiltersetTerm {
    Binary(String),
    Package(String),
    Test(TestMatcher),
}

/// A nextest filterset expression over the predicates the ceilings use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filterset {
    Term(FiltersetTerm),
    Not(Box<Filterset>),
    And(Vec<Filterset>),
    Or(Vec<Filterset>),
}

impl Filterset {
    pub fn test_matchers(&self) -> Vec<&TestMatcher> {
        match self {
            Filterset::Term(FiltersetTerm::Test(matcher)) => vec![matcher],
            Filterset::Term(_) => vec![],
            Filterset::Not(inner) => inner.test_matchers(),
            Filterset::And(parts) | Filterset::Or(parts) => {
                parts.iter().flat_map(Filterset::test_matchers).collect()
            }
        }
    }
}

/// One `[[profile.<name>.overrides]]` table: its filterset and every other key as TOML text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub filter: Filterset,
    pub settings: BTreeMap<String, String>,
}

pub fn filterset(text: &str) -> Result<Filterset, String> {
    unimplemented!("the filterset {text:?}")
}

pub fn overrides(config: &str, profile: &str) -> Result<Vec<Override>, String> {
    unimplemented!(
        "the overrides of profile.{profile} in {} bytes of config",
        config.len()
    )
}

impl TestMatcher {
    pub fn matches(&self, nextest_name: &str) -> Result<bool, String> {
        unimplemented!("whether {self:?} matches {nextest_name}")
    }
}

/// - Returns an error for a matcher the ceilings cannot evaluate, such as a regex.
pub fn names_a_defined_test(matcher: &TestMatcher, functions: &[Function]) -> Result<bool, String> {
    unimplemented!(
        "whether {matcher:?} names one of {} harness tests",
        functions.len()
    )
}

/// - Reads every directory under `root` except the ones that hold no sources.
pub fn rust_sources(root: &Path) -> Result<Vec<PathBuf>, Vec<String>> {
    Ok(crate::files_under(root, &DIRECTORIES_WITHOUT_SOURCES)?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_placement;

    const CONFIG: &str = r#"
[profile.default]
fail-fast = false

[[profile.default.overrides]]
filter = 'binary(testnet)'
test-group = 'testnet'
slow-timeout = { period = "60s", terminate-after = 60, grace-period = "30s" }

[[profile.default.overrides]]
filter = 'package(libtonode-tests) | package(zingo-cli)'

[[profile.default.overrides]]
filter = '''
test(send_mined_ironwood_to_ironwood)
| test(drain_chunks_a_fragmented_wallet)
'''
slow-timeout = { period = "60s", terminate-after = 20, grace-period = "30s" }
"#;

    fn test(name: &str) -> Filterset {
        Filterset::Term(FiltersetTerm::Test(TestMatcher::Contains(name.to_string())))
    }

    #[test]
    fn a_filter_splits_into_its_predicates_over_lines_and_spaces() -> Result<(), String> {
        assert_eq!(
            filterset("\ntest(a)\n| test(b_c)\n")?,
            Filterset::Or(vec![test("a"), test("b_c")])
        );
        assert_eq!(
            filterset("package(libtonode-tests) | package(zingo-cli)")?,
            Filterset::Or(vec![
                Filterset::Term(FiltersetTerm::Package("libtonode-tests".to_string())),
                Filterset::Term(FiltersetTerm::Package("zingo-cli".to_string())),
            ])
        );
        assert_eq!(
            filterset("binary(testnet)")?,
            Filterset::Term(FiltersetTerm::Binary("testnet".to_string()))
        );
        Ok(())
    }

    #[test]
    fn and_binds_tighter_than_or_and_parentheses_and_not_group() -> Result<(), String> {
        assert_eq!(
            filterset("test(a) | test(b) & test(c)")?,
            Filterset::Or(vec![test("a"), Filterset::And(vec![test("b"), test("c")])])
        );
        assert_eq!(
            filterset("(test(a) | test(b)) & not test(c)")?,
            Filterset::And(vec![
                Filterset::Or(vec![test("a"), test("b")]),
                Filterset::Not(Box::new(test("c"))),
            ])
        );
        Ok(())
    }

    #[test]
    fn a_test_argument_carries_its_matcher() -> Result<(), String> {
        assert_eq!(
            filterset("test(=exact::name)")?,
            Filterset::Term(FiltersetTerm::Test(TestMatcher::Equals(
                "exact::name".to_string()
            )))
        );
        assert_eq!(
            filterset("test(~part)")?,
            Filterset::Term(FiltersetTerm::Test(TestMatcher::Contains(
                "part".to_string()
            )))
        );
        assert_eq!(
            filterset("test(/^a.*z$/)")?,
            Filterset::Term(FiltersetTerm::Test(TestMatcher::Regex(
                "^a.*z$".to_string()
            )))
        );
        Ok(())
    }

    #[test]
    fn a_predicate_the_ceilings_do_not_use_or_a_malformed_filter_is_refused() {
        assert!(filterset("kind(test)").is_err());
        assert!(filterset("test(unclosed").is_err());
        assert!(filterset("bare_name").is_err());
        assert!(filterset("test(a) |").is_err());
        assert!(filterset("(test(a)").is_err());
        assert!(filterset("").is_err());
    }

    #[test]
    fn the_test_matchers_of_a_filterset_are_collected_through_every_operator() -> Result<(), String>
    {
        let matchers = filterset("(test(a) | package(p)) & not test(=b)")?
            .test_matchers()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            matchers,
            [
                TestMatcher::Contains("a".to_string()),
                TestMatcher::Equals("b".to_string())
            ]
        );
        Ok(())
    }

    #[test]
    fn overrides_carry_their_filterset_and_every_other_key_as_text() -> Result<(), String> {
        let overrides = overrides(CONFIG, "default")?;
        assert_eq!(overrides.len(), 3);
        assert_eq!(
            overrides[0].settings,
            BTreeMap::from([
                ("test-group".to_string(), "'testnet'".to_string()),
                (
                    "slow-timeout".to_string(),
                    r#"{ period = "60s", terminate-after = 60, grace-period = "30s" }"#.to_string()
                ),
            ])
        );
        assert!(overrides[1].settings.is_empty());
        assert_eq!(
            overrides[2].filter,
            Filterset::Or(vec![
                test("send_mined_ironwood_to_ironwood"),
                test("drain_chunks_a_fragmented_wallet"),
            ])
        );
        assert_eq!(
            overrides[2]
                .settings
                .get("slow-timeout")
                .map(String::as_str),
            Some(r#"{ period = "60s", terminate-after = 20, grace-period = "30s" }"#)
        );
        assert!(super::overrides(CONFIG, "ci").is_err());
        Ok(())
    }

    #[test]
    fn a_matcher_is_judged_against_the_nextest_name_of_a_harness_test() -> Result<(), String> {
        let source = "fn send_shield_cycle_helper() {}\n\
                      mod moved_from_regtest {\n    #[test]\n    fn send_mined_ironwood_to_ironwood() {}\n}\n\
                      #[tokio::test]\nasync fn send_shield_cycle_on_the_mock_chain() {}\n";
        let functions = test_placement::functions(source);
        let contains = |name: &str| TestMatcher::Contains(name.to_string());
        let equals = |name: &str| TestMatcher::Equals(name.to_string());
        assert!(names_a_defined_test(
            &contains("send_shield_cycle"),
            &functions
        )?);
        assert!(names_a_defined_test(
            &contains("moved_from_regtest"),
            &functions
        )?);
        assert!(names_a_defined_test(
            &equals("moved_from_regtest::send_mined_ironwood_to_ironwood"),
            &functions
        )?);
        assert!(!names_a_defined_test(
            &equals("send_mined_ironwood_to_ironwood"),
            &functions
        )?);
        assert!(!names_a_defined_test(&contains("helper"), &functions)?);
        assert!(!names_a_defined_test(
            &contains("drain_chunks_a_fragmented_wallet"),
            &functions
        )?);
        assert!(names_a_defined_test(&TestMatcher::Regex("x".to_string()), &functions).is_err());
        Ok(())
    }

    fn defined_functions() -> Result<Vec<Function>, Vec<String>> {
        let mut functions = Vec::new();
        for source in rust_sources(&crate::repo_root()?)? {
            let text = std::fs::read_to_string(&source)
                .map_err(|error| vec![format!("{}: {error}", source.display())])?;
            functions.extend(test_placement::functions(&text));
        }
        Ok(functions)
    }

    fn committed_config() -> Result<String, Vec<String>> {
        std::fs::read_to_string(crate::repo_root()?.join(NEXTEST_CONFIG))
            .map_err(|error| vec![format!("{NEXTEST_CONFIG}: {error}")])
    }

    #[test]
    fn every_ceiling_matcher_names_a_defined_harness_test() -> Result<(), Vec<String>> {
        let config = committed_config()?;
        let functions = defined_functions()?;
        for profile in PROFILES {
            for ceiling in overrides(&config, profile).map_err(|error| vec![error])? {
                for matcher in ceiling.filter.test_matchers() {
                    assert!(
                        names_a_defined_test(matcher, &functions).map_err(|error| vec![error])?,
                        "profile.{profile} names {matcher:?}, and no harness test's nextest name matches it"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn the_ci_profile_carries_the_overrides_of_the_default_profile() -> Result<(), Vec<String>> {
        let config = committed_config()?;
        let [default, ci] =
            PROFILES.map(|profile| overrides(&config, profile).map_err(|error| vec![error]));
        assert_eq!(default?, ci?);
        Ok(())
    }
}
