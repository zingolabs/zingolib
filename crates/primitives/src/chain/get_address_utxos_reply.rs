use zcash_protocol::TxId;
use zcash_protocol::consensus::BlockHeight;

/// A [`GetAddressUtxosReply`] that failed to parse from its wire representation.
#[derive(Debug, thiserror::Error)]
pub enum InvalidGetAddressUtxosReply {
    /// The txid was not exactly 32 bytes.
    #[error("txid is not 32 bytes (found {0})")]
    InvalidTxidLength(usize),
    /// The height does not fit a [`BlockHeight`].
    #[error("invalid height")]
    InvalidHeight,
}

/// An unspent transparent output, as served by an indexer's address-UTXO lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetAddressUtxosReply {
    /// The transparent address this output pays.
    pub address: String,
    /// The txid of the transaction that created this output.
    pub txid: TxId,
    /// The index of this output within that transaction.
    pub index: i32,
    /// The script pubkey that must be satisfied in order to spend this output.
    pub script: Vec<u8>,
    /// The value of the output, in zatoshis.
    pub value: i64,
    /// The height at which the output was mined.
    pub height: BlockHeight,
}

impl GetAddressUtxosReply {
    /// Parses an address-UTXO reply from its wire parts, rejecting a txid that is not exactly
    /// 32 bytes and a height that does not fit a [`BlockHeight`].
    pub fn from_wire_parts(
        address: String,
        txid: Vec<u8>,
        index: i32,
        script: Vec<u8>,
        value: i64,
        height: u64,
    ) -> Result<Self, InvalidGetAddressUtxosReply> {
        let found = txid.len();
        Ok(Self {
            address,
            txid: TxId::from_bytes(
                txid.try_into()
                    .map_err(|_| InvalidGetAddressUtxosReply::InvalidTxidLength(found))?,
            ),
            index,
            script,
            value,
            height: BlockHeight::try_from(height)
                .map_err(|_| InvalidGetAddressUtxosReply::InvalidHeight)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::GetAddressUtxosReply;
    use zcash_protocol::consensus::BlockHeight;

    /// A height one past `u32::MAX`, the smallest value `BlockHeight::try_from` rejects.
    const TOO_TALL: u64 = u32::MAX as u64 + 1;

    fn valid() -> (String, Vec<u8>, i32, Vec<u8>, i64, u64) {
        ("t1abc".to_string(), vec![0xaa; 32], 0, vec![], 1000, 100)
    }

    #[test]
    fn valid_parts_build() {
        let (address, txid, index, script, value, height) = valid();
        let reply =
            GetAddressUtxosReply::from_wire_parts(address, txid, index, script, value, height)
                .unwrap();
        assert_eq!(reply.value, 1000);
        assert_eq!(reply.height, BlockHeight::from_u32(100));
    }

    #[test]
    fn a_wrong_length_txid_is_rejected() {
        let (address, _, index, script, value, height) = valid();
        assert!(
            GetAddressUtxosReply::from_wire_parts(
                address,
                vec![0xaa; 20],
                index,
                script,
                value,
                height
            )
            .is_err()
        );
    }

    #[test]
    fn a_height_past_u32_max_is_rejected() {
        let (address, txid, index, script, value, _) = valid();
        assert!(
            GetAddressUtxosReply::from_wire_parts(address, txid, index, script, value, TOO_TALL)
                .is_err()
        );
    }
}
