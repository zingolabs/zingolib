use super::parse_arguments;

fn parse(args: &[&str]) -> Result<super::Arguments, super::ArgumentsError> {
    parse_arguments(args.iter().map(ToString::to_string))
}

/// HYPOTHESIS: each grammar refusal is a distinct typed variant, so a
/// wrapper matches the refusal instead of parsing prose. Falsified if a
/// refusal loses its variant or its payload.
#[test]
fn refusals_are_typed_variants() {
    assert!(matches!(
        parse(&["--exit"]).unwrap_err(),
        super::ArgumentsError::MissingExitIdentity
    ));
    assert!(matches!(
        parse(&["--bogus"]).unwrap_err(),
        super::ArgumentsError::UnknownArgument { argument } if argument == "--bogus"
    ));
}

/// HYPOTHESIS: the retired `--responsiveness` flag refuses as an unknown
/// argument, so a version-skewed older parent is diagnosed loudly rather
/// than silently accepted.
#[test]
fn the_retired_class_flag_refuses_as_unknown() {
    assert!(matches!(
        parse(&["--responsiveness", "prioritise-speed"]).unwrap_err(),
        super::ArgumentsError::UnknownArgument { argument }
            if argument == "--responsiveness"
    ));
}

/// HYPOTHESIS: the clutch grammar accepts repeated `--exit` pairs and
/// refuses anything else, so a malformed spawn fails loudly instead of
/// silently racing a short clutch.
#[test]
fn the_clutch_grammar_is_pairs_only() {
    assert_eq!(
        parse(&[]).expect("no arguments, no clutch").clutch,
        Vec::<String>::new()
    );
    assert_eq!(
        parse(&["--exit", "id-a", "--exit", "id-b"])
            .expect("two well-formed pairs")
            .clutch,
        vec!["id-a".to_string(), "id-b".to_string()]
    );
    assert!(parse(&["--exit"]).is_err(), "a dangling flag refuses");
    assert!(
        parse(&["--unknown"]).is_err(),
        "an unknown argument refuses"
    );
}

/// HYPOTHESIS: `--discover` is a standalone flag that composes with the
/// rest of the grammar, and is off unless named.
#[test]
fn the_discover_flag_is_off_unless_named() {
    assert!(!parse(&[]).expect("bare invocation").discover);
    assert!(parse(&["--discover"]).expect("the flag alone").discover);
}
