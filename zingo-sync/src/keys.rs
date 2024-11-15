//! Copied and modified from LRZ due to thread safety limitations and missing OVK

use std::collections::HashMap;

use getset::Getters;
use incrementalmerkletree::Position;
use orchard::{
    keys::{FullViewingKey, IncomingViewingKey, Scope},
    note_encryption::OrchardDomain,
};
use sapling_crypto::{
    self as sapling, note_encryption::SaplingDomain, NullifierDerivingKey, SaplingIvk,
};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_note_encryption::Domain;

pub(crate) type KeyId = (zcash_primitives::zip32::AccountId, Scope);

/// A key that can be used to perform trial decryption and nullifier
/// computation for a [`CompactSaplingOutput`] or [`CompactOrchardAction`].
pub trait ScanningKeyOps<D: Domain, Nf> {
    /// Prepare the key for use in batch trial decryption.
    fn prepare(&self) -> D::IncomingViewingKey;

    /// Returns the account identifier for this key. An account identifier corresponds
    /// to at most a single unified spending key's worth of spend authority, such that
    /// both received notes and change spendable by that spending authority will be
    /// interpreted as belonging to that account.
    fn account_id(&self) -> &zcash_primitives::zip32::AccountId;

    /// Returns the [`zip32::Scope`] for which this key was derived, if known.
    fn key_scope(&self) -> Option<Scope>;

    /// Produces the nullifier for the specified note and witness, if possible.
    ///
    /// IVK-based implementations of this trait cannot successfully derive
    /// nullifiers, in which this function will always return `None`.
    fn nf(&self, note: &D::Note, note_position: Position) -> Option<Nf>;
}
impl<D: Domain, Nf, K: ScanningKeyOps<D, Nf>> ScanningKeyOps<D, Nf> for &K {
    fn prepare(&self) -> D::IncomingViewingKey {
        (*self).prepare()
    }

    fn account_id(&self) -> &zcash_primitives::zip32::AccountId {
        (*self).account_id()
    }

    fn key_scope(&self) -> Option<Scope> {
        (*self).key_scope()
    }

    fn nf(&self, note: &D::Note, note_position: Position) -> Option<Nf> {
        (*self).nf(note, note_position)
    }
}

pub(crate) struct ScanningKey<Ivk, Nk> {
    key_id: KeyId,
    ivk: Ivk,
    // TODO: Ovk
    nk: Option<Nk>,
}

impl ScanningKeyOps<SaplingDomain, sapling::Nullifier>
    for ScanningKey<SaplingIvk, NullifierDerivingKey>
{
    fn prepare(&self) -> sapling::note_encryption::PreparedIncomingViewingKey {
        sapling_crypto::note_encryption::PreparedIncomingViewingKey::new(&self.ivk)
    }

    fn nf(&self, note: &sapling::Note, position: Position) -> Option<sapling::Nullifier> {
        self.nk.as_ref().map(|key| note.nf(key, position.into()))
    }

    fn account_id(&self) -> &zcash_primitives::zip32::AccountId {
        &self.key_id.0
    }

    fn key_scope(&self) -> Option<Scope> {
        Some(self.key_id.1)
    }
}

impl ScanningKeyOps<OrchardDomain, orchard::note::Nullifier>
    for ScanningKey<IncomingViewingKey, FullViewingKey>
{
    fn prepare(&self) -> orchard::keys::PreparedIncomingViewingKey {
        orchard::keys::PreparedIncomingViewingKey::new(&self.ivk)
    }

    fn nf(
        &self,
        note: &orchard::note::Note,
        _position: Position,
    ) -> Option<orchard::note::Nullifier> {
        self.nk.as_ref().map(|key| note.nullifier(key))
    }

    fn account_id(&self) -> &zcash_primitives::zip32::AccountId {
        &self.key_id.0
    }

    fn key_scope(&self) -> Option<Scope> {
        Some(self.key_id.1)
    }
}

/// A set of keys to be used in scanning for decryptable transaction outputs.
#[derive(Getters)]
#[getset(get = "pub(crate)")]
pub(crate) struct ScanningKeys {
    sapling: HashMap<KeyId, ScanningKey<SaplingIvk, NullifierDerivingKey>>,
    orchard: HashMap<KeyId, ScanningKey<IncomingViewingKey, FullViewingKey>>,
}

impl ScanningKeys {
    /// Constructs a [`ScanningKeys`] from an iterator of [`zcash_keys::keys::UnifiedFullViewingKey`]s,
    /// along with the account identifiers corresponding to those UFVKs.
    pub(crate) fn from_account_ufvks(
        ufvks: impl IntoIterator<Item = (zcash_primitives::zip32::AccountId, UnifiedFullViewingKey)>,
    ) -> Self {
        #![allow(clippy::type_complexity)]

        let mut sapling: HashMap<KeyId, ScanningKey<SaplingIvk, NullifierDerivingKey>> =
            HashMap::new();
        let mut orchard: HashMap<KeyId, ScanningKey<IncomingViewingKey, FullViewingKey>> =
            HashMap::new();

        for (account_id, ufvk) in ufvks {
            if let Some(dfvk) = ufvk.sapling() {
                for scope in [Scope::External, Scope::Internal] {
                    sapling.insert(
                        (account_id, scope),
                        ScanningKey {
                            key_id: (account_id, scope),
                            ivk: dfvk.to_ivk(scope),
                            nk: Some(dfvk.to_nk(scope)),
                        },
                    );
                }
            }

            if let Some(fvk) = ufvk.orchard() {
                for scope in [Scope::External, Scope::Internal] {
                    orchard.insert(
                        (account_id, scope),
                        ScanningKey {
                            key_id: (account_id, scope),
                            ivk: fvk.to_ivk(scope),
                            nk: Some(fvk.clone()),
                        },
                    );
                }
            }
        }

        Self { sapling, orchard }
    }
}
