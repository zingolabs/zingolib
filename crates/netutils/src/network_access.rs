use http::Uri;

use crate::{GetClientError, GrpcIndexer};

/// ```
/// use zl_netutils::{GetClientError, NetworkAccess};
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
mod tests;
