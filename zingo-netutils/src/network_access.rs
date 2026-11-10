use http::Uri;

use crate::{GetClientError, GrpcIndexer};

/// ```
/// use zingo_netutils::{GetClientError, NetworkAccess};
///
/// // The lazy `indexer` still needs a runtime to live in.
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a current-thread runtime builds in a doc-test").block_on(async {
/// let named: http::Uri = "https://indexer.test:443".parse().expect("the literal is a well-formed https URI");
/// let mut session = NetworkAccess::offline(Some(named.clone()));
/// assert!(!session.is_online());
/// assert_eq!(session.indexer_uri(), Some(&named));
///
/// // The lazy connect builds `indexer` without dialing.
/// session.connect_to_lazy(named.clone()).expect("a lazy connect to an https URI dials nothing");
/// assert!(session.is_online());
///
/// session.go_offline();
/// assert!(!session.is_online());
/// assert_eq!(session.indexer_uri(), Some(&named), "offline keeps the URI");
///
/// // Nothing to attach to refuses typed.
/// let mut bare = NetworkAccess::offline(None);
/// assert!(matches!(
///     bare.connect_to_lazy("ftp://indexer.test:21".parse().expect("the literal is a well-formed URI")),
///     Err(GetClientError::InvalidScheme)
/// ));
/// assert!(!bare.is_online());
/// assert_eq!(bare.indexer_uri(), None, "a failed connect changes nothing");
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct NetworkAccess {
    indexer_uri: Option<Uri>,
    indexer: Option<GrpcIndexer>,
}

impl NetworkAccess {
    pub fn offline(indexer_uri: Option<Uri>) -> Self {
        Self {
            indexer_uri,
            indexer: None,
        }
    }

    pub async fn online(indexer_uri: Uri) -> Result<Self, GetClientError> {
        let mut session = Self::offline(Some(indexer_uri));
        session.go_online().await?;
        Ok(session)
    }

    pub fn is_online(&self) -> bool {
        self.indexer.is_some()
    }

    pub fn indexer_uri(&self) -> Option<&Uri> {
        self.indexer_uri.as_ref()
    }

    pub fn indexer(&self) -> Option<&GrpcIndexer> {
        self.indexer.as_ref()
    }

    pub fn connected(&self) -> Option<(&Uri, &GrpcIndexer)> {
        self.indexer_uri.as_ref().zip(self.indexer.as_ref())
    }

    pub async fn go_online(&mut self) -> Result<Uri, GetClientError> {
        let uri = self
            .indexer_uri
            .clone()
            .ok_or(GetClientError::NoIndexerUri)?;
        self.indexer = Some(GrpcIndexer::new(uri.clone()).await?);
        Ok(uri)
    }

    pub fn go_offline(&mut self) -> bool {
        self.indexer.take().is_some()
    }

    pub async fn connect_to(&mut self, uri: Uri) -> Result<(), GetClientError> {
        let indexer = GrpcIndexer::new(uri.clone()).await?;
        self.indexer = Some(indexer);
        self.indexer_uri = Some(uri);
        Ok(())
    }

    pub fn connect_to_lazy(&mut self, uri: Uri) -> Result<(), GetClientError> {
        let indexer = GrpcIndexer::new_lazy(uri.clone())?;
        self.indexer = Some(indexer);
        self.indexer_uri = Some(uri);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
}
