pub mod interface;

pub struct ZingoWallet {
    keys: Vec<String>, //todo parsing and keyring
    pub lightclient: Option<zingolib::lightclient::LightClient>,
}
