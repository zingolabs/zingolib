//! tests that can be run either as lib-to-node or darkside.
#[allow(async_fn_in_trait)]
/// both lib-to-node and darkside can implement this.
pub trait ChainTest {
    /// returns the seed funded by mining.
    async fn mining_seed() -> String;
    /// moves the chain tip forward, confirming transactions that need to be confirmed
    async fn bump_chain();
}
