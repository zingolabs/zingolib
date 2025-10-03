mod protocol_id;

pub struct ZingoWallet {
    lightclient: zingolib::lightclient::LightClient,
}

impl zcash_wallet_interface::Wallet for ZingoWallet {
    fn protocol_id() -> zcash_wallet_interface::ProtocolId {
        protocol_id::protocol_id()
    }

    async fn new_wallet() -> Self {
        todo!()
    }

    type AddServerError = ();

    async fn add_server(&mut self, server_address: String) -> Result<(), Self::AddServerError> {
        todo!()
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
