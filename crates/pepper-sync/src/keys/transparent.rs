//! Transparent keys and addresses

use zcash_protocol::consensus;
use zcash_transparent::address::TransparentAddress;
use zcash_transparent::keys::NonHardenedChildIndex;

use zcash_transparent::keys::{AccountPubKey, IncomingViewingKey as _};

pub use zingolib_common::keys::transparent::{
    TransparentAddressId, TransparentScope, encode_address,
};

pub(crate) fn derive_address(
    consensus_parameters: &impl consensus::Parameters,
    account_pubkey: &AccountPubKey,
    address_id: TransparentAddressId,
) -> Result<String, bip32::Error> {
    let address = match address_id.scope() {
        TransparentScope::External => {
            derive_external_address(account_pubkey, address_id.address_index())?
        }
        TransparentScope::Internal => {
            derive_internal_address(account_pubkey, address_id.address_index())?
        }
        TransparentScope::Refund => {
            derive_refund_address(account_pubkey, address_id.address_index())?
        }
    };

    Ok(encode_address(consensus_parameters, address))
}

fn derive_external_address(
    account_pubkey: &AccountPubKey,
    address_index: NonHardenedChildIndex,
) -> Result<TransparentAddress, bip32::Error> {
    account_pubkey
        .derive_external_ivk()?
        .derive_address(address_index)
}

fn derive_internal_address(
    account_pubkey: &AccountPubKey,
    address_index: NonHardenedChildIndex,
) -> Result<TransparentAddress, bip32::Error> {
    account_pubkey
        .derive_internal_ivk()?
        .derive_address(address_index)
}

fn derive_refund_address(
    account_pubkey: &AccountPubKey,
    address_index: NonHardenedChildIndex,
) -> Result<TransparentAddress, bip32::Error> {
    account_pubkey
        .derive_ephemeral_ivk()?
        .derive_ephemeral_address(address_index)
}
