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

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct KeyId {
    account_id: zcash_primitives::zip32::AccountId,
    scope: Scope,
}

impl KeyId {
    pub fn from_parts(account_id: zcash_primitives::zip32::AccountId, scope: Scope) -> Self {
        Self { account_id, scope }
    }
}

impl memuse::DynamicUsage for KeyId {
    fn dynamic_usage(&self) -> usize {
        self.scope.dynamic_usage()
    }

    fn dynamic_usage_bounds(&self) -> (usize, Option<usize>) {
        self.scope.dynamic_usage_bounds()
    }
}

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

    /// Returns the outgtoing viewing key
    fn ovk(&self) -> D::OutgoingViewingKey;
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

    fn ovk(&self) -> D::OutgoingViewingKey {
        (*self).ovk()
    }
}

pub(crate) struct ScanningKey<Ivk, Nk, Ovk> {
    key_id: KeyId,
    ivk: Ivk,
    nk: Option<Nk>,
    ovk: Ovk,
}

impl ScanningKeyOps<SaplingDomain, sapling::Nullifier>
    for ScanningKey<SaplingIvk, NullifierDerivingKey, sapling::keys::OutgoingViewingKey>
{
    fn prepare(&self) -> sapling::note_encryption::PreparedIncomingViewingKey {
        sapling_crypto::note_encryption::PreparedIncomingViewingKey::new(&self.ivk)
    }

    fn nf(&self, note: &sapling::Note, position: Position) -> Option<sapling::Nullifier> {
        self.nk.as_ref().map(|key| note.nf(key, position.into()))
    }

    fn ovk(&self) -> <SaplingDomain as Domain>::OutgoingViewingKey {
        self.ovk
    }

    fn account_id(&self) -> &zcash_primitives::zip32::AccountId {
        &self.key_id.account_id
    }

    fn key_scope(&self) -> Option<Scope> {
        Some(self.key_id.scope)
    }
}

impl ScanningKeyOps<OrchardDomain, orchard::note::Nullifier>
    for ScanningKey<IncomingViewingKey, FullViewingKey, orchard::keys::OutgoingViewingKey>
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

    fn ovk(&self) -> <OrchardDomain as Domain>::OutgoingViewingKey {
        self.ovk.clone()
    }

    fn account_id(&self) -> &zcash_primitives::zip32::AccountId {
        &self.key_id.account_id
    }

    fn key_scope(&self) -> Option<Scope> {
        Some(self.key_id.scope)
    }
}

/// A set of keys to be used in scanning for decryptable transaction outputs.
#[derive(Getters)]
#[getset(get = "pub(crate)")]
pub(crate) struct ScanningKeys {
    sapling: HashMap<
        KeyId,
        ScanningKey<SaplingIvk, NullifierDerivingKey, sapling::keys::OutgoingViewingKey>,
    >,
    orchard: HashMap<
        KeyId,
        ScanningKey<IncomingViewingKey, FullViewingKey, orchard::keys::OutgoingViewingKey>,
    >,
}

impl ScanningKeys {
    /// Constructs a [`ScanningKeys`] from an iterator of [`zcash_keys::keys::UnifiedFullViewingKey`]s,
    /// along with the account identifiers corresponding to those UFVKs.
    pub(crate) fn from_account_ufvks(
        ufvks: impl IntoIterator<Item = (zcash_primitives::zip32::AccountId, UnifiedFullViewingKey)>,
    ) -> Self {
        #![allow(clippy::type_complexity)]

        let mut sapling: HashMap<
            KeyId,
            ScanningKey<SaplingIvk, NullifierDerivingKey, sapling::keys::OutgoingViewingKey>,
        > = HashMap::new();
        let mut orchard: HashMap<
            KeyId,
            ScanningKey<IncomingViewingKey, FullViewingKey, orchard::keys::OutgoingViewingKey>,
        > = HashMap::new();

        for (account_id, ufvk) in ufvks {
            if let Some(dfvk) = ufvk.sapling() {
                for scope in [Scope::External, Scope::Internal] {
                    let key_id = KeyId::from_parts(account_id, scope);
                    sapling.insert(
                        key_id,
                        ScanningKey {
                            key_id,
                            ivk: dfvk.to_ivk(scope),
                            nk: Some(dfvk.to_nk(scope)),
                            ovk: dfvk.to_ovk(scope),
                        },
                    );
                }
            }

            if let Some(fvk) = ufvk.orchard() {
                for scope in [Scope::External, Scope::Internal] {
                    let key_id = KeyId::from_parts(account_id, scope);
                    orchard.insert(
                        key_id,
                        ScanningKey {
                            key_id,
                            ivk: fvk.to_ivk(scope),
                            nk: Some(fvk.clone()),
                            ovk: fvk.to_ovk(scope),
                        },
                    );
                }
            }
        }

        Self { sapling, orchard }
    }
}
