use crate::{run_streaming, stdout_of_owned};

const GH: &str = "gh";
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
mod tests {
    use super::*;

    #[test]
    fn an_empty_listing_names_no_run() {
        assert_eq!(live_run_ids("", None), Ok(Vec::new()));
        assert_eq!(live_run_ids("\n\n", Some(7)), Ok(Vec::new()));
    }

    #[test]
    fn every_listed_run_is_named_in_order() {
        assert_eq!(
            live_run_ids("38850269996\n38851039696\n", None),
            Ok(vec![38850269996, 38851039696])
        );
    }

    #[test]
    fn the_cancelling_run_is_left_out() {
        assert_eq!(
            live_run_ids("1\n2\n3\n", Some(2)),
            Ok(vec![1, 3]),
            "the run that does the cancelling must not cancel itself"
        );
    }

    #[test]
    fn a_line_that_is_not_a_run_id_is_a_diagnostic() {
        let error = live_run_ids("12\nnot-a-run\n", None).expect_err("the listing holds a word");
        assert_eq!(error.len(), 1);
        assert!(error[0].contains("not-a-run"), "{error:?}");
    }

    #[test]
    fn a_run_id_is_trimmed_and_numeric() {
        assert_eq!(run_id(" 42\n"), Ok(42));
        assert!(run_id("later").is_err());
    }

    #[test]
    fn the_listing_asks_for_live_pull_request_runs_of_the_branch() {
        let args = list_args("fix/thing");
        assert_eq!(args[..4], ["run", "list", "--branch", "fix/thing"]);
        assert!(args.contains(&"pull_request".to_string()));
        assert!(args.contains(&LIVE_RUN_IDS.to_string()));
    }
}
