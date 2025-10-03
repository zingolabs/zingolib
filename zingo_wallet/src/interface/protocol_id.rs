struct Version(&'static str);

const VERSION_STR: &str = "0.0.1";
const VERSION: Version = Version(VERSION_STR);

pub(super) fn protocol_id() -> zcash_wallet_interface::ProtocolId {
    zcash_wallet_interface::ProtocolId {
        name: "zingo_wallet".to_string(),
        version: VERSION.0.to_string(),
    }
}
