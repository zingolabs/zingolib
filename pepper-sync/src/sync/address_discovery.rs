use std::collections::{HashMap, HashSet};

use zcash_keys::keys::UnifiedFullViewingKey;
use zip32::AccountId;

use crate::keys;
use crate::wallet::traits::SyncWallet;
use crate::wallet::{KeyIdInterface, NoteInterface, WalletTransaction};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct DiscoveredAddress<Address> {
    account_id: AccountId,
    address: Address,
    diversifier_index: zip32::DiversifierIndex,
}

pub(super) struct DiscoveredAddresses {
    orchard: Vec<DiscoveredAddress<orchard::Address>>,
    sapling: Vec<DiscoveredAddress<sapling_crypto::PaymentAddress>>,
}

#[must_use]
pub(super) fn discover_unified_addresses<'a>(
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    transactions: impl Iterator<Item = &'a WalletTransaction>,
) -> Vec<DiscoveredAddresses> {
    let mut seen_orchard = HashSet::new();
    let mut seen_sapling = HashSet::new();
    transactions
        .map(|transaction| {
            let mut discovered = DiscoveredAddresses {
                orchard: Vec::new(),
                sapling: Vec::new(),
            };
            discover_into(
                ufvks,
                transaction.orchard_notes(),
                orchard::Note::recipient,
                orchard_diversifier_index,
                &mut discovered.orchard,
            );
            // Ironwood recipients are orchard receivers, discovered the same way.
            discover_into(
                ufvks,
                transaction.ironwood_notes(),
                orchard::Note::recipient,
                orchard_diversifier_index,
                &mut discovered.orchard,
            );
            discover_into(
                ufvks,
                transaction.sapling_notes(),
                sapling_crypto::Note::recipient,
                sapling_diversifier_index,
                &mut discovered.sapling,
            );
            retain_first_seen(&mut seen_orchard, &mut discovered.orchard);
            retain_first_seen(&mut seen_sapling, &mut discovered.sapling);
            discovered
        })
        .collect()
}

fn retain_first_seen<Address>(
    seen: &mut HashSet<(AccountId, zip32::DiversifierIndex)>,
    addresses: &mut Vec<DiscoveredAddress<Address>>,
) {
    addresses
        .retain(|discovered| seen.insert((discovered.account_id, discovered.diversifier_index)));
}

fn discover_into<N, Address>(
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    notes: &[N],
    recipient: impl Fn(&N::ZcashNote) -> Address,
    diversifier_index: impl Fn(&UnifiedFullViewingKey, &Address) -> zip32::DiversifierIndex,
    discovered: &mut Vec<DiscoveredAddress<Address>>,
) where
    N: NoteInterface<KeyId = keys::KeyId>,
{
    discovered.extend(
        notes
            .iter()
            .filter(|note| note.key_id().scope == zip32::Scope::External)
            .map(|note| {
                let account_id = note.key_id().account_id();
                let ufvk = ufvks
                    .get(&account_id)
                    .expect("ufvk must exist to decrypt this note");
                let address = recipient(note.note());
                let diversifier_index = diversifier_index(ufvk, &address);
                DiscoveredAddress {
                    account_id,
                    address,
                    diversifier_index,
                }
            }),
    );
}

fn orchard_diversifier_index(
    ufvk: &UnifiedFullViewingKey,
    address: &orchard::Address,
) -> zip32::DiversifierIndex {
    ufvk.orchard()
        .expect("fvk must exist to decrypt this note")
        .to_ivk(zip32::Scope::External)
        .diversifier_index(address)
        .expect("must be key used to create this address")
}

fn sapling_diversifier_index(
    ufvk: &UnifiedFullViewingKey,
    address: &sapling_crypto::PaymentAddress,
) -> zip32::DiversifierIndex {
    ufvk.sapling()
        .expect("fvk must exist to decrypt this note")
        .to_external_ivk()
        .decrypt_diversifier(address)
        .expect("must be key used to create this address")
}

/// - Adds each discovered orchard address to the wallet's unified address list.
/// - Adds each discovered sapling address to the wallet's unified address list.
pub(super) fn add_discovered_addresses<W>(
    wallet: &mut W,
    discovered: Vec<DiscoveredAddresses>,
) -> Result<(), W::Error>
where
    W: SyncWallet,
{
    for transaction_addresses in discovered {
        for DiscoveredAddress {
            account_id,
            address,
            diversifier_index,
        } in transaction_addresses.orchard
        {
            wallet.add_orchard_address(account_id, address, diversifier_index)?;
        }
        for DiscoveredAddress {
            account_id,
            address,
            diversifier_index,
        } in transaction_addresses.sapling
        {
            wallet.add_sapling_address(account_id, address, diversifier_index)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use zcash_keys::keys::{UnifiedFullViewingKey, UnifiedSpendingKey};
    use zcash_primitives::transaction::TxId;
    use zcash_protocol::{
        consensus::{BlockHeight, MAIN_NETWORK},
        memo::Memo,
    };
    use zingo_status::confirmation_status::ConfirmationStatus;

    use super::{DiscoveredAddress, discover_unified_addresses};
    use crate::wallet::{
        IronwoodNote, OrchardNote, OutputId, SaplingNote, WalletNote, WalletTransaction,
    };

    const FIRST_TXID: TxId = TxId::from_bytes([3; 32]);
    const SECOND_TXID: TxId = TxId::from_bytes([4; 32]);
    const STATUS: ConfirmationStatus = ConfirmationStatus::Confirmed(BlockHeight::from_u32(1));
    const ORCHARD_INDEX: u128 = 7;
    const IRONWOOD_INDEX: u128 = 9;
    const INTERNAL_INDEX: u128 = 10;
    const SAPLING_INDEX: u128 = 11;

    fn spending_key() -> UnifiedSpendingKey {
        UnifiedSpendingKey::from_seed(&MAIN_NETWORK, &[0; 32], zip32::AccountId::ZERO).unwrap()
    }

    fn viewing_keys() -> HashMap<zip32::AccountId, UnifiedFullViewingKey> {
        HashMap::from([(
            zip32::AccountId::ZERO,
            spending_key().to_unified_full_viewing_key(),
        )])
    }

    fn diversifier_index(index: u128) -> zip32::DiversifierIndex {
        zip32::DiversifierIndex::try_from(index).unwrap()
    }

    fn orchard_address(index: u128, scope: zip32::Scope) -> orchard::Address {
        orchard::keys::FullViewingKey::from(spending_key().orchard())
            .address_at(diversifier_index(index), scope)
    }

    fn orchard_crypto_note(
        recipient: orchard::Address,
        version: orchard::note::NoteVersion,
    ) -> orchard::Note {
        let rho = orchard::note::Rho::from_bytes(&[0; 32]).unwrap();
        let rseed = orchard::note::RandomSeed::from_bytes([0; 32], &rho).unwrap();
        orchard::Note::from_parts(
            recipient,
            orchard::value::NoteValue::from_raw(1),
            rho,
            rseed,
            version,
        )
        .unwrap()
    }

    fn orchard_note(txid: TxId, index: u128, scope: zip32::Scope) -> OrchardNote {
        WalletNote::new_for_test(
            OutputId::new(txid, 0),
            zip32::AccountId::ZERO,
            scope,
            orchard_crypto_note(
                orchard_address(index, scope),
                orchard::note::NoteVersion::V2,
            ),
            Memo::Empty,
            None,
        )
    }

    fn ironwood_note(txid: TxId, index: u128) -> IronwoodNote {
        WalletNote::new_for_test(
            OutputId::new(txid, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            orchard_crypto_note(
                orchard_address(index, zip32::Scope::External),
                orchard::note::NoteVersion::V3,
            ),
            Memo::Empty,
            None,
        )
    }

    fn sapling_address(index: u128) -> (zip32::DiversifierIndex, sapling_crypto::PaymentAddress) {
        spending_key()
            .sapling()
            .to_diversifiable_full_viewing_key()
            .find_address(diversifier_index(index))
            .unwrap()
    }

    fn sapling_note(txid: TxId, index: u128) -> SaplingNote {
        let (_, recipient) = sapling_address(index);
        WalletNote::new_for_test(
            OutputId::new(txid, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            sapling_crypto::Note::from_parts(
                recipient,
                sapling_crypto::value::NoteValue::from_raw(1),
                sapling_crypto::Rseed::AfterZip212([0; 32]),
            ),
            Memo::Empty,
            None,
        )
    }

    fn planned<Address>(address: Address, index: u128) -> DiscoveredAddress<Address> {
        DiscoveredAddress {
            account_id: zip32::AccountId::ZERO,
            address,
            diversifier_index: diversifier_index(index),
        }
    }

    #[test]
    fn external_notes_of_every_pool_are_discovered() {
        let mut transaction = WalletTransaction::new_for_test_with_orchard_notes(
            FIRST_TXID,
            STATUS,
            vec![orchard_note(
                FIRST_TXID,
                ORCHARD_INDEX,
                zip32::Scope::External,
            )],
            vec![],
        )
        .with_sapling_notes_for_test(vec![sapling_note(FIRST_TXID, SAPLING_INDEX)]);
        transaction.ironwood_notes = vec![ironwood_note(FIRST_TXID, IRONWOOD_INDEX)];

        let discovered = discover_unified_addresses(&viewing_keys(), [&transaction].into_iter());

        let (sapling_index, sapling_recipient) = sapling_address(SAPLING_INDEX);
        assert_eq!(discovered.len(), 1);
        assert_eq!(
            discovered[0].orchard,
            vec![
                planned(
                    orchard_address(ORCHARD_INDEX, zip32::Scope::External),
                    ORCHARD_INDEX
                ),
                planned(
                    orchard_address(IRONWOOD_INDEX, zip32::Scope::External),
                    IRONWOOD_INDEX
                ),
            ]
        );
        assert_eq!(
            discovered[0].sapling,
            vec![DiscoveredAddress {
                account_id: zip32::AccountId::ZERO,
                address: sapling_recipient,
                diversifier_index: sapling_index,
            }]
        );
    }

    #[test]
    fn internal_scope_notes_are_not_discovered() {
        let transaction = WalletTransaction::new_for_test_with_orchard_notes(
            FIRST_TXID,
            STATUS,
            vec![orchard_note(
                FIRST_TXID,
                INTERNAL_INDEX,
                zip32::Scope::Internal,
            )],
            vec![],
        );

        let discovered = discover_unified_addresses(&viewing_keys(), [&transaction].into_iter());

        assert_eq!(discovered.len(), 1);
        assert!(discovered[0].orchard.is_empty());
        assert!(discovered[0].sapling.is_empty());
    }

    #[test]
    fn transactions_keep_their_order() {
        let first = WalletTransaction::new_for_test(FIRST_TXID, STATUS)
            .with_sapling_notes_for_test(vec![sapling_note(FIRST_TXID, SAPLING_INDEX)]);
        let second = WalletTransaction::new_for_test_with_orchard_notes(
            SECOND_TXID,
            STATUS,
            vec![orchard_note(
                SECOND_TXID,
                ORCHARD_INDEX,
                zip32::Scope::External,
            )],
            vec![],
        );

        let discovered = discover_unified_addresses(&viewing_keys(), [&first, &second].into_iter());

        assert_eq!(discovered.len(), 2);
        assert!(discovered[0].orchard.is_empty());
        assert_eq!(discovered[0].sapling.len(), 1);
        assert_eq!(discovered[1].orchard.len(), 1);
        assert!(discovered[1].sapling.is_empty());
    }

    #[test]
    fn repeated_addresses_are_planned_once() {
        let first = WalletTransaction::new_for_test_with_orchard_notes(
            FIRST_TXID,
            STATUS,
            vec![orchard_note(
                FIRST_TXID,
                ORCHARD_INDEX,
                zip32::Scope::External,
            )],
            vec![],
        );
        let second = WalletTransaction::new_for_test_with_orchard_notes(
            SECOND_TXID,
            STATUS,
            vec![orchard_note(
                SECOND_TXID,
                ORCHARD_INDEX,
                zip32::Scope::External,
            )],
            vec![],
        );

        let discovered = discover_unified_addresses(&viewing_keys(), [&first, &second].into_iter());

        assert_eq!(discovered.len(), 2);
        assert_eq!(discovered[0].orchard.len(), 1);
        assert!(discovered[1].orchard.is_empty());
    }
}
