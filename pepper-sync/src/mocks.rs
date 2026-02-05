use std::num::NonZeroU32;

use crate::config::{self, TransparentAddressDiscovery};
use zcash_protocol::consensus::BlockHeight;
//libuse zingolib::config::zingoconfigbuilder;
//use zingolib::wallet::{lightwallet, walletbase::freshentropy, walletsettings};
/*
fn create_bday_confirm_wallet(bday: blockheight, min_confirmations: nonzerou32) -> lightwallet {
    let chain = zingoconfigbuilder::default().create().chain;
    let sync_config = config::syncconfig {
        transparent_address_discovery: transparentaddressdiscovery::minimal(),
        performance_level: config::performancelevel::low,
    };
    let entropy = freshentropy {
        no_of_accounts: nonzerou32::try_from(1).expect("1 is non-zero u32"),
    };
    let wallet_settings = walletsettings {
        sync_config,
        min_confirmations,
    };
    todo!()
}
*/
