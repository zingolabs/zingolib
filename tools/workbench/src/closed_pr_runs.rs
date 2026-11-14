use crate::{run_streaming, stdout_of_owned, GH};

const RUN: &str = "run";
const LIST: &str = "list";
const CANCEL: &str = "cancel";

/// `gh run list` returns the newest runs first, so the live ones of a
/// branch sit inside this many.
const LIST_LIMIT: &str = "100";

/// Every status but `completed` holds a runner or waits for one.
const LIVE_RUN_IDS: &str = r#".[] | select(.status != "completed") | .databaseId"#;

pub fn list_args(branch: &str) -> Vec<String> {
    [
        RUN,
        LIST,
        "--branch",
        branch,
        "--event",
        "pull_request",
        "--limit",
        LIST_LIMIT,
        "--json",
        "databaseId,status",
        "--jq",
        LIVE_RUN_IDS,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub fn run_id(text: &str) -> Result<u64, Vec<String>> {
    let text = text.trim();
    text.parse()
        .map_err(|e| vec![format!("run id {text:?} is not a number: {e}")])
}

pub fn live_run_ids(listing: &str, own_run: Option<u64>) -> Result<Vec<u64>, Vec<String>> {
    listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(run_id)
        .filter(|id| !matches!((id, own_run), (Ok(id), Some(own)) if *id == own))
        .collect()
}

/// - Runs `gh run list` for `branch` as a child process and waits for it.
/// - Runs `gh run cancel` once per live run other than `own_run`, each as a child process.
pub fn cancel_live_runs(branch: &str, own_run: Option<u64>) -> Result<Vec<u64>, Vec<String>> {
    let listing = stdout_of_owned(GH, &list_args(branch))?;
    let ids = live_run_ids(&listing, own_run)?;
    for id in &ids {
        let id = id.to_string();
        run_streaming(GH, &[RUN, CANCEL, &id], &[])?;
    }
    Ok(ids)
}

#[cfg(test)]
mod tests;
