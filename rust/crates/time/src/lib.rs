//! Monotonic, second-resolution time measured from process start.
//!
//! Two deliberately distinct types:
//!
//! - [`Ttl`] is a **duration** — "1000 seconds from whenever".
//! - [`Time`] is an **instant** — "the 1000th second of process uptime".
//!
//! They do not convert into each other implicitly. Assigning a `Ttl` where a
//! `Time` is expected is a type error rather than an entry that expires at a
//! fixed wall-clock moment regardless of when it was written.
//!
//! # Zero-validity
//!
//! `Time` is `#[repr(transparent)]` over `u32`, and the all-zero bit pattern is
//! [`Time::NEVER`]. This is load-bearing: a `Time` embedded in a zero-initialized
//! `mmap` region reads as "no expiry" with no explicit construction, which is
//! what `XXH3Map::new_mmap` relies on. Changing the representation or the
//! meaning of zero silently breaks that invariant.

use std::sync::OnceLock;
use std::time::Instant;

pub type Seconds = u32;

static START: OnceLock<Instant> = OnceLock::new();

/// Pins the process epoch.
///
/// Call once during startup, before spawning workers. The hot path can then
/// treat the `OnceLock` branch as always-taken and perfectly predicted.
#[inline]
pub fn init() {
    START.get_or_init(Instant::now);
}

#[inline]
fn uptime_secs() -> Seconds {
    let secs = START.get_or_init(Instant::now).elapsed().as_secs();
    Seconds::try_from(secs).unwrap_or(Seconds::MAX)
}

/// A duration in whole seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
#[repr(transparent)]
pub struct Ttl(Seconds);

impl Ttl {
    pub const ZERO: Ttl = Ttl(0);

    #[inline]
    pub const fn from_secs(secs: Seconds) -> Self {
        Self(secs)
    }

    #[inline]
    pub const fn as_secs(self) -> Seconds {
        self.0
    }
}

/// An instant, in whole seconds since process start.
///
/// `Time::default()` and the all-zero bit pattern are both [`Time::NEVER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
#[repr(transparent)]
pub struct Time(Seconds);

impl Time {
    /// "Never expires". Deliberately the zero value — see the module docs.
    pub const NEVER: Time = Time(0);

    /// Raw construction from an uptime-relative second count.
    ///
    /// Named verbosely so it cannot be mistaken for a duration constructor.
    /// To build a deadline, use [`Time::after`] or [`Time::plus`].
    #[inline]
    pub const fn from_uptime_secs(secs: Seconds) -> Self {
        Self(secs)
    }

    #[inline]
    pub const fn as_uptime_secs(self) -> Seconds {
        self.0
    }

    /// Reads the clock.
    ///
    /// This is a `clock_gettime` through the vDSO — on the order of 20ns. Call
    /// it once per event-loop iteration and pass the result down; do not call
    /// it per probe or per entry.
    #[inline]
    pub fn now() -> Self {
        Self(uptime_secs())
    }

    /// The instant `ttl` from now. This is the TTL constructor.
    #[inline]
    pub fn after(ttl: Ttl) -> Self {
        Self::now().plus(ttl)
    }

    /// The instant `ttl` after `self`, saturating at [`Seconds::MAX`].
    ///
    /// The result is never [`Time::NEVER`]: a real deadline of zero is nudged
    /// to 1 so it cannot alias the sentinel.
    #[inline]
    pub const fn plus(self, ttl: Ttl) -> Self {
        let secs = self.0.saturating_add(ttl.0);
        Self(if secs == 0 { 1 } else { secs })
    }

    #[inline]
    pub const fn is_never(self) -> bool {
        self.0 == Time::NEVER.0
    }

    /// Hot path: whether this deadline has passed as of a caller-supplied
    /// `now`, with no clock read. [`Time::NEVER`] is never in the past.
    #[inline]
    pub const fn is_past_at(self, now: Time) -> bool {
        !self.is_never() && self.0 <= now.0
    }

    /// Cold-path convenience. Reads the clock — prefer [`Time::is_past_at`]
    /// anywhere that runs per request.
    #[inline]
    pub fn is_past(self) -> bool {
        self.is_past_at(Time::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards the invariant `XXH3Map::new_mmap` depends on.
    #[test]
    fn zero_bit_pattern_is_never() {
        assert_eq!(std::mem::size_of::<Time>(), 4);
        assert_eq!(unsafe { std::mem::zeroed::<Time>() }, Time::NEVER);
        assert_eq!(Time::default(), Time::NEVER);
        assert!(Time::NEVER.is_never());
    }

    #[test]
    fn never_is_never_past() {
        let far_future = Time::from_uptime_secs(Seconds::MAX);
        assert!(!Time::NEVER.is_past_at(far_future));
    }

    #[test]
    fn deadline_is_relative_to_its_base() {
        let base = Time::from_uptime_secs(100);
        let deadline = base.plus(Ttl::from_secs(50));

        assert_eq!(deadline.as_uptime_secs(), 150);
        assert!(!deadline.is_past_at(Time::from_uptime_secs(149)));
        assert!(deadline.is_past_at(Time::from_uptime_secs(150)));
        assert!(deadline.is_past_at(Time::from_uptime_secs(151)));
    }

    #[test]
    fn deadlines_never_alias_the_sentinel() {
        assert!(!Time::from_uptime_secs(0).plus(Ttl::ZERO).is_never());
        assert_eq!(
            Time::from_uptime_secs(Seconds::MAX).plus(Ttl::from_secs(10)),
            Time::from_uptime_secs(Seconds::MAX)
        );
    }
}
