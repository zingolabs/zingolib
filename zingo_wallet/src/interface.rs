pub struct ZingoWallet {
    keys: Vec<String>,
    lightclient: Option<zingolib::lightclient::LightClient>,
}

mod add_server;
mod protocol_id;

impl zcash_wallet_interface::Wallet for ZingoWallet {
    fn protocol_id() -> zcash_wallet_interface::ProtocolId {
        protocol_id::protocol_id()
    }

    async fn new_wallet() -> Self {
        // we cannot instantiate the current version of the lightclient yet
        // without assumptions about keys and servers
        // which would violate principles of the interface
        // so we dont
        ZingoWallet {
            keys: Vec::new(),
            lightclient: None,
        }
    }

    type AddServerError = add_server::AddServerError;

    async fn add_server(&mut self, server_address: String) -> Result<(), Self::AddServerError> {
        add_server::add_server(self, server_address).await
    }

    type AddKeyError = ();

    async fn add_key(&mut self, key_string: String) -> Result<(), Self::AddKeyError> {
        todo!()
    }

    type GetMaxScannedHeightError = ();

    async fn get_max_scanned_height_for_server(
        &mut self,
        server: String,
    ) -> Result<zcash_wallet_interface::BlockHeight, Self::GetMaxScannedHeightError> {
        todo!()
    }

    type PayError = ();

    async fn pay(
        &mut self,
        payments: Vec<zcash_wallet_interface::Payment>,
    ) -> Result<(), Self::PayError> {
        todo!()
    }
}
