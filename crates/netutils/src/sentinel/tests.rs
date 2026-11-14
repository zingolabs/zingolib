use super::*;

/// HYPOTHESIS: any completed round trip proves the exit, whatever the
/// destination said, and every failure to complete one is silence.
/// Falsified if an empty read counts as proof or a reply does not.
#[test]
fn only_a_completed_round_trip_proves_the_exit() {
    assert_eq!(
        evidence_of(Some(63), 900),
        ExitEvidence::Answered { millis: 900 }
    );
    assert!(evidence_of(Some(63), 900).proves_the_exit());
    assert_eq!(evidence_of(Some(0), 3500), ExitEvidence::Silent);
    assert_eq!(evidence_of(None, 3500), ExitEvidence::Silent);
    assert!(!evidence_of(None, 3500).proves_the_exit());
}

/// HYPOTHESIS: the query is an ordinary DNS lookup — length-prefixed,
/// one question, naming a constant the wallet has no interest in — so
/// the exit and the resolver both see traffic they see constantly.
/// Falsified if the framing or the question section drifts.
#[test]
fn the_query_is_an_ordinary_dns_lookup() {
    let query = sentinel_query();
    let framed_len = u16::from_be_bytes([query[0], query[1]]) as usize;
    assert_eq!(framed_len, query.len() - 2, "the length prefix frames it");
    assert_eq!(&query[2..4], &SENTINEL_QUERY_ID, "the id opens the body");
    assert_eq!(&query[6..8], &[0x00, 0x01], "exactly one question");
    let name: Vec<u8> = query[14..].to_vec();
    assert_eq!(name[0], 7, "the first label is `example`");
    assert_eq!(&name[1..8], b"example");
    assert_eq!(name[8], 3, "the second label is `com`");
    assert_eq!(&name[9..12], b"com");
    assert_eq!(
        &name[12..],
        &[0x00, 0x00, 0x01, 0x00, 0x01],
        "an A record in IN"
    );
}
