//! TODO: Add Mod Description Here!

use orchard::{note_encryption::OrchardDomain, tree::MerkleHashOrchard};
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::proto::service::TreeState;
use zcash_note_encryption::Domain;

use crate::wallet::{
    data::COMMITMENT_TREE_LEVELS,
    notes,
    traits::{self, DomainWalletExt},
};

/// TODO: Add Doc Comment Here!
pub fn get_legacy_frontiers(
    trees: TreeState,
) -> (
    Option<incrementalmerkletree::frontier::NonEmptyFrontier<sapling_crypto::Node>>,
    Option<incrementalmerkletree::frontier::NonEmptyFrontier<MerkleHashOrchard>>,
) {
    (
        get_legacy_frontier::<SaplingDomain>(&trees),
        get_legacy_frontier::<OrchardDomain>(&trees),
    )
}

fn get_legacy_frontier<D: DomainWalletExt>(
    trees: &TreeState,
) -> Option<
    incrementalmerkletree::frontier::NonEmptyFrontier<
        <D::WalletNote as notes::ShieldedNoteInterface>::Node,
    >,
>
where
    <D as Domain>::Note: PartialEq + Clone,
    <D as Domain>::Recipient: traits::Recipient,
{
    zcash_primitives::merkle_tree::read_commitment_tree::<
        <D::WalletNote as notes::ShieldedNoteInterface>::Node,
        &[u8],
        COMMITMENT_TREE_LEVELS,
    >(&hex::decode(D::get_tree(trees)).unwrap()[..])
    .ok()
    .and_then(|tree| tree.to_frontier().take())
}
