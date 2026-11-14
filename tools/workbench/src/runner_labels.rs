use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const WORKFLOWS_DIR: &str = ".github/workflows";

pub const PAID_LABEL_PREFIX: &str = "warp-";

const WORKFLOW_EXTENSIONS: [&str; 2] = ["yaml", "yml"];
const RUNS_ON_KEY: &str = "runs-on:";
const DEFAULT_KEY: &str = "default:";
const RUNNER_INPUT_KEY: &str = "runner:";
const COMMENT_MARK: char = '#';
const EXPRESSION_OPEN: &str = "${{";
const EXPRESSION_CLOSE: &str = "}}";
const VARS_CONTEXT: &str = "vars.";
const FALLBACK_OPERATOR: &str = "||";
const LABEL_QUOTE: char = '\'';

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunnerSite {
    pub workflow: String,
    pub line: usize,
    pub value: String,
}

impl RunnerSite {
    pub fn names_paid_label(&self) -> bool {
        self.value.contains(PAID_LABEL_PREFIX)
    }

    pub fn variable_fallback(&self) -> Option<(&str, &str)> {
        let expression = self
            .value
            .trim()
            .strip_prefix(EXPRESSION_OPEN)?
            .strip_suffix(EXPRESSION_CLOSE)?;
        let (variable, fallback) = expression.split_once(FALLBACK_OPERATOR)?;
        let variable = variable.trim().strip_prefix(VARS_CONTEXT)?;
        let label = fallback
            .trim()
            .strip_prefix(LABEL_QUOTE)?
            .strip_suffix(LABEL_QUOTE)?;
        let is_variable_name = !variable.is_empty()
            && variable
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        (is_variable_name && !label.is_empty()).then_some((variable, label))
    }

    pub fn location(&self) -> String {
        format!("{}:{}", self.workflow, self.line)
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_prose(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with(COMMENT_MARK)
}

fn parent_key<'a>(lines: &[&'a str], index: usize) -> Option<&'a str> {
    let indent = indent_of(lines[index]);
    lines[..index]
        .iter()
        .rev()
        .filter(|line| !is_prose(line))
        .find(|line| indent_of(line) < indent)
        .map(|line| line.trim())
}

pub fn runner_sites(workflow: &str, text: &str) -> Vec<RunnerSite> {
    let lines: Vec<&str> = text.lines().collect();
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !is_prose(line))
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            let value = if let Some(value) = trimmed.strip_prefix(RUNS_ON_KEY) {
                value
            } else if parent_key(&lines, index) == Some(RUNNER_INPUT_KEY) {
                trimmed.strip_prefix(DEFAULT_KEY)?
            } else {
                return None;
            };
            Some(RunnerSite {
                workflow: workflow.to_string(),
                line: index + 1,
                value: value.trim().to_string(),
            })
        })
        .collect()
}

pub fn workflow_files(root: &Path) -> Result<Vec<PathBuf>, Vec<String>> {
    let directory = root.join(WORKFLOWS_DIR);
    let mut files: Vec<PathBuf> = crate::files_under(&directory, &[])?
        .into_iter()
        .filter(|file| {
            file.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| WORKFLOW_EXTENSIONS.contains(&extension))
        })
        .collect();
    files.sort();
    Ok(files)
}

pub fn paid_sites_without_override(root: &Path) -> Result<Vec<String>, Vec<String>> {
    let mut diagnostics = Vec::new();
    let mut variables_by_label: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for file in workflow_files(root)? {
        let text = crate::read(&file)?;
        let workflow = crate::display_relative(root, &file);
        for site in runner_sites(&workflow, &text) {
            if !site.names_paid_label() {
                continue;
            }
            match site.variable_fallback() {
                Some((variable, label)) => {
                    variables_by_label
                        .entry(label.to_string())
                        .or_default()
                        .insert(variable.to_string());
                }
                None => diagnostics.push(format!(
                    "{} pins the paid runner {:?} with no repository-variable override",
                    site.location(),
                    site.value
                )),
            }
        }
    }
    for (label, variables) in variables_by_label {
        if variables.len() > 1 {
            let names: Vec<&str> = variables.iter().map(String::as_str).collect();
            diagnostics.push(format!(
                "the paid runner {label:?} is overridden by {} variables: {}",
                names.len(),
                names.join(crate::LIST_SEPARATOR)
            ));
        }
    }
    Ok(diagnostics)
}

#[cfg(test)]
mod tests;
