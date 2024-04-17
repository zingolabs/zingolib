use std::io::Write;

use byteorder::{ReadBytesExt, WriteBytesExt};

use zcash_primitives::transaction::{components::OutPoint, TxId};

use super::NoteInterface;

#[derive(Clone, Debug, PartialEq)]
pub struct TransparentNote {
    pub address: String,
    pub txid: TxId,
    pub output_index: u64,
    pub script: Vec<u8>,
    pub value: u64,

    spent: Option<(TxId, u32)>, // If this utxo was confirmed spent Todo: potential data incoherence with unconfirmed_spent

    // If this utxo was spent in a send, but has not yet been confirmed.
    // Contains the txid and height at which the Tx was broadcast
    pub unconfirmed_spent: Option<(TxId, u32)>,
}

impl NoteInterface for TransparentNote {
    fn spent(&self) -> &Option<(TxId, u32)> {
        &self.spent
    }
    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.spent
    }
    fn pending_spent(&self) -> &Option<(TxId, u32)> {
        &self.unconfirmed_spent
    }
    fn pending_spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.unconfirmed_spent
    }
}

impl TransparentNote {
    pub fn from_parts(
        address: String,
        txid: TxId,
        output_index: u64,
        script: Vec<u8>,
        value: u64,
        spent: Option<(TxId, u32)>,
        unconfirmed_spent: Option<(TxId, u32)>,
    ) -> Self {
        Self {
            address,
            txid,
            output_index,
            script,
            value,
            spent,
            unconfirmed_spent,
        }
    }

    pub fn to_outpoint(&self) -> OutPoint {
        OutPoint::new(*self.txid.as_ref(), self.output_index as u32)
    }

    // write + read
    pub fn serialized_version() -> u64 {
        4
    }
    pub fn write<W: Write>(&self, mut writer: W) -> std::io::Result<()> {
        writer.write_u64::<byteorder::LittleEndian>(Self::serialized_version())?;

        writer.write_u32::<byteorder::LittleEndian>(self.address.as_bytes().len() as u32)?;
        writer.write_all(self.address.as_bytes())?;

        writer.write_all(self.txid.as_ref())?;

        writer.write_u64::<byteorder::LittleEndian>(self.output_index)?;
        writer.write_u64::<byteorder::LittleEndian>(self.value)?;
        writer.write_i32::<byteorder::LittleEndian>(0)?;

        let (spent, spent_at_height) = if let Some(spent_tuple) = self.spent {
            (Some(spent_tuple.0), Some(spent_tuple.1 as i32))
        } else {
            (None, None)
        };

        zcash_encoding::Vector::write(&mut writer, &self.script, |w, b| w.write_all(&[*b]))?;

        zcash_encoding::Optional::write(&mut writer, spent, |w, transaction_id| {
            w.write_all(transaction_id.as_ref())
        })?;

        zcash_encoding::Optional::write(&mut writer, spent_at_height, |w, s| {
            w.write_i32::<byteorder::LittleEndian>(s)
        })?;

        Ok(())
    }
    pub fn read<R: std::io::Read>(mut reader: R) -> std::io::Result<Self> {
        let version = reader.read_u64::<byteorder::LittleEndian>()?;

        let address_len = reader.read_i32::<byteorder::LittleEndian>()?;
        let mut address_bytes = vec![0; address_len as usize];
        reader.read_exact(&mut address_bytes)?;
        let address = String::from_utf8(address_bytes).unwrap();
        assert_eq!(address.chars().take(1).collect::<Vec<char>>()[0], 't');

        let mut transaction_id_bytes = [0; 32];
        reader.read_exact(&mut transaction_id_bytes)?;
        let transaction_id = TxId::from_bytes(transaction_id_bytes);

        let output_index = reader.read_u64::<byteorder::LittleEndian>()?;
        let value = reader.read_u64::<byteorder::LittleEndian>()?;
        let _height = reader.read_i32::<byteorder::LittleEndian>()?;

        let script = zcash_encoding::Vector::read(&mut reader, |r| {
            let mut byte = [0; 1];
            r.read_exact(&mut byte)?;
            Ok(byte[0])
        })?;

        let spent = zcash_encoding::Optional::read(&mut reader, |r| {
            let mut transaction_bytes = [0u8; 32];
            r.read_exact(&mut transaction_bytes)?;
            Ok(TxId::from_bytes(transaction_bytes))
        })?;

        let spent_at_height = if version <= 1 {
            None
        } else {
            zcash_encoding::Optional::read(&mut reader, |r| {
                r.read_i32::<byteorder::LittleEndian>()
            })?
        };

        let _unconfirmed_spent = if version == 3 {
            zcash_encoding::Optional::read(&mut reader, |r| {
                let mut transaction_bytes = [0u8; 32];
                r.read_exact(&mut transaction_bytes)?;

                let height = r.read_u32::<byteorder::LittleEndian>()?;
                Ok((TxId::from_bytes(transaction_bytes), height))
            })?
        } else {
            None
        };

        let spent_tuple: Option<(TxId, u32)> = if let Some(txid) = spent {
            if let Some(height) = spent_at_height {
                Some((txid, height as u32))
            } else {
                Some((txid, 0))
            }
        } else {
            None
        };

        Ok(TransparentNote {
            address,
            txid: transaction_id,
            output_index,
            script,
            value,
            spent: spent_tuple,
            unconfirmed_spent: None,
        })
    }
}

#[cfg(any(test, feature = "test-features"))]
pub mod mocks {
    //! Mock version of the struct for testing
    use zcash_primitives::transaction::TxId;

    use crate::{test_framework::mocks::build_method, wallet::notes::TransparentNote};

    /// to create a mock TransparentNote
    pub struct TransparentNoteBuilder {
        address: Option<String>,
        txid: Option<TxId>,
        output_index: Option<u64>,
        script: Option<Vec<u8>>,
        value: Option<u64>,
        spent: Option<Option<(TxId, u32)>>,
        unconfirmed_spent: Option<Option<(TxId, u32)>>,
    }
    #[allow(dead_code)] //TODO:  fix this gross hack that I tossed in to silence the language-analyzer false positive
    impl TransparentNoteBuilder {
        /// blank builder
        pub fn new() -> Self {
            Self {
                address: None,
                txid: None,
                output_index: None,
                script: None,
                value: None,
                spent: None,
                unconfirmed_spent: None,
            }
        }
        // Methods to set each field
        build_method!(address, String);
        build_method!(txid, TxId);
        build_method!(output_index, u64);
        build_method!(script, Vec<u8>);
        build_method!(value, u64);
        build_method!(spent, Option<(TxId, u32)>);
        build_method!(unconfirmed_spent, Option<(TxId, u32)>);

        /// builds a mock TransparentNote after all pieces are supplied
        pub fn build(self) -> TransparentNote {
            TransparentNote::from_parts(
                self.address.unwrap(),
                self.txid.unwrap(),
                self.output_index.unwrap(),
                self.script.unwrap(),
                self.value.unwrap(),
                self.spent.unwrap(),
                self.unconfirmed_spent.unwrap(),
            )
        }
    }

    impl Default for TransparentNoteBuilder {
        fn default() -> Self {
            Self::new()
                .address("default_address".to_string())
                .txid(TxId::from_bytes([0u8; 32]))
                .output_index(0)
                .script(vec![])
                .value(0)
                .spent(None)
                .unconfirmed_spent(None)
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{
        test_framework::mocks::default_txid,
        wallet::notes::{transparent::mocks::TransparentNoteBuilder, NoteInterface},
    };

    #[test]
    fn pending_spent_note_is_pending_spent() {
        let spend = Some((default_txid(), 112358));
        let note = TransparentNoteBuilder::default()
            .unconfirmed_spent(spend)
            .build();
        assert_eq!(note.is_spent(), false);
        assert_eq!(note.is_pending_spent(), true);
        assert_eq!(note.is_spent_or_pending_spent(), true);
        assert_eq!(note.pending_spent(), &spend);
    }
}
