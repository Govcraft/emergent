//! Timestamp newtype for milliseconds since Unix epoch.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// A timestamp in milliseconds since Unix epoch.
///
/// This provides type safety and clear semantics for timestamp values.
/// All timestamps in the Emergent system use this format for consistency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(u64);

impl Timestamp {
    /// Creates a timestamp from milliseconds since Unix epoch.
    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis)
    }

    /// Returns the current system time as a timestamp.
    ///
    /// Returns `Timestamp(0)` if system time is before Unix epoch.
    #[must_use]
    pub fn now() -> Self {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(Self(0), |d| {
                Self(u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            })
    }

    /// Returns the timestamp as milliseconds.
    #[must_use]
    pub const fn as_millis(&self) -> u64 {
        self.0
    }

    /// Returns the timestamp as seconds (truncated).
    #[must_use]
    pub const fn as_secs(&self) -> u64 {
        self.0 / 1000
    }

    /// Returns the duration since another timestamp (if this one is later).
    ///
    /// Returns `None` if `other` is after `self`.
    #[must_use]
    pub const fn duration_since(&self, other: Self) -> Option<u64> {
        if self.0 >= other.0 {
            Some(self.0 - other.0)
        } else {
            None
        }
    }

    /// Adds milliseconds to this timestamp.
    #[must_use]
    pub const fn add_millis(&self, millis: u64) -> Self {
        Self(self.0.saturating_add(millis))
    }

    /// Subtracts milliseconds from this timestamp.
    #[must_use]
    pub const fn sub_millis(&self, millis: u64) -> Self {
        Self(self.0.saturating_sub(millis))
    }
}

impl Default for Timestamp {
    fn default() -> Self {
        Self::now()
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for Timestamp {
    fn from(millis: u64) -> Self {
        Self::from_millis(millis)
    }
}

impl From<Timestamp> for u64 {
    fn from(ts: Timestamp) -> Self {
        ts.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_millis_creates_timestamp() {
        let ts = Timestamp::from_millis(1_704_067_200_000);
        assert_eq!(ts.as_millis(), 1_704_067_200_000);
    }

    #[test]
    fn now_creates_current_timestamp() {
        let ts = Timestamp::now();
        assert!(ts.as_millis() > 0);
    }

    #[test]
    fn as_secs_truncates() {
        let ts = Timestamp::from_millis(5500);
        assert_eq!(ts.as_secs(), 5);
    }

    #[test]
    fn timestamps_are_ordered() {
        let ts1 = Timestamp::from_millis(1000);
        let ts2 = Timestamp::from_millis(2000);
        assert!(ts1 < ts2);
    }

    #[test]
    fn conversion_to_u64() {
        let ts = Timestamp::from_millis(12345);
        let millis: u64 = ts.into();
        assert_eq!(millis, 12345);
    }

    #[test]
    fn conversion_from_u64() {
        let ts: Timestamp = 12345_u64.into();
        assert_eq!(ts.as_millis(), 12345);
    }

    #[test]
    fn duration_since() {
        let ts1 = Timestamp::from_millis(1000);
        let ts2 = Timestamp::from_millis(2000);
        assert_eq!(ts2.duration_since(ts1), Some(1000));
        assert_eq!(ts1.duration_since(ts2), None);
    }

    #[test]
    fn add_millis() {
        let ts = Timestamp::from_millis(1000);
        assert_eq!(ts.add_millis(500).as_millis(), 1500);
    }

    #[test]
    fn sub_millis() {
        let ts = Timestamp::from_millis(1000);
        assert_eq!(ts.sub_millis(500).as_millis(), 500);
    }

    #[test]
    fn sub_millis_saturates() {
        let ts = Timestamp::from_millis(100);
        assert_eq!(ts.sub_millis(500).as_millis(), 0);
    }

    #[test]
    fn serde_roundtrip() {
        let ts = Timestamp::from_millis(1_704_067_200_000);
        let json = serde_json::to_string(&ts).expect("serialize");
        let restored: Timestamp = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ts, restored);
    }

    #[test]
    fn serde_is_transparent() {
        let ts = Timestamp::from_millis(12345);
        let json = serde_json::to_string(&ts).expect("serialize");
        assert_eq!(json, "12345");
    }
}
