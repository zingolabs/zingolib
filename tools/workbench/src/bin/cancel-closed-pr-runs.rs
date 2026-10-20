#![forbid(unsafe_code)]

use workbench::closed_pr_runs;
use workbench::run;

const PROG: &str = "cancel-closed-pr-runs";

/// The run id GitHub sets for the job that executes this binary.
const OWN_RUN_VARIABLE: &str = "GITHUB_RUN_ID";

fn main() {
    run(
        PROG,
        || {
            let branch = std::env::args()
                .nth(1)
                .ok_or_else(|| vec![format!("usage: {PROG} <head-branch>")])?;
            let own_run = std::env::var(OWN_RUN_VARIABLE)
                .ok()
                .map(|id| closed_pr_runs::run_id(&id))
                .transpose()?;
            closed_pr_runs::cancel_live_runs(&branch, own_run)
        },
        |cancelled| {
            for id in cancelled {
                println!("cancelled run {id}");
            }
        },
    )
}
