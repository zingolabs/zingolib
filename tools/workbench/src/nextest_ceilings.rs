use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

pub const NEXTEST_CONFIG: &str = ".config/nextest.toml";

pub const PROFILES: [&str; 2] = ["default", "ci"];

const DIRECTORIES_WITHOUT_SOURCES: [&str; 4] = [".git", "target", "build", "node_modules"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FiltersetTerm {
    Binary(String),
    Package(String),
    Test(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub terms: Vec<FiltersetTerm>,
    pub terminate_after: Option<i64>,
}

pub fn terms(filter: &str) -> Result<Vec<FiltersetTerm>, String> {
    filter
        .split('|')
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(term)
        .collect()
}

fn term(text: &str) -> Result<FiltersetTerm, String> {
    let (predicate, rest) = text
        .split_once('(')
        .ok_or_else(|| format!("{text}: a filterset term is a predicate over one argument"))?;
    let argument = rest
        .strip_suffix(')')
        .ok_or_else(|| format!("{text}: the predicate's argument is unclosed"))?
        .to_string();
    match predicate {
        "binary" => Ok(FiltersetTerm::Binary(argument)),
        "package" => Ok(FiltersetTerm::Package(argument)),
        "test" => Ok(FiltersetTerm::Test(argument)),
        other => Err(format!("{text}: the ceilings use no {other} predicate")),
    }
}

pub fn overrides(config: &str, profile: &str) -> Result<Vec<Override>, String> {
    let document: DocumentMut = config
        .parse()
        .map_err(|error| format!("{NEXTEST_CONFIG}: {error}"))?;
    let overrides = document
        .get("profile")
        .and_then(|profiles| profiles.get(profile))
        .and_then(|profile| profile.get("overrides"))
        .and_then(Item::as_array_of_tables)
        .ok_or_else(|| format!("profile.{profile}.overrides is not an array of tables"))?;
    overrides
        .iter()
        .map(|table| {
            let filter = table
                .get("filter")
                .and_then(Item::as_str)
                .ok_or_else(|| format!("an override of profile.{profile} has no string filter"))?;
            let terminate_after = table
                .get("slow-timeout")
                .and_then(|timeout| timeout.get("terminate-after"))
                .and_then(Item::as_integer);
            Ok(Override {
                terms: terms(filter)?,
                terminate_after,
            })
        })
        .collect()
}

fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

pub fn function_names(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once("fn ")?;
            let name: String = rest
                .chars()
                .take_while(|&character| is_identifier_character(character))
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

pub fn names_a_defined_test(test_term: &str, function_names: &[String]) -> bool {
    function_names.iter().any(|name| name.contains(test_term))
}

/// - Reads every directory under `root` except the ones that hold no sources.
pub fn rust_sources(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut sources = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?;
        for entry in entries {
            let path = entry
                .map_err(|error| format!("{}: {error}", directory.display()))?
                .path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if path.is_dir() {
                if !DIRECTORIES_WITHOUT_SOURCES.contains(&name) {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(path);
            }
        }
    }
    sources.sort();
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
[profile.default]
fail-fast = false

[[profile.default.overrides]]
filter = 'binary(testnet)'
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

    #[test]
    fn a_filter_splits_into_its_predicates_over_lines_and_spaces() -> Result<(), String> {
        assert_eq!(
            terms("\ntest(a)\n| test(b_c)\n")?,
            vec![
                FiltersetTerm::Test("a".to_string()),
                FiltersetTerm::Test("b_c".to_string()),
            ]
        );
        assert_eq!(
            terms("package(libtonode-tests) | package(zingo-cli)")?,
            vec![
                FiltersetTerm::Package("libtonode-tests".to_string()),
                FiltersetTerm::Package("zingo-cli".to_string()),
            ]
        );
        assert_eq!(
            terms("binary(testnet)")?,
            vec![FiltersetTerm::Binary("testnet".to_string())]
        );
        Ok(())
    }

    #[test]
    fn a_predicate_the_ceilings_do_not_use_is_refused() {
        assert!(terms("kind(test)").is_err());
        assert!(terms("test(unclosed").is_err());
        assert!(terms("bare_name").is_err());
    }

    #[test]
    fn overrides_carry_their_terms_and_their_ceiling() -> Result<(), String> {
        let overrides = overrides(CONFIG, "default")?;
        assert_eq!(overrides.len(), 3);
        assert_eq!(overrides[0].terminate_after, Some(60));
        assert_eq!(overrides[1].terminate_after, None);
        assert_eq!(overrides[2].terminate_after, Some(20));
        assert_eq!(
            overrides[2].terms,
            vec![
                FiltersetTerm::Test("send_mined_ironwood_to_ironwood".to_string()),
                FiltersetTerm::Test("drain_chunks_a_fragmented_wallet".to_string()),
            ]
        );
        assert!(super::overrides(CONFIG, "ci").is_err());
        Ok(())
    }

    #[test]
    fn function_names_are_read_from_every_fn_line() {
        let source = "#[tokio::test]\nasync fn send_mined_ironwood_to_ironwood() {\n}\n\
                      pub(super) fn helper<T>(value: T) {}\nlet closure = |x| x;\n";
        assert_eq!(
            function_names(source),
            vec!["send_mined_ironwood_to_ironwood", "helper"]
        );
    }

    #[test]
    fn a_test_term_matches_a_function_name_by_substring_as_nextest_does() {
        let names = vec!["send_shield_cycle_on_the_mock_chain".to_string()];
        assert!(names_a_defined_test("send_shield_cycle", &names));
        assert!(!names_a_defined_test(
            "drain_chunks_a_fragmented_wallet",
            &names
        ));
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn defined_function_names() -> Result<Vec<String>, String> {
        let mut names = Vec::new();
        for source in rust_sources(&repo_root())? {
            let text = std::fs::read_to_string(&source)
                .map_err(|error| format!("{}: {error}", source.display()))?;
            names.extend(function_names(&text));
        }
        Ok(names)
    }

    #[test]
    fn every_ceiling_term_names_a_defined_test() -> Result<(), String> {
        let config = std::fs::read_to_string(repo_root().join(NEXTEST_CONFIG))
            .map_err(|error| format!("{NEXTEST_CONFIG}: {error}"))?;
        let names = defined_function_names()?;
        for profile in PROFILES {
            for ceiling in overrides(&config, profile)? {
                for term in &ceiling.terms {
                    if let FiltersetTerm::Test(test_term) = term {
                        assert!(
                            names_a_defined_test(test_term, &names),
                            "profile.{profile} names test({test_term}), and no function name contains it"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn the_ci_profile_carries_the_overrides_of_the_default_profile() -> Result<(), String> {
        let config = std::fs::read_to_string(repo_root().join(NEXTEST_CONFIG))
            .map_err(|error| format!("{NEXTEST_CONFIG}: {error}"))?;
        let [default, ci] = PROFILES.map(|profile| overrides(&config, profile));
        assert_eq!(default?, ci?);
        Ok(())
    }
}
