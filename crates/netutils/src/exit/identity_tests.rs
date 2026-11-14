use super::{BlankExitNodeId, ExitNodeId};

/// HYPOTHESIS: minting trims the candidate and refuses a blank, so the
/// spawned and hosted paths cannot mint unequal spellings of one exit or
/// an empty identity. Falsified if whitespace survives or a blank mints.
#[test]
fn minting_trims_and_refuses_a_blank() {
    assert_eq!(
        ExitNodeId::parse(" exit-alpha ").expect("non-blank mints"),
        ExitNodeId::parse("exit-alpha").expect("non-blank mints"),
    );
    assert_eq!(ExitNodeId::parse(""), Err(BlankExitNodeId));
    assert_eq!(ExitNodeId::parse("   "), Err(BlankExitNodeId));
    assert_eq!(
        ExitNodeId::try_from(String::from("  ")),
        Err(BlankExitNodeId)
    );
}
