#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum WalletError {
    #[error("Command queue closed")]
    CommandQueueClosed,
    #[error("Listener lock poisoned")]
    ListenerLockPoisoned,
    #[error("Wallet not initialized")]
    NotInitialized,
    #[error("Internal error: {0}")]
    Internal(String),
}
