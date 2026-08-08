//! The compile-time partition of network acquisitions by declared priority:
//! [`PrioritiseSpeed`] when the wait is the cost that matters,
//! [`PrioritisePrivacy`] when the exposure is.

use crate::arm_race::LaunchPolicy;
use crate::time::HEDGE_INTERVAL;

/// The number of exit reservations an acquisition draws to attempt one
/// connection.
pub const RESERVATION_CLUTCH_SIZE: usize = 3;

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

/// The priority of an operation whose acquisition buys minimum latency at
/// maximum exposure.
pub struct PrioritiseSpeed;

/// The priority of an operation whose acquisition buys minimum exposure at
/// the cost of patience.
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
    /// The wait is the cost that matters: launch the full width at once.
    PrioritiseSpeed,
    /// The exposure is the cost that matters: hedge one pull at a time.
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
