use super::*;

fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

#[test]
fn an_empty_command_line_sweeps_the_touched_crates_against_the_default_base(
) -> Result<(), Vec<String>> {
    assert_eq!(
        parse(&[])?,
        Request {
            all: false,
            base: workbench::DEFAULT_BASE.to_string(),
            crates: Vec::new(),
        }
    );
    Ok(())
}

#[test]
fn base_parses_in_both_spellings() -> Result<(), Vec<String>> {
    assert_eq!(parse(&words("--base main"))?.base, "main");
    assert_eq!(parse(&words("--base=main"))?.base, "main");
    Ok(())
}

#[test]
fn a_base_value_is_not_read_as_a_crate() -> Result<(), Vec<String>> {
    assert!(parse(&words("--base main"))?.crates.is_empty());
    assert_eq!(
        parse(&words("--base main zingolib-cli"))?.crates,
        vec!["zingolib-cli".to_string()]
    );
    Ok(())
}

#[test]
fn base_without_a_reference_is_an_error() {
    assert!(parse(&words("--base")).is_err());
}

#[test]
fn an_unknown_flag_is_an_error() {
    assert!(parse(&words("--every-feature")).is_err());
}

#[test]
fn all_selects_the_whole_workspace() -> Result<(), Vec<String>> {
    assert!(parse(&words("--all"))?.all);
    Ok(())
}

#[test]
fn a_lone_separator_passes_through_from_cargo_make() -> Result<(), Vec<String>> {
    assert_eq!(
        parse(&words("-- zingolib-cli"))?.crates,
        vec!["zingolib-cli".to_string()]
    );
    Ok(())
}
