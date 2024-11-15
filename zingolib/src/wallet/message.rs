//! TODO: Add Crate Discription Here!
use crate::config::ChainType;
use byteorder::ReadBytesExt;
use bytes::Bytes;
use group::GroupEncoding;
use rand::{rngs::OsRng, CryptoRng, Rng, RngCore};
use sapling_crypto::{
    keys::{EphemeralPublicKey, OutgoingViewingKey},
    note::ExtractedNoteCommitment,
    note_encryption::{try_sapling_note_decryption, PreparedIncomingViewingKey, SaplingDomain},
    value::NoteValue,
    PaymentAddress, Rseed,
};
use std::io::{self, ErrorKind};
use zcash_note_encryption::{
    Domain, EphemeralKeyBytes, NoteEncryption, ShieldedOutput, ENC_CIPHERTEXT_SIZE,
};
use zcash_primitives::{
    consensus::BlockHeight,
    memo::{Memo, MemoBytes},
};

pub struct Message {
    pub to: PaymentAddress,
    pub memo: Memo,
}

impl Message {
    pub fn new(to: PaymentAddress, memo: Memo) -> Self {
        Self { to, memo }
    }

    fn serialized_version() -> u8 {
        1
    }

    fn magic_word() -> String {
        "ZcashOfflineMemo".to_string()
    }

    // Internal method that does the actual encryption
    fn encrypt_message_to<R: RngCore + CryptoRng>(
        &self,
        ovk: Option<OutgoingViewingKey>,
        rng: &mut R,
    ) -> Result<
        (
            ExtractedNoteCommitment,
            EphemeralPublicKey,
            [u8; ENC_CIPHERTEXT_SIZE],
        ),
        String,
    > {
        // 0-value note
        let value = NoteValue::ZERO;

        // Construct the value commitment, used if an OVK was supplied to create out_ciphertext
        let rseed = Rseed::AfterZip212(rng.gen::<[u8; 32]>());

        // 0-value note with the rseed
        let note = self.to.create_note(value, rseed);

        // CMU is used in the out_cuphertext. Technically this is not needed to recover the note
        // by the receiver, but it is needed to recover the note by the sender.
        let cmu = note.cmu();

        // Create the note encryption object
        let ne = NoteEncryption::<SaplingDomain>::new(
            ovk,
            note,
            *MemoBytes::from(self.memo.clone()).as_array(),
        );

        // EPK, which needs to be sent to the receiver.
        // A very awkward unpack-repack here, as EphemeralPublicKey doesn't implement Clone,
        // So in order to get the EPK instead of a reference, we convert it to epk_bytes and back
        let epk = SaplingDomain::epk(&SaplingDomain::epk_bytes(ne.epk())).unwrap();

        // enc_ciphertext is the encrypted note, out_ciphertext is the outgoing cipher text that the
        // sender can recover
        let enc_ciphertext = ne.encrypt_note_plaintext();

        Ok((cmu, epk, enc_ciphertext))
    }

    pub fn encrypt(&self) -> Result<Vec<u8>, String> {
        let mut rng = OsRng;

        // Encrypt To address. We're using a 'NONE' OVK here, so the out_ciphertext is not recoverable.
        let (cmu, epk, enc_ciphertext) = self.encrypt_message_to(None, &mut rng)?;

        // We'll encode the message on the wire as a series of bytes
        // u8 -> serialized version
        // [u8; 32] -> cmu
        // [u8; 32] -> epk
        // [u8; ENC_CIPHERTEXT_SIZE] -> encrypted bytes
        let mut data = vec![];
        data.extend_from_slice(Message::magic_word().as_bytes());
        data.push(Message::serialized_version());
        data.extend_from_slice(&cmu.to_bytes());
        // Main Network is maybe incorrect, but not used in the calculation of epk_bytes
        data.extend_from_slice(&SaplingDomain::epk_bytes(&epk).0);
        data.extend_from_slice(&enc_ciphertext);

        Ok(data)
    }

    pub fn decrypt(data: &[u8], ivk: &PreparedIncomingViewingKey) -> io::Result<Message> {
        if data.len() != 1 + Message::magic_word().len() + 32 + 32 + ENC_CIPHERTEXT_SIZE {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "Incorrect encrypred payload size".to_string(),
            ));
        }

        use std::io::Read as _;
        let mut reader = std::io::Cursor::new(Bytes::from(data.to_vec()));
        let mut magic_word_bytes = vec![0u8; Message::magic_word().len()];
        reader.read_exact(&mut magic_word_bytes)?;
        let read_magic_word = String::from_utf8(magic_word_bytes)
            .map_err(|e| io::Error::new(ErrorKind::InvalidData, format!("{}", e)))?;
        if read_magic_word != Message::magic_word() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "Bad magic words. Wanted:{}, but found {}",
                    Message::magic_word(),
                    read_magic_word
                ),
            ));
        }

        let version = reader.read_u8()?;
        if version > Message::serialized_version() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("Can't read version {}", version),
            ));
        }

        let mut cmu_bytes = [0u8; 32];
        reader.read_exact(&mut cmu_bytes)?;
        let cmu = bls12_381::Scalar::from_bytes(&cmu_bytes);
        if cmu.is_none().into() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "Can't read CMU bytes".to_string(),
            ));
        }

        let mut epk_bytes = [0u8; 32];
        reader.read_exact(&mut epk_bytes)?;
        let epk = jubjub::ExtendedPoint::from_bytes(&epk_bytes);
        if epk.is_none().into() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "Can't read EPK bytes".to_string(),
            ));
        }

        let mut enc_bytes = [0u8; ENC_CIPHERTEXT_SIZE];
        reader.read_exact(&mut enc_bytes)?;

        #[derive(Debug)]
        struct Unspendable {
            cmu_bytes: [u8; 32],
            epk_bytes: [u8; 32],
            enc_bytes: [u8; ENC_CIPHERTEXT_SIZE],
        }

        impl ShieldedOutput<SaplingDomain, ENC_CIPHERTEXT_SIZE> for Unspendable {
            fn ephemeral_key(&self) -> EphemeralKeyBytes {
                EphemeralKeyBytes(self.epk_bytes)
            }
            fn cmstar_bytes(&self) -> [u8; 32] {
                self.cmu_bytes
            }
            fn enc_ciphertext(&self) -> &[u8; ENC_CIPHERTEXT_SIZE] {
                &self.enc_bytes
            }
        }

        // Attempt decryption. We attempt at main_network at 1,000,000 height, but it doesn't
        // really apply, since this note is not spendable anyway, so the rseed and the note itself
        // are not usable.
        match try_sapling_note_decryption(
            ivk,
            &Unspendable {
                cmu_bytes,
                epk_bytes,
                enc_bytes,
            },
            zcash_primitives::transaction::components::sapling::zip212_enforcement(
                &ChainType::Mainnet,
                BlockHeight::from_u32(1_100_000),
            ),
        ) {
            Some((_note, address, memo)) => Ok(Self::new(
                address,
                Memo::from_bytes(&memo).map_err(|_e| {
                    io::Error::new(ErrorKind::InvalidData, "Failed to decrypt".to_string())
                })?,
            )),
            None => Err(io::Error::new(
                ErrorKind::InvalidData,
                "Failed to decrypt".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use ff::Field;
    use zcash_note_encryption::OUT_PLAINTEXT_SIZE;

    use crate::mocks::random_zaddr;

    use super::*;

    #[test]
    fn test_encrpyt_decrypt() {
        let (_, ivk, to) = random_zaddr();

        let msg = Memo::from_bytes("Hello World with some value!".to_string().as_bytes()).unwrap();

        let enc = Message::new(to, msg.clone()).encrypt().unwrap();
        let dec_msg = Message::decrypt(&enc, &ivk).unwrap();

        assert_eq!(dec_msg.memo, msg);
        assert_eq!(dec_msg.to, to);

        // Also attempt decryption with all addresses
        let dec_msg = Message::decrypt(&enc, &ivk).unwrap();
        assert_eq!(dec_msg.memo, msg);
        assert_eq!(dec_msg.to, to);

        // Raw memo of 512 bytes
        let msg = Memo::from_bytes(&[255u8; 512]).unwrap();
        let enc = Message::new(to, msg.clone()).encrypt().unwrap();
        let dec_msg = Message::decrypt(&enc, &ivk).unwrap();

        assert_eq!(dec_msg.memo, msg);
        assert_eq!(dec_msg.to, to);
    }

    #[test]
    fn test_bad_inputs() {
        let (_, ivk1, to1) = random_zaddr();
        let (_, ivk2, _) = random_zaddr();

        let msg = Memo::from_bytes("Hello World with some value!".to_string().as_bytes()).unwrap();

        let enc = Message::new(to1, msg.clone()).encrypt().unwrap();
        let dec_success = Message::decrypt(&enc, &ivk2);
        assert!(dec_success.is_err());

        let dec_success = Message::decrypt(&enc, &ivk1).unwrap();

        assert_eq!(dec_success.memo, msg);
        assert_eq!(dec_success.to, to1);
    }

    #[test]
    fn test_enc_dec_bad_epk_cmu() {
        let mut rng = OsRng;

        let magic_len = Message::magic_word().len();
        let prefix_len = magic_len + 1; // version byte

        let (_, ivk, to) = random_zaddr();
        let msg_str = "Hello World with some value!";
        let msg = Memo::from_bytes(msg_str.to_string().as_bytes()).unwrap();

        let enc = Message::new(to, msg).encrypt().unwrap();

        // Mad magic word
        let mut bad_enc = enc.clone();
        bad_enc.splice(..magic_len, [1u8; 16].to_vec());
        let dec_success = Message::decrypt(&bad_enc, &ivk);
        assert!(dec_success.is_err());

        // Bad version
        let mut bad_enc = enc.clone();
        bad_enc.splice(
            magic_len..magic_len + 1,
            [Message::serialized_version() + 1].to_vec(),
        );
        let dec_success = Message::decrypt(&bad_enc, &ivk);
        assert!(dec_success.is_err());

        // Create a new, random EPK
        let note = to.create_note(
            NoteValue::ZERO,
            Rseed::BeforeZip212(jubjub::Fr::random(&mut rng)),
        );
        let mut random_bytes = [0; OUT_PLAINTEXT_SIZE];
        random_bytes[32..OUT_PLAINTEXT_SIZE]
            .copy_from_slice(&jubjub::Scalar::random(&mut rng).to_bytes());
        let epk_bad = SaplingDomain::epk_bytes(&SaplingDomain::ka_derive_public(
            &note,
            &SaplingDomain::extract_esk(&zcash_note_encryption::OutPlaintextBytes(random_bytes))
                .unwrap(),
        ));

        let mut bad_enc = enc.clone();
        bad_enc.splice(prefix_len..prefix_len + 33, epk_bad.0.to_vec());
        let dec_success = Message::decrypt(&bad_enc, &ivk);
        assert!(dec_success.is_err());

        // Bad CMU should fail
        let mut bad_enc = enc.clone();
        bad_enc.splice(prefix_len + 33..prefix_len + 65, [1u8; 32].to_vec());
        let dec_success = Message::decrypt(&bad_enc, &ivk);
        assert!(dec_success.is_err());

        // Bad EPK and CMU should fail
        let mut bad_enc = enc.clone();
        bad_enc.splice(prefix_len + 1..prefix_len + 33, [0u8; 32].to_vec());
        bad_enc.splice(prefix_len + 33..prefix_len + 65, [1u8; 32].to_vec());
        let dec_success = Message::decrypt(&bad_enc, &ivk);
        assert!(dec_success.is_err());

        // Bad payload 1
        let mut bad_enc = enc.clone();
        bad_enc.splice(prefix_len + 65.., [0u8; ENC_CIPHERTEXT_SIZE].to_vec());
        let dec_success = Message::decrypt(&bad_enc, &ivk);
        assert!(dec_success.is_err());

        // Bad payload 2
        let mut bad_enc = enc.clone();
        bad_enc.reverse();
        let dec_success = Message::decrypt(&bad_enc, &ivk);
        assert!(dec_success.is_err());

        // Bad payload 3
        let c = enc.clone();
        let (bad_enc, _) = c.split_at(bad_enc.len() - 1);
        let dec_success = Message::decrypt(bad_enc, &ivk);
        assert!(dec_success.is_err());

        // Bad payload 4
        let dec_success = Message::decrypt(&[], &ivk);
        assert!(dec_success.is_err());

        // This should finally work.
        let dec_success = Message::decrypt(&enc, &ivk);
        assert!(dec_success.is_ok());
        if let Memo::Text(memo) = dec_success.unwrap().memo {
            assert_eq!(memo.to_string(), msg_str.to_string());
        } else {
            panic!("Wrong memo");
        }
    }

    #[test]
    fn test_individual_bytes() {
        let (_, ivk, to) = random_zaddr();
        let msg_str = "Hello World with some value!";
        let msg = Memo::from_bytes(msg_str.to_string().as_bytes()).unwrap();

        let enc = Message::new(to, msg.clone()).encrypt().unwrap();

        // Replace each individual byte and make sure it breaks. i.e., each byte is important
        for i in 0..enc.len() {
            let byte = enc.get(i).unwrap();
            let mut bad_enc = enc.clone();
            bad_enc.splice(i..i + 1, [!byte].to_vec());

            let dec_success = Message::decrypt(&bad_enc, &ivk);
            assert!(dec_success.is_err());
        }

        let dec_success = Message::decrypt(&enc, &ivk).unwrap();

        assert_eq!(dec_success.memo, msg);
        assert_eq!(dec_success.to, to);
    }
}
