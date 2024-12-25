use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

use tokio::sync::mpsc;

use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::{self, BlockHeight};
use zcash_primitives::zip32::AccountId;

use crate::client::{self, FetchRequest};
use crate::keys;
use crate::keys::transparent::{TransparentAddressId, TransparentScope};
use crate::primitives::Locator;
use crate::traits::SyncWallet;

use super::MAX_VERIFICATION_WINDOW;

const ADDRESS_GAP_LIMIT: usize = 20;

/// Discovers all addresses in use by the wallet and returns locators for any new relevant transactions to scan transparent
/// bundles.
/// `wallet_height` should be the value before updating scan ranges. i.e. the wallet height as of previous sync.
pub(crate) async fn update_addresses_and_locators<P, W>(
    consensus_parameters: &P,
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    wallet_height: BlockHeight,
    chain_height: BlockHeight,
) where
    P: consensus::Parameters,
    W: SyncWallet,
{
    let wallet_addresses = wallet.get_transparent_addresses_mut().unwrap();
    let mut locators: BTreeSet<Locator> = BTreeSet::new();
    let block_range = Range {
        start: wallet_height + 1 - MAX_VERIFICATION_WINDOW,
        end: chain_height + 1,
    };

    // find locators for any new transactions relevant to known addresses
    for address in wallet_addresses.values() {
        let transactions = client::get_transparent_address_transactions(
            fetch_request_sender.clone(),
            address.clone(),
            block_range.clone(),
        )
        .await
        .unwrap();

        // The transaction is not scanned here, instead the locator is stored to be later sent to a scan task for these reasons:
        // - We must search for all relevant transactions MAX_VERIFICATION_WINDOW blocks below wallet height in case of re-org.
        // These would be scanned again which would be inefficient
        // - In case of re-org, any scanned transactions with heights within the re-org range would be wrongly invalidated
        // - The locator will cause the surrounding range to be set to high priority which will often also contain shielded notes
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
        transactions.iter().for_each(|(height, tx)| {
            locators.insert((*height, tx.txid()));
        });
    }

    // discover new addresses and find locators for relevant transactions
    for (account_id, ufvk) in ufvks {
        if let Some(account_pubkey) = ufvk.transparent() {
            for scope in [
                TransparentScope::External,
                TransparentScope::Internal,
                TransparentScope::Refund,
            ] {
                // start with the first address index previously unused by the wallet
                let mut address_index = if let Some(id) = wallet_addresses
                    .iter()
                    .map(|(id, _)| id)
                    .filter(|id| id.account_id() == *account_id && id.scope() == scope)
                    .next_back()
                {
                    id.address_index() + 1
                } else {
                    0
                };
                let mut unused_address_count: usize = 0;
                let mut addresses: Vec<(TransparentAddressId, String)> = Vec::new();

                while unused_address_count < ADDRESS_GAP_LIMIT {
                    let address_id =
                        TransparentAddressId::from_parts(*account_id, scope, address_index);
                    let address = keys::transparent::derive_address(
                        consensus_parameters,
                        account_pubkey,
                        address_id,
                    );
                    addresses.push((address_id, address.clone()));

                    let transactions = client::get_transparent_address_transactions(
                        fetch_request_sender.clone(),
                        address,
                        block_range.clone(),
                    )
                    .await
                    .unwrap();

                    if transactions.is_empty() {
                        unused_address_count += 1;
                    } else {
                        transactions.iter().for_each(|(height, tx)| {
                            locators.insert((*height, tx.txid()));
                        });
                        unused_address_count = 0;
                    }

                    address_index += 1;
                }

                addresses.truncate(addresses.len() - ADDRESS_GAP_LIMIT);
                addresses.into_iter().for_each(|(id, address)| {
                    wallet_addresses.insert(id, address);
                });
            }
        }
    }

    wallet
        .get_sync_state_mut()
        .unwrap()
        .locators_mut()
        .append(&mut locators);
}

// TODO: process memo encoded address indexes.
// 1. return any memo address ids from scan in ScanResults
// 2. derive the addresses up to that index, add to wallet addresses and send them to GetTaddressTxids
// 3. for each transaction returned:
// a) if the tx is in a range that is not scanned, add locator to sync_state
// b) if the range is scanned and the tx is already in the wallet, rescan the zcash transaction transparent bundles in
// the wallet transaction
// c) if the range is scanned and the tx does not exist in the wallet, fetch the compact block if its not in the wallet
// and scan the transparent bundles
