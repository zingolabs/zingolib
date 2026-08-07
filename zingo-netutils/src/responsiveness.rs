//! The compile-time partition of network operations by responsiveness:
//! [`Critical`] when a user actively awaits the operation, [`NonCritical`]
//! when nobody blocks on it.

use crate::arm_race::LaunchPolicy;
use crate::time::HEDGE_INTERVAL;

/// The most simultaneous connect attempts any acquisition holds in flight.
pub const MAX_PARALLEL_CONNECTS: usize = 3;

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Critical {}
    impl Sealed for super::NonCritical {}
}

/// The responsiveness category of a network operation, named at compile
/// time by every acquisition site.
pub trait Responsiveness: sealed::Sealed {
    /// The value form of this category, for seams a type cannot cross.
    const CLASS: ResponsivenessClass;
}

/// The category of a network operation a user is actively waiting on.
pub struct Critical;

/// The category of a network operation nobody blocks on.
pub struct NonCritical;

impl Responsiveness for Critical {
    const CLASS: ResponsivenessClass = ResponsivenessClass::Critical;
}

impl Responsiveness for NonCritical {
    const CLASS: ResponsivenessClass = ResponsivenessClass::NonCritical;
}

/// The value form of the responsiveness partition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponsivenessClass {
    /// A user actively awaits the operation.
    Critical,
    /// Nobody blocks on the operation.
    NonCritical,
}

impl ResponsivenessClass {
    /// The wire token this class travels as in the proxy binary's argument
    /// grammar.
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

    /// The launch policy an acquisition of this class races under.
    pub fn launch_policy(self) -> LaunchPolicy {
        match self {
            ResponsivenessClass::Critical => LaunchPolicy::Saturating {
                max_parallel: MAX_PARALLEL_CONNECTS,
            },
            ResponsivenessClass::NonCritical => LaunchPolicy::Hedged {
                max_parallel: MAX_PARALLEL_CONNECTS,
                hedge_interval: HEDGE_INTERVAL,
            },
        }
    }
}
