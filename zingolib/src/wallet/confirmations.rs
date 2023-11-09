pub enum Confirmations {
    Unconfirmed,
    // confirmed on blockchain implies a height. this data piece will eventually be a block height
    Confirmed(u32),
}
