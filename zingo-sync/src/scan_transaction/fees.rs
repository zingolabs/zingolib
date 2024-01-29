//! Fee calculation
//!
//! Library for calculating fees during transaction scanning.
//!
//! Should process fee as a validator would with no need for viewing key or notes.
//! Requires the transactions of all transparent inputs to have been scanned before fee can
//! be calculated this way.
//! May also provide fee verification from the enhanced transaction by summing consumed notes
//! and subtracting outputs if outgoing metadata exists.
