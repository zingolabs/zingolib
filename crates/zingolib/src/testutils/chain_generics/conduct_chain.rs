use zcash_primitives::transaction::TxId;

use crate::config::{ClientConfig, WalletConfig};
use crate::get_base_address_macro;
use crate::lightclient::LightClient;
use crate::testutils::lightclient::from_inputs;
use crate::testutils::timed;
use crate::wallet::LightWallet;
use crate::wallet::keys::unified::ReceiverSelection;

#[allow(async_fn_in_trait)]
#[allow(opaque_hidden_inferred_bound)]
/// a trait (capability) for operating a server.
/// delegates client setup, because different mock servers require different client configuration
/// currently, the server conductor is limited to adding to the mock blockchain linearly (bump chain)
pub trait ConductChain {
    /// set up the test chain
    async fn setup() -> Self;

    /// used to connect to server via grpc
    fn lightserver_uri(&self) -> Option<http::Uri>;

    /// builds a faucet (funded from mining)
    async fn create_faucet(&mut self) -> LightClient;

    /// the server communicates some parameters (asyncronously)
    /// that are here compiled into an appropriate wallet configuration
    // super awful that this function has to exist, because the wallet should be able to communicate without 'test-only helpers'
    async fn zingo_config(&mut self) -> crate::config::ClientConfig;

    /// builds an empty client
    async fn create_client(&mut self) -> LightClient {
        let config = timed("create_client::zingo_config", self.zingo_config()).await;
        assert!(!matches!(config.wallet_config(), WalletConfig::Read));
        let mut lightclient = timed(
            "create_client::new_nakednet_consented",
            LightClient::new_nakednet_consented(config, false),
        )
        .await
        .unwrap();
        timed(
            "create_client::generate_unified_address",
            lightclient.generate_unified_address(
                ReceiverSelection::sapling_only(),
                zip32::AccountId::ZERO,
            ),
        )
        .await
        .unwrap();

        lightclient
    }

    /// loads a client from bytes
    async fn load_client(&mut self, config: ClientConfig) -> LightClient {
        assert!(matches!(config.wallet_config(), WalletConfig::Read));
        LightClient::new_nakednet_consented(config, false)
            .await
            .unwrap()
    }

    /// moves the chain tip forward, creating 1 new block
    /// and confirming transactions that were received by the server
    async fn increase_chain_height(&mut self);

    /// Syncs `client` far enough to act on the environment's current chain
    /// tip. The default is a bare sync (sufficient for mocked chains), and
    /// environments with a real Validator behind a lagging Indexer must
    /// override this to sync deterministically to the Validator's tip, since
    /// a client that signs from a stale tip produces transactions the
    /// Validator rejects (wrong consensus branch id near upgrade
    /// activations).
    async fn sync_client_to_tip(&self, client: &mut LightClient) {
        client.sync_and_await().await.unwrap();
    }

    /// builds a client and funds it in orchard and syncs it
    async fn fund_client_orchard(&mut self, value: u64) -> LightClient {
        let mut faucet = timed("fund_client_orchard::create_faucet", self.create_faucet()).await;
        let mut recipient = timed("fund_client_orchard::create_client", self.create_client()).await;

        timed(
            "fund_client_orchard::increase_chain_height[pre-send]",
            self.increase_chain_height(),
        )
        .await;
        timed(
            "fund_client_orchard::sync_faucet_to_tip",
            self.sync_client_to_tip(&mut faucet),
        )
        .await;

        let recipient_unified_address =
            timed("fund_client_orchard::get_recipient_address", async {
                get_base_address_macro!(recipient, "unified")
            })
            .await;
        timed(
            "fund_client_orchard::quick_send",
            from_inputs::quick_send(
                &mut faucet,
                vec![(recipient_unified_address.as_str(), value, None)],
            ),
        )
        .await
        .unwrap();

        timed(
            "fund_client_orchard::increase_chain_height[post-send]",
            self.increase_chain_height(),
        )
        .await;

        timed(
            "fund_client_orchard::sync_recipient_to_tip",
            self.sync_client_to_tip(&mut recipient),
        )
        .await;

        recipient
    }

    /// how many blocks of leeway to allow the chain to Confirm a transaction
    fn confirmation_patience_blocks(&self) -> usize;

    /// - Moves the chain tip forward up to the confirmation patience, syncing `client` after each
    ///   block, and stops as soon as every one of `txids` is confirmed.
    async fn await_confirmed(&mut self, client: &mut LightClient, txids: &[TxId]) -> bool {
        for _ in 0..self.confirmation_patience_blocks() {
            self.increase_chain_height().await;
            self.sync_client_to_tip(client).await;
            if all_confirmed(&*client.wallet().read().await, txids) {
                return true;
            }
        }
        all_confirmed(&*client.wallet().read().await, txids)
    }
}

pub fn all_confirmed(wallet: &LightWallet, txids: &[TxId]) -> bool {
    txids.iter().all(|txid| {
        wallet
            .wallet_transactions
            .get(txid)
            .is_some_and(|transaction| transaction.status().is_confirmed())
    })
}
