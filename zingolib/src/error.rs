use std::error::Error;
use std::fmt;

use zcash_client_backend::zip321::Zip321Error;
use zcash_primitives::transaction::TxId;

#[derive(Debug)]
pub enum ZingoLibError {
    Error(String), //review! know our errors

    // client startup errors
    NoWalletLocation,
    MetadataUnderflow(String),
    InternalWriteBuffer(std::io::Error),
    WriteFile(std::io::Error),
    EmptySaveBuffer,
    CantReadWallet(std::io::Error),

    // record corruption errors
    NoSuchTxId(TxId),
    NoSuchSaplingOutputInTx(TxId, u32),
    NoSuchOrchardOutputInTx(TxId, u32),
    NoSuchNullifierInTx(TxId),
    MissingOutputIndex(TxId),
    CouldNotDecodeMemo(std::io::Error),

    // spending errors
    ViewkeyCantSpend,
    RequestConstruction(Zip321Error),
    ProposeTransaction(String),
    MissingProposal,
    CalculateTransaction(String),
    CalculatedTransactionEncode(String),
    CalculatedTransactionDecode(String),
    FundShortfall(u64),
    Broadcast(String),
    PartialBroadcast(u64, String),
}

pub type ZingoLibResult<T> = Result<T, ZingoLibError>;

impl ZingoLibError {
    pub fn handle<T>(self) -> ZingoLibResult<T> {
        log::error!("{}", self);
        Err(self)
    }
}

impl std::fmt::Display for ZingoLibError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ZingoLibError::*;
        write!(
            f,
            "ZingoLibError: {}",
            match self {
                Error(string) => format!(
                    "unknown error: {}",
                    string,
                ),
                NoWalletLocation => "No wallet location! (compiled for native rust, wallet location expected)".to_string(),
                MetadataUnderflow(explanation) => format!(
                    "Metadata underflow! Recorded metadata shows greater output than input value. This may be because input notes are prebirthday. {}",
                    explanation,
                ),
                InternalWriteBuffer(err) => format!(
                    "Internal save error! {} ",
                    err,
                ),
                WriteFile(err) => format!(
                    "Could not write to wallet save file. Was this erroneously attempted in mobile?, instead of native save buffer handling? Is there a permission issue? {} ",
                    err,
                ),
                EmptySaveBuffer => "Empty save buffer. probably save_external was called before save_internal_rust. this is handled by save_external.".to_string(),
                CantReadWallet(err) => format!(
                    "Cant read wallet. Corrupt file. Or maybe a backwards version issue? {}",
                    err,
                ),
                NoSuchTxId(txid) => format!(
                    "Cant find TxId {}!",
                    txid,
                ),
                NoSuchSaplingOutputInTx(txid, output_index) => format!(
                    "Cant find note with sapling output_index {} in TxId {}",
                    output_index,
                    txid,
                ),
                NoSuchOrchardOutputInTx(txid, output_index) => format!(
                    "Cant find note with orchard output_index {} in TxId {}",
                    output_index,
                    txid,
                ),
                NoSuchNullifierInTx(txid) => format!(
                    "Cant find that Nullifier in TxId {}",
                    txid,
                ),
                CouldNotDecodeMemo(err) => format!(
                    "Could not decode memo. Zingo plans to support foreign memo formats soon. {}",
                    err,
                ),
                MissingOutputIndex(txid) => format!(
                    "{txid} is missing output_index for note, cannot mark change"
                ),

                ViewkeyCantSpend => "viewkey cannot spend".to_string(),
                RequestConstruction(err) => format!(
                    "transaction request {}",
                    err
                ),
                ProposeTransaction(string) => format!(
                    "propose transaction: {}",
                    string,
                ),
                MissingProposal => "proposal missing. must propose transaction before sending.".to_string(),
                CalculateTransaction(string) => format!(
                    "calculating transaction: {}",
                    string,
                ),
                CalculatedTransactionEncode(string) => format!(
                    "encoding newly created transaction {}",
                    string,
                ),
                CalculatedTransactionDecode(string) => format!(
                    "decoding newly created transaction {}",
                    string,
                ),
                FundShortfall(shortfall) => format!(
                    "Insufficient sendable balance, need {} more zats",
                    shortfall,
                ),
                Broadcast(string) => format!(
                    "Broadcast failed! {}",
                    string,
                ),
                PartialBroadcast(num, string) => format!(
                    "Broadcast {} of multistep transaction failed! {}",
                    num,
                    string,
                ),
            }
        )
    }
}

impl From<ZingoLibError> for String {
    fn from(value: ZingoLibError) -> Self {
        format!("{value}")
    }
}

impl Error for ZingoLibError {}
