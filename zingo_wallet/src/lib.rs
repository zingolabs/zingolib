pub mod interface;

pub struct ZingoWallet {
    keys: Vec<String>, //todo parsing and keyring
    lightclient: Option<zingolib::lightclient::LightClient>,
}

fn main() {
    println!("Hello, world!");
}
