//! Contains structs for querying a database about notes.

use getset::Getters;

/// Selects received notes by how they been spent
#[derive(Getters)]
pub struct NoteSpendStatusQuery {
    /// will the query include unspent notes?
    #[getset(get = "pub")]
    unspent: bool,
    /// will the query include pending_spent notes?
    #[getset(get = "pub")]
    pending_spent: bool,
    /// will the query include spent notes?
    #[getset(get = "pub")]
    spent: bool,
}

/// Selects received notes by pool
#[derive(Getters)]
pub struct NotePoolQuery {
    /// will the query include transparent notes? (coins)
    #[getset(get = "pub")]
    transparent: bool,
    /// will the query include sapling notes?
    #[getset(get = "pub")]
    sapling: bool,
    /// will the query include orchard notes?
    #[getset(get = "pub")]
    orchard: bool,
}

/// Selects received notes by any properties
#[derive(Getters)]
pub struct NoteQuery {
    /// selects spend status properties
    #[getset(get = "pub")]
    spend_status: NoteSpendStatusQuery,
    /// selects pools
    #[getset(get = "pub")]
    pools: NotePoolQuery,
}

impl NoteQuery {
    /// will the query include unspent notes?
    pub fn unspent(&self) -> &bool {
        self.spend_status().unspent()
    }
    /// will the query include pending_spent notes?
    pub fn pending_spent(&self) -> &bool {
        self.spend_status().pending_spent()
    }
    /// will the query include spent notes?
    pub fn spent(&self) -> &bool {
        self.spend_status().spent()
    }
    /// will the query include transparent notes? (coins)
    pub fn transparent(&self) -> &bool {
        self.pools().transparent()
    }
    /// will the query include sapling notes?
    pub fn sapling(&self) -> &bool {
        self.pools().sapling()
    }
    /// will the query include orchard notes?
    pub fn orchard(&self) -> &bool {
        self.pools().orchard()
    }
}
