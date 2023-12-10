use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
};
use zcash_encoding::{Optional, Vector};
use zcash_primitives::transaction::TxId;

use crate::wallet::{data::TransactionMetadata, keys::unified::WalletCapability, WitnessTrees};

use super::TransactionMetadataSet;
impl TransactionMetadataSet {
    pub fn serialized_version() -> u64 {
        22
    }

    pub fn read_old<R: Read>(
        mut reader: R,
        wallet_capability: &WalletCapability,
    ) -> io::Result<Self> {
        // Note, witness_trees will be Some(x) if the wallet has spend capability
        // so this check is a very un-ergonomic way of checking if the wallet
        // can spend.
        let mut witness_trees = wallet_capability.get_trees_witness_trees();
        let mut old_inc_witnesses = if witness_trees.is_some() {
            Some((Vec::new(), Vec::new()))
        } else {
            None
        };
        let txs = Vector::read_collected_mut(&mut reader, |r| {
            let mut txid_bytes = [0u8; 32];
            r.read_exact(&mut txid_bytes)?;

            Ok((
                TxId::from_bytes(txid_bytes),
                TransactionMetadata::read(r, (wallet_capability, old_inc_witnesses.as_mut()))
                    .unwrap(),
            ))
        })?;

        if let Some((mut old_sap_wits, mut old_orch_wits)) = old_inc_witnesses {
            old_sap_wits.sort_by(|(_w1, height1), (_w2, height2)| height1.cmp(height2));
            let sap_tree = &mut witness_trees.as_mut().unwrap().witness_tree_sapling;
            for (sap_wit, height) in old_sap_wits {
                sap_tree
                    .insert_witness_nodes(sap_wit, height - 1)
                    .expect("infallible");
                sap_tree.checkpoint(height).expect("infallible");
            }
            old_orch_wits.sort_by(|(_w1, height1), (_w2, height2)| height1.cmp(height2));
            let orch_tree = &mut witness_trees.as_mut().unwrap().witness_tree_orchard;
            for (orch_wit, height) in old_orch_wits {
                orch_tree
                    .insert_witness_nodes(orch_wit, height - 1)
                    .expect("infallible");
                orch_tree.checkpoint(height).expect("infallible");
            }
        }

        Ok(Self {
            current: txs,
            witness_trees,
        })
    }

    pub fn read<R: Read>(mut reader: R, wallet_capability: &WalletCapability) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;
        if version > Self::serialized_version() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Can't read wallettxns because of incorrect version",
            ));
        }

        let mut witness_trees = wallet_capability.get_trees_witness_trees();
        let mut old_inc_witnesses = if witness_trees.is_some() {
            Some((Vec::new(), Vec::new()))
        } else {
            None
        };
        let current: HashMap<_, _> = Vector::read_collected_mut(&mut reader, |r| {
            let mut txid_bytes = [0u8; 32];
            r.read_exact(&mut txid_bytes)?;

            Ok((
                TxId::from_bytes(txid_bytes),
                TransactionMetadata::read(r, (wallet_capability, old_inc_witnesses.as_mut()))?,
            ))
        })?;

        let _mempool: Vec<(TxId, TransactionMetadata)> = if version <= 20 {
            Vector::read_collected_mut(&mut reader, |r| {
                let mut txid_bytes = [0u8; 32];
                r.read_exact(&mut txid_bytes)?;
                let transaction_metadata =
                    TransactionMetadata::read(r, (wallet_capability, old_inc_witnesses.as_mut()))?;

                Ok((TxId::from_bytes(txid_bytes), transaction_metadata))
            })?
        } else {
            vec![]
        };

        if version >= 22 {
            witness_trees = Optional::read(reader, |r| WitnessTrees::read(r))?;
        } else if let Some((mut old_sap_wits, mut old_orch_wits)) = old_inc_witnesses {
            old_sap_wits.sort_by(|(_w1, height1), (_w2, height2)| height1.cmp(height2));
            let sap_tree = &mut witness_trees.as_mut().unwrap().witness_tree_sapling;
            for (sap_wit, height) in old_sap_wits {
                sap_tree
                    .insert_witness_nodes(sap_wit, height - 1)
                    .expect("infallible");
                sap_tree.checkpoint(height).expect("infallible");
            }
            old_orch_wits.sort_by(|(_w1, height1), (_w2, height2)| height1.cmp(height2));
            let orch_tree = &mut witness_trees.as_mut().unwrap().witness_tree_orchard;
            for (orch_wit, height) in old_orch_wits {
                orch_tree
                    .insert_witness_nodes(orch_wit, height - 1)
                    .expect("infallible");
                orch_tree.checkpoint(height).expect("infallible");
            }
        };

        Ok(Self {
            current,
            witness_trees,
        })
    }

    pub async fn write<W: Write>(&mut self, mut writer: W) -> io::Result<()> {
        // Write the version
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        // The hashmap, write as a set of tuples. Store them sorted so that wallets are
        // deterministically saved
        {
            let mut transaction_metadatas = self
                .current
                .iter()
                .collect::<Vec<(&TxId, &TransactionMetadata)>>();
            // Don't write down metadata for transactions in the mempool, we'll rediscover
            // it on reload
            transaction_metadatas.retain(|metadata| metadata.1.status.is_confirmed());
            transaction_metadatas.sort_by(|a, b| a.0.partial_cmp(b.0).unwrap());

            Vector::write(&mut writer, &transaction_metadatas, |w, (k, v)| {
                w.write_all(k.as_ref())?;
                v.write(w)
            })?;
        }

        Optional::write(writer, self.witness_trees.as_mut(), |w, t| t.write(w))
    }
}
