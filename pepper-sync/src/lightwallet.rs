use zcash_primitives::{block::BlockHash, transaction::TxId};
use zcash_protocol::consensus::BlockHeight;

pub(crate) use lightwallet_protocol::*;

pub(crate) trait CompactBlockExt {
    fn hash(&self) -> BlockHash;
    fn height(&self) -> BlockHeight;
}

impl CompactBlockExt for CompactBlock {
    fn hash(&self) -> BlockHash {
        BlockHash::from_slice(&self.hash)
    }

    fn height(&self) -> BlockHeight {
        self.height
            .try_into()
            .expect("block height must fit in u32")
    }
}

pub(crate) trait CompactTxExt {
    fn txid(&self) -> TxId;
}

impl CompactTxExt for CompactTx {
    fn txid(&self) -> TxId {
        let txid: [u8; 32] = self
            .txid
            .as_slice()
            .try_into()
            .expect("txid must be 32 bytes");
        TxId::from_bytes(txid)
    }
}
