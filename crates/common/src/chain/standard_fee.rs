/// A backend's ZIP 317 marginal fee report, as returned by `GetStandardFee`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandardFee {
    /// The standard fee in zatoshis per logical action.
    pub fee_zat: u64,
    /// The backend's fee-estimator version, where 0 is the static ZIP 317 marginal fee.
    pub estimator_version: u32,
}
