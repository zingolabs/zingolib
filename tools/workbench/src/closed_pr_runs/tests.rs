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
