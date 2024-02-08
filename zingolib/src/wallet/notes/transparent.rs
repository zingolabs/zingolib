use std::io::Write;

use byteorder::{ReadBytesExt, WriteBytesExt};

use zcash_primitives::transaction::{components::OutPoint, TxId};

#[derive(Clone, Debug, PartialEq)]
pub struct TransparentNote {
    pub address: String,
    pub txid: TxId,
    pub output_index: u64,
    pub script: Vec<u8>,
    pub value: u64,

    pub spent_at_height: Option<i32>,
    pub spent: Option<TxId>, // If this utxo was confirmed spent Todo: potential data incoherence with unconfirmed_spent

    // If this utxo was spent in a send, but has not yet been confirmed.
    // Contains the txid and height at which the Tx was broadcast
    pub unconfirmed_spent: Option<(TxId, u32)>,
}

impl TransparentNote {
    pub fn serialized_version() -> u64 {
        4
    }

    pub fn to_outpoint(&self) -> OutPoint {
        OutPoint::new(*self.txid.as_ref(), self.output_index as u32)
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

        Ok(TransparentNote {
            address,
            txid: transaction_id,
            output_index,
            script,
            value,
            spent_at_height,
            spent,
            unconfirmed_spent: None,
        })
    }

    pub fn write<W: Write>(&self, mut writer: W) -> std::io::Result<()> {
        writer.write_u64::<byteorder::LittleEndian>(Self::serialized_version())?;

        writer.write_u32::<byteorder::LittleEndian>(self.address.as_bytes().len() as u32)?;
        writer.write_all(self.address.as_bytes())?;

        writer.write_all(self.txid.as_ref())?;

        writer.write_u64::<byteorder::LittleEndian>(self.output_index)?;
        writer.write_u64::<byteorder::LittleEndian>(self.value)?;
        writer.write_i32::<byteorder::LittleEndian>(0)?;

        zcash_encoding::Vector::write(&mut writer, &self.script, |w, b| w.write_all(&[*b]))?;

        zcash_encoding::Optional::write(&mut writer, self.spent, |w, transaction_id| {
            w.write_all(transaction_id.as_ref())
        })?;

        zcash_encoding::Optional::write(&mut writer, self.spent_at_height, |w, s| {
            w.write_i32::<byteorder::LittleEndian>(s)
        })?;

        Ok(())
    }
}
