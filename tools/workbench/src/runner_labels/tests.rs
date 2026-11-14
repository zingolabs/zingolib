use super::*;
use crate::{repo_root, verdict};

const WORKFLOW: &str = "x.yaml";
const VARIABLE: &str = "CI_LINUX_RUNNER";
const LABEL: &str = "warp-ubuntu-2404-x64-16x";
const STANDARD_LABEL: &str = "ubuntu-24.04";

fn fallback_expression() -> String {
    format!("{EXPRESSION_OPEN} vars.{VARIABLE} {FALLBACK_OPERATOR} '{LABEL}' {EXPRESSION_CLOSE}")
}

fn site(value: &str) -> RunnerSite {
    RunnerSite {
        workflow: WORKFLOW.to_string(),
        line: 1,
        value: value.to_string(),
    }
}

#[test]
fn a_variable_with_a_quoted_fallback_parses() {
    let site = site(&fallback_expression());
    assert_eq!(site.variable_fallback(), Some((VARIABLE, LABEL)));
    assert!(site.names_paid_label());
}

#[test]
fn a_bare_label_has_no_override() {
    assert_eq!(site(LABEL).variable_fallback(), None);
    assert_eq!(site(STANDARD_LABEL).variable_fallback(), None);
    assert!(!site(STANDARD_LABEL).names_paid_label());
}

#[test]
fn runs_on_lines_and_runner_input_defaults_are_sites() {
    let text = format!(
        "on:\n  workflow_call:\n    inputs:\n      abis:\n        default: all\n      # the runner\n      runner:\n        type: string\n        default: {LABEL}\njobs:\n  build:\n    runs-on: {STANDARD_LABEL}\n"
    );
    let sites = runner_sites(WORKFLOW, &text);
    let found: Vec<(usize, &str)> = sites
        .iter()
        .map(|site| (site.line, site.value.as_str()))
        .collect();
    assert_eq!(found, vec![(9, LABEL), (12, STANDARD_LABEL)]);
}

#[test]
fn every_paid_runner_site_falls_back_from_a_repository_variable() -> Result<(), Vec<String>> {
    verdict(paid_sites_without_override(&repo_root()?)?)
}
