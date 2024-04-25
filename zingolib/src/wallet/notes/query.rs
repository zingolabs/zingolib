//! Contains structs for querying a database about notes.

use derive_more::Constructor;
use getset::Getters;

/// Selects received notes by how they been spent
#[derive(Getters, Constructor, Clone, Copy)]
pub struct OutputSpendStatusQuery {
    /// will the query include unspent notes?
    #[getset(get = "pub")]
    pub unspent: bool,
    /// will the query include pending_spent notes?
    #[getset(get = "pub")]
    pub pending_spent: bool,
    /// will the query include spent notes?
    #[getset(get = "pub")]
    pub spent: bool,
}
impl OutputSpendStatusQuery {
    /// a query that accepts notes of any spent status
    pub fn any() -> Self {
        Self {
            unspent: true,
            pending_spent: true,
            spent: true,
        }
    }
}

/// Selects received notes by pool
#[derive(Getters, Constructor, Clone, Copy)]
pub struct OutputPoolQuery {
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
#[derive(Getters, Constructor, Clone, Copy)]
pub struct OutputQuery {
    /// selects spend status properties
    /// the query is expected to match note with ANY of the specified spend_stati AND ANY of the specified pools
    #[getset(get = "pub")]
    spend_status: OutputSpendStatusQuery,
    /// selects pools
    #[getset(get = "pub")]
    pools: OutputPoolQuery,
}

pub(crate) struct QueryStipulations {
    pub(crate) unspent: bool,
    pub(crate) pending_spent: bool,
    pub(crate) spent: bool,
    pub(crate) transparent: bool,
    pub(crate) sapling: bool,
    pub(crate) orchard: bool,
}
impl QueryStipulations {
    pub(crate) fn stipulate(self) -> OutputQuery {
        OutputQuery::stipulations(
            self.unspent,
            self.pending_spent,
            self.spent,
            self.transparent,
            self.sapling,
            self.orchard,
        )
    }
}
impl OutputQuery {
    /// build a query, specifying each stipulation
    pub fn stipulations(
        unspent: bool,
        pending_spent: bool,
        spent: bool,
        transparent: bool,
        sapling: bool,
        orchard: bool,
    ) -> Self {
        Self::new(
            OutputSpendStatusQuery::new(unspent, pending_spent, spent),
            OutputPoolQuery::new(transparent, sapling, orchard),
        )
    }
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
