//! Responsible for the change of state of notes and transactions.
//!
//! - Iterate through all shielded receipts and check if the notes' nullifier matches a nullifier in
//! an existing transaction in the wallet. If so, create a spend from the receipt or update field.
//!
//! - Iterate through all shielded spends and check if the notes' nullifier matches a nullifier in
//! an existing transaction in the wallet. If not, create a receipt from the spend or update field.
//!
//! - Iterate through all transparent receipts and check if they match the transparent inputs in
//! existing transactions in the wallet (transparent input mapper?). If so, create a spend from the
//! receipt or update field.
//!
//! - Iterate through all transparent spends and check if they match the transparent inputs in
//! existing transactions in the wallet. If not, create a receipt from the spend or update field.
//!
//! - Iterate through basic transactions and check if all notes consumed in transaction are in the
//! wallet. If so, create an enhanced transaction from the basic transaction or update fields.
//!
//! - Iterate through enhanced transactions and check if all notes consumed in transaction are in the
//! wallet. If not, create a basic transaction from the enhanced transaction or update fields.
//! This might not be neccessary but maybe some weird edge case?
