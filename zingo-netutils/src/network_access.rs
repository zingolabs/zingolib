//! The wallet's access to the network: where it points and whether it is
//! open. The Session Indexer (zingo-adrs zingolib/0063) is the one indexer a
//! session attaches to, held as a URI that outlives the connection.

use http::Uri;

use crate::{GetClientError, GrpcIndexer};

/// The wallet's access to the network: `indexer_uri`, the Session Indexer
/// URI the session attaches to, and `indexer`, the live `GrpcIndexer`
/// dialed from it while the session is online.
///
/// `indexer_uri` and `indexer` change only together, through this type's
/// methods, so a `Some` `indexer` was always dialed from the `indexer_uri`
/// held beside it. The presence of `indexer` is the online indicator:
/// [`Self::go_offline`] drops `indexer` and keeps `indexer_uri`, and
/// [`Self::go_online`] dials `indexer_uri` again. A consumer that holds no
/// `indexer_uri` (a chain without a Default Indexer, and none named) cannot
/// go online until [`Self::connect_to`] names one.
///
/// A failed dial changes nothing: whatever `indexer_uri` and `indexer` were
/// held before the call are held after it.
///
/// ```
/// use zingo_netutils::{GetClientError, NetworkAccess};
///
/// // The lazy `indexer` still needs a runtime to live in.
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let named: http::Uri = "https://indexer.test:443".parse().unwrap();
/// let mut session = NetworkAccess::offline(Some(named.clone()));
/// assert!(!session.is_online());
/// assert_eq!(session.indexer_uri(), Some(&named));
///
/// // The lazy connect builds `indexer` without dialing.
/// session.connect_to_lazy(named.clone()).unwrap();
/// assert!(session.is_online());
///
/// session.go_offline();
/// assert!(!session.is_online());
/// assert_eq!(session.indexer_uri(), Some(&named), "offline keeps the URI");
///
/// // Nothing to attach to refuses typed.
/// let mut bare = NetworkAccess::offline(None);
/// assert!(matches!(
///     bare.connect_to_lazy("ftp://indexer.test:21".parse().unwrap()),
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
    /// Holds `indexer_uri` without dialing it.
    pub fn offline(indexer_uri: Option<Uri>) -> Self {
        Self {
            indexer_uri,
            indexer: None,
        }
    }

    /// Holds `indexer_uri` and dials it.
    pub async fn online(indexer_uri: Uri) -> Result<Self, GetClientError> {
        let mut session = Self::offline(Some(indexer_uri));
        session.go_online().await?;
        Ok(session)
    }

    /// Whether a live `indexer` is held.
    pub fn is_online(&self) -> bool {
        self.indexer.is_some()
    }

    /// The URI the session attaches to, held whether or not it is online.
    pub fn indexer_uri(&self) -> Option<&Uri> {
        self.indexer_uri.as_ref()
    }

    /// The live `indexer`, `Some` only while online.
    pub fn indexer(&self) -> Option<&GrpcIndexer> {
        self.indexer.as_ref()
    }

    /// The live `indexer` beside the `indexer_uri` it was dialed from, `Some`
    /// only while online.
    pub fn connected(&self) -> Option<(&Uri, &GrpcIndexer)> {
        self.indexer_uri.as_ref().zip(self.indexer.as_ref())
    }

    /// Dials the held `indexer_uri` and returns it. A session already online
    /// dials it again and replaces `indexer`. A session that holds no
    /// `indexer_uri` refuses with [`GetClientError::NoIndexerUri`].
    pub async fn go_online(&mut self) -> Result<Uri, GetClientError> {
        let uri = self
            .indexer_uri
            .clone()
            .ok_or(GetClientError::NoIndexerUri)?;
        self.indexer = Some(GrpcIndexer::new(uri.clone()).await?);
        Ok(uri)
    }

    /// Drops `indexer` and keeps `indexer_uri`. Returns whether an `indexer`
    /// was held, so a caller can tell a teardown from a no-op.
    pub fn go_offline(&mut self) -> bool {
        self.indexer.take().is_some()
    }

    /// Dials `uri` and, on success, holds it as the Session Indexer in place
    /// of any URI held before.
    pub async fn connect_to(&mut self, uri: Uri) -> Result<(), GetClientError> {
        let indexer = GrpcIndexer::new(uri.clone()).await?;
        self.indexer = Some(indexer);
        self.indexer_uri = Some(uri);
        Ok(())
    }

    /// As [`Self::connect_to`], but `indexer` connects on first use instead
    /// of eagerly, as [`GrpcIndexer::new_lazy`] does.
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
            .unwrap();
        let (held, _indexer) = session.connected().expect("online");
        assert_eq!(held, &uri("https://second.test:443"));
        assert!(session.is_online());
    }

    #[tokio::test]
    async fn go_offline_keeps_the_uri() {
        let mut session = NetworkAccess::offline(None);
        session
            .connect_to_lazy(uri("https://named.test:443"))
            .unwrap();
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
            .unwrap();

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
