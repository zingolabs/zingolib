//! A Lightclient test may involve hosting a server to send data to the LightClient. This trait can be asked to set simple scenarios where a mock LightServer sends data showing a note to a LightClient, the LightClient updates and responds by sending the note, and the Lightserver accepts the transaction and rebroadcasts it...
//! The initial two implementors are
//! lib-to-node, which links a lightserver to a zcashd in regtest mode. see `impl ConductChain for LibtoNode
//! darkside, a mode for the lightserver which mocks zcashd. search 'impl ConductChain for DarksideScenario

use crate::get_base_address_macro;
use crate::testutils::lightclient::from_inputs;
use crate::{lightclient::LightClient, wallet::LightWallet};

#[allow(async_fn_in_trait)]
#[allow(opaque_hidden_inferred_bound)]
/// a trait (capability) for operating a server.
/// delegates client setup, because different mock servers require different client configuration
/// currently, the server conductor is limited to adding to the mock blockchain linearly (bump chain)
pub trait ConductChain {
    /// set up the test chain
    async fn setup() -> Self;
    /// builds a faucet (funded from mining)
    async fn create_faucet(&mut self) -> LightClient;

    /// sets server parameters
    fn zingo_config(&mut self) -> crate::config::ZingoConfig;

    /// builds an empty client
    async fn create_client(&mut self) -> LightClient {
        let mut zingo_config = self.zingo_config();
        zingo_config.accept_server_txids = true;
        LightClient::create_from_wallet_base_async(
            crate::wallet::WalletBase::FreshEntropy,
            &zingo_config,
            0,
            false,
        )
        .await
        .unwrap()
    }

    /// loads a client from bytes
    async fn load_client(&mut self, data: &[u8]) -> LightClient {
        let mut zingo_config = self.zingo_config();
        zingo_config.accept_server_txids = true;

        LightClient::create_from_wallet_async(LightWallet::unsafe_from_buffer_testnet(data).await)
            .await
            .unwrap()
    }

    /// moves the chain tip forward, creating 1 new block
    /// and confirming transactions that were received by the server
    async fn bump_chain(&mut self);

    /// gets the height. does not yet need to be async
    fn get_chain_height(&mut self) -> u32;

    /// builds a client and funds it in orchard and syncs it
    async fn fund_client_orchard(&mut self, value: u64) -> LightClient {
        let faucet = self.create_faucet().await;
        let recipient = self.create_client().await;

        self.bump_chain().await;
        faucet.do_sync(false).await.unwrap();

        from_inputs::quick_send(
            &faucet,
            vec![(
                (get_base_address_macro!(recipient, "unified")).as_str(),
                value,
                None,
            )],
        )
        .await
        .unwrap();

        self.bump_chain().await;

        recipient.do_sync(false).await.unwrap();

        recipient
    }
}
