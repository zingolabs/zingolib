//! The compile-time partition of network operations by responsiveness:
//! [`Critical`] when latency governs the widening and racing wide carries
//! no privacy cost, [`NonCritical`] when parsimony outranks latency.

use crate::arm_race::LaunchPolicy;
use crate::time::HEDGE_INTERVAL;

/// The number of exit reservations an acquisition draws to attempt one
/// connection.
pub const RESERVATION_CLUTCH_SIZE: usize = 3;

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Critical {}
    impl Sealed for super::NonCritical {}
}

/// The responsiveness category of a network operation, named at compile
/// time by every acquisition site.
pub trait Responsiveness: sealed::Sealed {
    /// The value form of this category, for seams a type cannot cross.
    ///
    /// ```
    /// use zingo_netutils::responsiveness::{
    ///     Critical, NonCritical, Responsiveness, ResponsivenessClass,
    /// };
    ///
    /// assert_eq!(Critical::CLASS, ResponsivenessClass::Critical);
    /// assert_eq!(NonCritical::CLASS, ResponsivenessClass::NonCritical);
    /// ```
    const CLASS: ResponsivenessClass;
}

/// The category of a network operation where latency governs and racing
/// wide carries no privacy cost.
pub struct Critical;

/// The category of a network operation where parsimony outranks latency.
pub struct NonCritical;

impl Responsiveness for Critical {
    const CLASS: ResponsivenessClass = ResponsivenessClass::Critical;
}

impl Responsiveness for NonCritical {
    const CLASS: ResponsivenessClass = ResponsivenessClass::NonCritical;
}

/// The value form of the responsiveness partition.
// Preferred iff `adt_const_params` stabilizes — this enum then derives
// `ConstParamTy`, acquisition sites name the class as a const parameter,
// and the marker types retire:
//
//     #[derive(ConstParamTy, Clone, Copy, Debug, PartialEq, Eq)]
//     pub enum ResponsivenessClass { Critical, NonCritical }
//
//     async fn start<const CLASS: ResponsivenessClass>() { /* ... */ }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponsivenessClass {
    /// Latency governs the widening; racing wide carries no privacy cost.
    Critical,
    /// Parsimony — privacy or load — outranks latency.
    NonCritical,
}

impl ResponsivenessClass {
    /// The wire token a [`ResponsivenessClass`] travels as in the proxy
    /// binary's argument grammar.
    ///
    /// ```
    /// use zingo_netutils::responsiveness::ResponsivenessClass;
    ///
    /// for class in [
    ///     ResponsivenessClass::Critical,
    ///     ResponsivenessClass::NonCritical,
    /// ] {
    ///     assert_eq!(ResponsivenessClass::parse(class.wire()), Some(class));
    /// }
    /// assert_eq!(ResponsivenessClass::parse("bogus"), None);
    /// ```
    pub fn wire(self) -> &'static str {
        match self {
            ResponsivenessClass::Critical => "critical",
            ResponsivenessClass::NonCritical => "non-critical",
        }
    }

    /// The class a wire token names, or `None` for an unknown token.
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "critical" => Some(ResponsivenessClass::Critical),
            "non-critical" => Some(ResponsivenessClass::NonCritical),
            _ => None,
        }
    }

    /// The launch policy a [`ResponsivenessClass`]'s Acquisition Race runs
    /// under.
    pub fn launch_policy(self) -> LaunchPolicy {
        match self {
            ResponsivenessClass::Critical => LaunchPolicy::Saturating {
                max_parallel: RESERVATION_CLUTCH_SIZE,
            },
            ResponsivenessClass::NonCritical => LaunchPolicy::Hedged {
                max_parallel: RESERVATION_CLUTCH_SIZE,
                hedge_interval: HEDGE_INTERVAL,
            },
        }
    }
}
