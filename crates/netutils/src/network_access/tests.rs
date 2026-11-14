use super::*;

fn uri(raw: &str) -> Uri {
    raw.parse().expect("a static test uri")
}

#[tokio::test]
async fn no_uri_cannot_go_online() {
    let mut session = NetworkAccess::offline(None);
    assert!(matches!(
        session.go_online().await,
        Err(GetClientError::NoIndexerUri)
    ));
    assert!(!session.is_online());
    assert_eq!(session.indexer_uri(), None);
    assert!(session.connected().is_none());
}

// The lazy `indexer` still needs a runtime to live in, hence tokio tests.
#[tokio::test]
async fn connect_to_lazy_sets_the_uri_and_the_indexer_together() {
    let mut session = NetworkAccess::offline(Some(uri("https://first.test:443")));
    session
        .connect_to_lazy(uri("https://second.test:443"))
        .expect("a lazy connect to an https URI dials nothing");
    let (held, _indexer) = session.connected().expect("online");
    assert_eq!(held, &uri("https://second.test:443"));
    assert!(session.is_online());
}

#[tokio::test]
async fn go_offline_keeps_the_uri() {
    let mut session = NetworkAccess::offline(None);
    session
        .connect_to_lazy(uri("https://named.test:443"))
        .expect("a lazy connect to an https URI dials nothing");
    session.go_offline();
    assert!(!session.is_online());
    assert!(session.indexer().is_none());
    assert_eq!(session.indexer_uri(), Some(&uri("https://named.test:443")));
}

#[tokio::test]
async fn a_failed_connect_changes_nothing() {
    let mut session = NetworkAccess::offline(None);
    session
        .connect_to_lazy(uri("https://named.test:443"))
        .expect("a lazy connect to an https URI dials nothing");

    assert!(matches!(
        session.connect_to_lazy(uri("ftp://bad.test:21")),
        Err(GetClientError::InvalidScheme)
    ));
    assert!(matches!(
        session.connect_to(uri("ftp://bad.test:21")).await,
        Err(GetClientError::InvalidScheme)
    ));
    assert!(session.is_online(), "the previous `indexer` survives");
    assert_eq!(session.indexer_uri(), Some(&uri("https://named.test:443")));
}
