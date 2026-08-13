//! The compile-time partition of network operations by responsiveness:
//! [`PrioritiseSpeed`] when latency governs the widening and racing wide
//! carries no privacy cost, [`PrioritisePrivacy`] when parsimony outranks
//! latency.

use crate::arm_race::LaunchPolicy;
use crate::time::HEDGE_INTERVAL;

/// The number of exit reservations an acquisition draws to attempt one
/// connection.
pub const RESERVATION_CLUTCH_SIZE: usize = 4;

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::PrioritiseSpeed {}
    impl Sealed for super::PrioritisePrivacy {}
}

/// The declared priority of a network operation, named at compile time by
/// every acquisition site.
pub trait Responsiveness: sealed::Sealed {
    /// The value form of this priority, for seams a type cannot cross.
    ///
    /// ```
    /// use zingo_netutils::responsiveness::{
    ///     PrioritisePrivacy, PrioritiseSpeed, Responsiveness, ResponsivenessClass,
    /// };
    ///
    /// assert_eq!(PrioritiseSpeed::CLASS, ResponsivenessClass::PrioritiseSpeed);
    /// assert_eq!(
    ///     PrioritisePrivacy::CLASS,
    ///     ResponsivenessClass::PrioritisePrivacy
    /// );
    /// ```
    const CLASS: ResponsivenessClass;
}

/// The priority of a network operation where latency governs the widening
/// and racing wide carries no privacy cost.
pub struct PrioritiseSpeed;

/// The priority of a network operation where parsimony outranks latency.
pub struct PrioritisePrivacy;

impl Responsiveness for PrioritiseSpeed {
    const CLASS: ResponsivenessClass = ResponsivenessClass::PrioritiseSpeed;
}

impl Responsiveness for PrioritisePrivacy {
    const CLASS: ResponsivenessClass = ResponsivenessClass::PrioritisePrivacy;
}

/// The value form of the priority partition.
// Preferred iff `adt_const_params` stabilizes — this enum then derives
// `ConstParamTy`, acquisition sites name the class as a const parameter,
// and the marker types retire:
//
//     #[derive(ConstParamTy, Clone, Copy, Debug, PartialEq, Eq)]
//     pub enum ResponsivenessClass { PrioritiseSpeed, PrioritisePrivacy }
//
//     async fn start<const CLASS: ResponsivenessClass>() { /* ... */ }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponsivenessClass {
    /// Latency governs the widening; racing wide carries no privacy cost.
    PrioritiseSpeed,
    /// Parsimony — privacy or load — outranks latency.
    PrioritisePrivacy,
}

impl ResponsivenessClass {
    /// The wire token a [`ResponsivenessClass`] travels as in the proxy
    /// binary's argument grammar.
    ///
    /// ```
    /// use zingo_netutils::responsiveness::ResponsivenessClass;
    ///
    /// for class in [
    ///     ResponsivenessClass::PrioritiseSpeed,
    ///     ResponsivenessClass::PrioritisePrivacy,
    /// ] {
    ///     assert_eq!(ResponsivenessClass::parse(class.wire()), Some(class));
    /// }
    /// assert_eq!(ResponsivenessClass::parse("bogus"), None);
    /// ```
    pub fn wire(self) -> &'static str {
        match self {
            ResponsivenessClass::PrioritiseSpeed => "prioritise-speed",
            ResponsivenessClass::PrioritisePrivacy => "prioritise-privacy",
        }
    }

    /// The class a wire token names, or `None` for an unknown token.
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "prioritise-speed" => Some(ResponsivenessClass::PrioritiseSpeed),
            "prioritise-privacy" => Some(ResponsivenessClass::PrioritisePrivacy),
            _ => None,
        }
    }

    /// The launch policy a [`ResponsivenessClass`]'s Acquisition Race runs
    /// under.
    pub fn launch_policy(self) -> LaunchPolicy {
        match self {
            ResponsivenessClass::PrioritiseSpeed => LaunchPolicy::Saturating {
                max_parallel: RESERVATION_CLUTCH_SIZE,
            },
            ResponsivenessClass::PrioritisePrivacy => LaunchPolicy::Hedged {
                max_parallel: RESERVATION_CLUTCH_SIZE,
                hedge_interval: HEDGE_INTERVAL,
            },
        }
    }
}
