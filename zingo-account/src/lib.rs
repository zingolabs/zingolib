//! A micro crate to provide a wrapper around the zcash_client_backend account trait.
//! By constraining the wrapper to be independent of internal zingolib functionality
//! we expose a shareable component, and refine the definition of zingolib.
pub struct ZingoAccount(
    pub zip32::AccountId,
    pub zcash_keys::keys::UnifiedFullViewingKey,
);

impl zcash_client_backend::data_api::Account<zip32::AccountId> for ZingoAccount {
    fn id(&self) -> zip32::AccountId {
        self.0
    }

    fn source(&self) -> zcash_client_backend::data_api::AccountSource {
        unimplemented!()
    }

    fn ufvk(&self) -> Option<&zcash_keys::keys::UnifiedFullViewingKey> {
        Some(&self.1)
    }

    fn uivk(&self) -> zcash_keys::keys::UnifiedIncomingViewingKey {
        unimplemented!()
    }
}
