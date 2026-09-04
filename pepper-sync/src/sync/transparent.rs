use std::cmp;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::sync::Arc;

use tokio::sync::{RwLock, mpsc};

use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_protocol::consensus::{self, BlockHeight};
use zcash_transparent::keys::NonHardenedChildIndex;
use zip32::AccountId;

use crate::client::{self, FetchRequest};
use crate::config::TransparentAddressDiscovery;
use crate::error::SyncError;
use crate::keys;
use crate::keys::transparent::{TransparentAddressId, TransparentScope};
use crate::wallet::traits::SyncWallet;
use crate::wallet::{KeyIdInterface, ScanTarget};

use super::MAX_REORG_ALLOWANCE;

/// Discovers all addresses in use by the wallet and returns `scan_targets` for any new relevant transactions to scan transparent
/// bundles.
/// `last_known_chain_height` should be the value before updating to latest chain height.
pub(crate) async fn update_addresses_and_scan_targets<W: SyncWallet>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: Arc<RwLock<W>>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    last_known_chain_height: BlockHeight,
    chain_height: BlockHeight,
    config: TransparentAddressDiscovery,
) -> Result<(), SyncError<W::Error>> {
    if !config.scopes.external && !config.scopes.internal && !config.scopes.refund {
        return Ok(());
    }

    let wallet_addresses = wallet
        .read()
        .await
        .get_transparent_addresses()
        .map_err(SyncError::WalletError)?
        .clone();
    let mut scan_targets: BTreeSet<ScanTarget> = BTreeSet::new();
    let sapling_activation_height = consensus_parameters
        .activation_height(consensus::NetworkUpgrade::Sapling)
        .expect("sapling activation height should always return Some");
    let block_range_start = last_known_chain_height.saturating_sub(MAX_REORG_ALLOWANCE) + 1;
    let checked_block_range_start = match block_range_start.cmp(&sapling_activation_height) {
        cmp::Ordering::Greater | cmp::Ordering::Equal => block_range_start,
        cmp::Ordering::Less => sapling_activation_height,
    };
    let block_range = Range {
        start: checked_block_range_start,
        end: chain_height + 1,
    };

    // find scan_targets for any new transactions relevant to known addresses
    for address in wallet_addresses.values() {
        let transactions = client::get_transparent_address_transactions(
            fetch_request_sender.clone(),
            consensus_parameters,
            address.clone(),
            block_range.clone(),
        )
        .await?;

        // The transaction is not scanned here, instead the scan target is stored to be later sent to a scan task for these reasons:
        // - We must search for all relevant transactions MAX_REORG_ALLOWANCE blocks below wallet height in case of re-org.
        // These would be scanned again which would be inefficient
        // - In case of re-org, any scanned transactions with heights within the re-org range would be wrongly invalidated
        // - The scan target will cause the surrounding range to be set to high priority which will often also contain shielded notes
        // relevant to the wallet
        // - Scanning a transaction without scanning the surrounding range of compact blocks in the context of a scan task creates
        // complications. Instead of writing all the information into a wallet transaction once, it would result in "incomplete"
        // transactions that only contain transparent outputs and must be updated with shielded notes and other data when scanned.
        // - We would need to add additional processing here to fetch the compact block for transaction metadata such as block time
        // and append this to the wallet.
        // - It allows SyncState to maintain complete knowledge and control of all the tasks that have and will be performed by the
        // sync engine.
        //
        // To summarise, keeping transaction scanning within the scanner is much better co-ordinated and allows us to leverage
        // any new developments to sync state management and scanning. It also separates concerns, with tasks happening in one
        // place and performed once, wherever possible.
        for (height, tx) in &transactions {
            scan_targets.insert(ScanTarget {
                block_height: *height,
                txid: tx.txid(),
                narrow_scan_area: true,
            });
        }
    }

    let mut scopes = Vec::new();
    if config.scopes.external {
        scopes.push(TransparentScope::External);
    }
    if config.scopes.internal {
        scopes.push(TransparentScope::Internal);
    }
    if config.scopes.refund {
        scopes.push(TransparentScope::Refund);
    }

    // discover new addresses and find scan_targets for relevant transactions
    for (account_id, ufvk) in ufvks {
        if let Some(account_pubkey) = ufvk.transparent() {
            for scope in &scopes {
                // start with the first address index previously unused by the wallet
                let mut address_index = if let Some(id) = wallet_addresses
                    .keys()
                    .rfind(|id| id.account_id() == *account_id && id.scope() == *scope)
                {
                    id.address_index().next()
                } else {
                    Some(NonHardenedChildIndex::ZERO)
                }
                .ok_or_else(|| {
                    SyncError::TransparentAddressDerivationError(bip32::Error::ChildNumber)
                })?;
                let mut unused_address_count: usize = 0;
                let mut addresses: Vec<(TransparentAddressId, String)> = Vec::new();

                while unused_address_count < config.gap_limit as usize {
                    let address_id = TransparentAddressId::new(*account_id, *scope, address_index);
                    let address = keys::transparent::derive_address(
                        consensus_parameters,
                        account_pubkey,
                        address_id,
                    )
                    .map_err(SyncError::TransparentAddressDerivationError)?;
                    addresses.push((address_id, address.clone()));

                    let transactions = client::get_transparent_address_transactions(
                        fetch_request_sender.clone(),
                        consensus_parameters,
                        address,
                        block_range.clone(),
                    )
                    .await?;

                    if transactions.is_empty() {
                        unused_address_count += 1;
                    } else {
                        for (height, tx) in &transactions {
                            scan_targets.insert(ScanTarget {
                                block_height: *height,
                                txid: tx.txid(),
                                narrow_scan_area: true,
                            });
                        }
                        unused_address_count = 0;
                    }

                    address_index = address_index.next().ok_or_else(|| {
                        SyncError::TransparentAddressDerivationError(bip32::Error::ChildNumber)
                    })?;
                }

                addresses.truncate(addresses.len() - config.gap_limit as usize);

                let mut wallet_guard = wallet.write().await;
                let wallet_addresses_mut = wallet_guard
                    .get_transparent_addresses_mut()
                    .map_err(SyncError::WalletError)?;
                for (id, address) in addresses {
                    wallet_addresses_mut.insert(id, address);
                }
            }
        }
    }

    let mut wallet_guard = wallet.write().await;
    wallet_guard
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?
        .scan_targets
        .append(&mut scan_targets);
    wallet_guard
        .set_save_flag()
        .map_err(SyncError::WalletError)?;

    Ok(())
}

// TODO: process memo encoded address indexes.
// 1. return any memo address ids from scan in ScanResults
// 2. derive the addresses up to that index, add to wallet addresses and send them to GetTaddressTxids
// 3. for each transaction returned:
// a) if the tx is in a range that is not scanned, add scan_targets to sync_state
// b) if the range is scanned and the tx is already in the wallet, rescan the zcash transaction transparent bundles in
// the wallet transaction
// c) if the range is scanned and the tx does not exist in the wallet, fetch the compact block if its not in the wallet
// and scan the transparent bundles

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use futures::stream;
    use tonic::codec::{
        DecodeBuf, Decoder, EncodeBody, EncodeBuf, Encoder, SingleMessageCompressionOverride,
        Streaming,
    };
    use tonic::codegen::http::StatusCode;
    use zcash_keys::keys::UnifiedSpendingKey;
    use zcash_primitives::transaction::{Authorized, TransactionData, TxVersion};
    use zcash_protocol::consensus::BranchId;
    use zcash_protocol::local_consensus::LocalNetwork;
    use zingo_netutils::lightwallet_protocol::RawTransaction;

    use super::*;
    use crate::config::TransparentAddressDiscoveryScopes;
    use crate::mocks::MockWalletError;
    use crate::wallet::SyncState;

    const NETWORK: LocalNetwork = LocalNetwork {
        overwinter: Some(BlockHeight::from_u32(1)),
        sapling: Some(BlockHeight::from_u32(1)),
        blossom: Some(BlockHeight::from_u32(1)),
        heartwood: Some(BlockHeight::from_u32(1)),
        canopy: Some(BlockHeight::from_u32(1)),
        nu5: Some(BlockHeight::from_u32(1)),
        nu6: Some(BlockHeight::from_u32(1)),
        nu6_1: Some(BlockHeight::from_u32(1)),
        nu6_2: Some(BlockHeight::from_u32(1)),
        nu6_3: Some(BlockHeight::from_u32(1)),
    };

    /// Minimal in-memory wallet exposing only what `update_addresses_and_scan_targets` touches.
    struct TestWallet {
        sync_state: SyncState,
        transparent_addresses: BTreeMap<TransparentAddressId, String>,
    }

    impl SyncWallet for TestWallet {
        type Error = MockWalletError;

        fn get_birthday(&self) -> Result<BlockHeight, Self::Error> {
            Ok(BlockHeight::from_u32(1))
        }
        fn get_sync_state(&self) -> Result<&SyncState, Self::Error> {
            Ok(&self.sync_state)
        }
        fn get_sync_state_mut(&mut self) -> Result<&mut SyncState, Self::Error> {
            Ok(&mut self.sync_state)
        }
        fn get_unified_full_viewing_keys(
            &self,
        ) -> Result<HashMap<AccountId, UnifiedFullViewingKey>, Self::Error> {
            Ok(HashMap::new())
        }
        fn add_orchard_address(
            &mut self,
            _account_id: AccountId,
            _address: orchard::Address,
            _diversifier_index: zip32::DiversifierIndex,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        fn add_sapling_address(
            &mut self,
            _account_id: AccountId,
            _address: sapling_crypto::PaymentAddress,
            _diversifier_index: zip32::DiversifierIndex,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        fn get_transparent_addresses(
            &self,
        ) -> Result<&BTreeMap<TransparentAddressId, String>, Self::Error> {
            Ok(&self.transparent_addresses)
        }
        fn get_transparent_addresses_mut(
            &mut self,
        ) -> Result<&mut BTreeMap<TransparentAddressId, String>, Self::Error> {
            Ok(&mut self.transparent_addresses)
        }
    }

    /// Writes a zero-length gRPC frame per item. The payload is irrelevant because `FixedDecoder`
    /// ignores it.
    struct EmptyEncoder;

    impl Encoder for EmptyEncoder {
        type Item = ();
        type Error = tonic::Status;

        fn encode(&mut self, _item: (), _dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// Yields the same `RawTransaction` for every frame in the stream.
    struct FixedDecoder(RawTransaction);

    impl Decoder for FixedDecoder {
        type Item = RawTransaction;
        type Error = tonic::Status;

        fn decode(&mut self, _src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
            Ok(Some(self.0.clone()))
        }
    }

    /// Builds a `tonic::Streaming<RawTransaction>` that yields `raw_transaction` `count` times.
    fn raw_transaction_stream(
        raw_transaction: RawTransaction,
        count: usize,
    ) -> Streaming<RawTransaction> {
        let body = EncodeBody::new_server(
            EmptyEncoder,
            stream::iter(std::iter::repeat_n(Ok::<(), tonic::Status>(()), count)),
            None,
            SingleMessageCompressionOverride::default(),
            None,
        );
        Streaming::new_response(
            FixedDecoder(raw_transaction),
            body,
            StatusCode::OK,
            None,
            None,
        )
    }

    fn empty_v5_raw_transaction(height: u32) -> RawTransaction {
        let transaction = TransactionData::<Authorized>::from_parts(
            TxVersion::V5,
            BranchId::Nu5,
            0,
            BlockHeight::from_u32(0),
            None,
            None,
            None,
            None,
        )
        .freeze()
        .expect("empty v5 transaction should freeze");
        let mut data = Vec::new();
        transaction
            .write(&mut data)
            .expect("transaction should serialise");
        RawTransaction {
            data,
            height: u64::from(height),
        }
    }

    // REPRO: claim `transparent-discovery-partial-persist-on-error`.
    // `update_addresses_and_scan_targets` writes each scope's discovered addresses into the
    // wallet inside the per-scope loop but only appends the collected scan targets after all
    // scopes complete. When the server fails part way through a later scope the function
    // returns `Err` with the first scope's addresses persisted and every scan target discarded.
    // On the next sync those addresses are only queried over the recent reorg window, so the
    // transactions found in the aborted pass are never scanned.
    //
    // The invariant asserted here is that an error leaves the wallet in a consistent state:
    // either no addresses were persisted, or the scan targets found for the persisted addresses
    // were also persisted.
    #[tokio::test]
    async fn error_in_later_scope_does_not_persist_addresses_without_their_scan_targets() {
        let chain_height = BlockHeight::from_u32(300);
        let last_known_chain_height = BlockHeight::from_u32(199);
        let transaction_height = 250;

        let ufvk = UnifiedSpendingKey::from_seed(&NETWORK, &[0u8; 32], AccountId::ZERO)
            .expect("seed should derive")
            .to_unified_full_viewing_key();
        assert!(ufvk.transparent().is_some());
        let mut ufvks = HashMap::new();
        ufvks.insert(AccountId::ZERO, ufvk);

        let config = TransparentAddressDiscovery {
            gap_limit: 2,
            scopes: TransparentAddressDiscoveryScopes {
                external: true,
                internal: true,
                refund: false,
            },
        };

        let wallet = Arc::new(RwLock::new(TestWallet {
            sync_state: SyncState::new(),
            transparent_addresses: BTreeMap::new(),
        }));

        // Responder standing in for `client::fetch::fetch`. The wallet has no known addresses, so
        // the requests arrive in this order:
        //   request 0: external index 0  -> one transaction (scan target collected)
        //   request 1: external index 1  -> empty
        //   request 2: external index 2  -> empty (gap limit reached, external scope persisted)
        //   request 3: internal index 0  -> server error (not retried by the client)
        let (fetch_request_sender, mut fetch_request_receiver) =
            mpsc::unbounded_channel::<FetchRequest>();
        let raw_transaction = empty_v5_raw_transaction(transaction_height);
        let responder = tokio::spawn(async move {
            let mut request_count = 0usize;
            while let Some(request) = fetch_request_receiver.recv().await {
                let FetchRequest::TransparentAddressTxs(reply_sender, _) = request else {
                    panic!("unexpected fetch request");
                };
                let reply = match request_count {
                    0 => Ok(raw_transaction_stream(raw_transaction.clone(), 1)),
                    1 | 2 => Ok(raw_transaction_stream(raw_transaction.clone(), 0)),
                    _ => Err(tonic::Status::internal("simulated server failure")),
                };
                let _ = reply_sender.send(reply);
                request_count += 1;
            }
            request_count
        });

        let result = update_addresses_and_scan_targets(
            &NETWORK,
            wallet.clone(),
            fetch_request_sender,
            &ufvks,
            last_known_chain_height,
            chain_height,
            config,
        )
        .await;
        assert!(
            result.is_err(),
            "expected the simulated server failure to propagate"
        );

        let request_count = responder.await.expect("responder should not panic");
        assert_eq!(request_count, 4);

        let wallet_guard = wallet.read().await;
        let persisted_addresses = wallet_guard.get_transparent_addresses().unwrap();
        let persisted_scan_targets = &wallet_guard.get_sync_state().unwrap().scan_targets;

        assert!(
            persisted_addresses.is_empty() || !persisted_scan_targets.is_empty(),
            "addresses were persisted without their scan targets after an error: \
             persisted addresses = {persisted_addresses:?}, \
             persisted scan targets = {persisted_scan_targets:?}"
        );
    }
}
