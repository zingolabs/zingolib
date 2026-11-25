/// A transparent output being created by the transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxOut {
    /// The value of the output, in zatoshis.
    pub value: u64,
    /// The script pubkey that must be satisfied in order to spend this output.
    pub script_pub_key: Vec<u8>,
}
