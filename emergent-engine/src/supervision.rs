//! Supervision policy: pure decision logic for restarting exited primitives.
//!
//! Everything in this module is a pure function of its inputs. The actor in
//! [`crate::primitive_actor`] owns the side effects (spawning, signalling,
//! broadcasting); this module only decides *what* should happen.
//!
//! The two decisions are:
//!
//! - [`backoff_delay`]: how long to wait before the next restart attempt.
//! - [`decide_restart`]: whether to restart at all, and with which delay.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// What the engine should do when a primitive's child process exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    /// Never respawn the child. This is the default and preserves the
    /// behaviour the engine had before supervision was added.
    #[default]
    Never,
    /// Respawn only when the child died from a non-zero exit code or a signal.
    OnFailure,
    /// Respawn on any exit, clean or not.
    Always,
}

impl RestartPolicy {
    /// The configuration spelling of this policy.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::OnFailure => "on-failure",
            Self::Always => "always",
        }
    }

    /// Every spelling a configuration file may use, in the order documented.
    #[must_use]
    pub const fn variants() -> [&'static str; 3] {
        ["never", "on-failure", "always"]
    }
}

/// Parse a `restart` configuration value (pure function).
///
/// Returns `None` for any value outside the documented set, so the caller can
/// report an error naming both the primitive and the offending value.
#[must_use]
pub fn parse_restart_policy(value: &str) -> Option<RestartPolicy> {
    match value {
        "never" => Some(RestartPolicy::Never),
        "on-failure" => Some(RestartPolicy::OnFailure),
        "always" => Some(RestartPolicy::Always),
        _ => None,
    }
}

/// How a child process ended (pure classification input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitOutcome {
    /// Exit code 0, or a conventional SIGTERM exit (143 / signal 15).
    Clean,
    /// Any other exit code, or death by another signal.
    Failure,
}

/// Restart pacing limits for a single primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartLimits {
    /// Delay before the first restart attempt, in milliseconds.
    pub backoff_ms: u64,
    /// Ceiling for the exponential backoff, in milliseconds.
    pub max_backoff_ms: u64,
    /// Maximum restarts allowed inside `window_ms`.
    pub max_retries: u32,
    /// Sliding window over which `max_retries` is counted, in milliseconds.
    pub window_ms: u64,
}

impl Default for RestartLimits {
    fn default() -> Self {
        Self {
            backoff_ms: default_restart_backoff_ms(),
            max_backoff_ms: default_restart_max_backoff_ms(),
            max_retries: default_restart_max_retries(),
            window_ms: default_restart_window_ms(),
        }
    }
}

/// Default delay before the first restart attempt.
#[must_use]
pub const fn default_restart_backoff_ms() -> u64 {
    500
}

/// Default ceiling for the exponential backoff.
#[must_use]
pub const fn default_restart_max_backoff_ms() -> u64 {
    30_000
}

/// Default number of restarts allowed inside the window.
#[must_use]
pub const fn default_restart_max_retries() -> u32 {
    5
}

/// Default sliding window for counting restarts.
#[must_use]
pub const fn default_restart_window_ms() -> u64 {
    60_000
}

/// The outcome of a restart decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartDecision {
    /// The policy does not call for a restart of this exit.
    NoRestart,
    /// The engine is shutting down, so nothing is respawned.
    SuppressedByShutdown,
    /// Restart after `delay`. `attempt` is 1 for the first restart.
    Restart {
        /// How long to wait before respawning.
        delay: Duration,
        /// 1-based attempt counter within the current window.
        attempt: u32,
    },
    /// The restart budget for the window is spent; leave the primitive failed.
    Exhausted {
        /// Number of restarts already made inside the window.
        attempts: u32,
        /// The window those attempts fall inside, in milliseconds.
        window_ms: u64,
    },
}

/// Compute the backoff before a given restart attempt (pure function).
///
/// Attempt 1 waits `backoff_ms`, attempt 2 waits `2 * backoff_ms`, attempt 3
/// `4 * backoff_ms`, and so on, saturating at `max_backoff_ms`. Attempt 0 is
/// treated as attempt 1 so callers cannot accidentally produce a zero delay.
#[must_use]
pub fn backoff_delay(attempt: u32, limits: &RestartLimits) -> Duration {
    let step = attempt.max(1) - 1;
    // Shifting by 63 or more is undefined, and the cap is reached long before
    // then, so clamp the exponent first.
    let factor = 1_u64.checked_shl(step.min(62)).unwrap_or(u64::MAX);
    let raw = limits.backoff_ms.saturating_mul(factor);
    Duration::from_millis(raw.min(limits.max_backoff_ms))
}

/// Drop restart timestamps that fall outside the sliding window (pure function).
///
/// Timestamps are Unix milliseconds. A timestamp exactly `window_ms` old is
/// considered outside the window.
#[must_use]
pub fn prune_attempts(attempts: &[u64], now_ms: u64, window_ms: u64) -> Vec<u64> {
    let cutoff = now_ms.saturating_sub(window_ms);
    attempts.iter().copied().filter(|ts| *ts > cutoff).collect()
}

/// Decide whether an exited primitive should be restarted (pure function).
///
/// # Arguments
///
/// * `policy` - the configured restart policy for this primitive.
/// * `outcome` - how the child ended.
/// * `attempts` - Unix-millisecond timestamps of previous restarts.
/// * `limits` - pacing limits for this primitive.
/// * `now_ms` - the current time in Unix milliseconds.
/// * `shutting_down` - whether the engine is tearing the topology down.
///
/// Shutdown always wins: no primitive is respawned while the engine is
/// stopping, whatever its policy says.
#[must_use]
pub fn decide_restart(
    policy: RestartPolicy,
    outcome: ExitOutcome,
    attempts: &[u64],
    limits: &RestartLimits,
    now_ms: u64,
    shutting_down: bool,
) -> RestartDecision {
    if shutting_down {
        return RestartDecision::SuppressedByShutdown;
    }

    let wanted = match (policy, outcome) {
        (RestartPolicy::Never, _) => false,
        (RestartPolicy::OnFailure, ExitOutcome::Clean) => false,
        (RestartPolicy::OnFailure, ExitOutcome::Failure) | (RestartPolicy::Always, _) => true,
    };

    if !wanted {
        return RestartDecision::NoRestart;
    }

    let recent = prune_attempts(attempts, now_ms, limits.window_ms);
    let used = u32::try_from(recent.len()).unwrap_or(u32::MAX);

    if used >= limits.max_retries {
        return RestartDecision::Exhausted {
            attempts: used,
            window_ms: limits.window_ms,
        };
    }

    let attempt = used + 1;
    RestartDecision::Restart {
        delay: backoff_delay(attempt, limits),
        attempt,
    }
}

/// Classify an integer exit code into an [`ExitOutcome`] (pure function).
///
/// Exit code 0 and 143 (the conventional 128+SIGTERM) count as clean, matching
/// the engine's existing treatment of SIGTERM during graceful shutdown.
#[must_use]
pub const fn outcome_from_exit_code(code: i32) -> ExitOutcome {
    if code == 0 || code == 143 {
        ExitOutcome::Clean
    } else {
        ExitOutcome::Failure
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn limits(
        backoff_ms: u64,
        max_backoff_ms: u64,
        max_retries: u32,
        window_ms: u64,
    ) -> RestartLimits {
        RestartLimits {
            backoff_ms,
            max_backoff_ms,
            max_retries,
            window_ms,
        }
    }

    #[test]
    fn parse_restart_policy_accepts_documented_spellings() {
        assert_eq!(parse_restart_policy("never"), Some(RestartPolicy::Never));
        assert_eq!(
            parse_restart_policy("on-failure"),
            Some(RestartPolicy::OnFailure)
        );
        assert_eq!(parse_restart_policy("always"), Some(RestartPolicy::Always));
    }

    #[test]
    fn parse_restart_policy_rejects_anything_else() {
        assert_eq!(parse_restart_policy(""), None);
        assert_eq!(parse_restart_policy("on_failure"), None);
        assert_eq!(parse_restart_policy("Never"), None);
        assert_eq!(parse_restart_policy("sometimes"), None);
    }

    #[test]
    fn policy_as_str_round_trips_through_parse() {
        for spelling in RestartPolicy::variants() {
            let policy = parse_restart_policy(spelling);
            assert_eq!(policy.map(RestartPolicy::as_str), Some(spelling));
        }
    }

    #[test]
    fn default_policy_is_never() {
        assert_eq!(RestartPolicy::default(), RestartPolicy::Never);
    }

    #[test]
    fn outcome_from_exit_code_treats_zero_and_143_as_clean() {
        assert_eq!(outcome_from_exit_code(0), ExitOutcome::Clean);
        assert_eq!(outcome_from_exit_code(143), ExitOutcome::Clean);
    }

    #[test]
    fn outcome_from_exit_code_treats_failures_as_failure() {
        assert_eq!(outcome_from_exit_code(1), ExitOutcome::Failure);
        assert_eq!(outcome_from_exit_code(137), ExitOutcome::Failure); // SIGKILL
        assert_eq!(outcome_from_exit_code(139), ExitOutcome::Failure); // SIGSEGV
        assert_eq!(outcome_from_exit_code(-1), ExitOutcome::Failure);
    }

    #[test]
    fn backoff_doubles_each_attempt() {
        let l = limits(100, 10_000, 10, 60_000);
        assert_eq!(backoff_delay(1, &l), Duration::from_millis(100));
        assert_eq!(backoff_delay(2, &l), Duration::from_millis(200));
        assert_eq!(backoff_delay(3, &l), Duration::from_millis(400));
        assert_eq!(backoff_delay(4, &l), Duration::from_millis(800));
    }

    #[test]
    fn backoff_saturates_at_the_cap() {
        let l = limits(100, 500, 10, 60_000);
        assert_eq!(backoff_delay(4, &l), Duration::from_millis(500));
        assert_eq!(backoff_delay(40, &l), Duration::from_millis(500));
        assert_eq!(backoff_delay(u32::MAX, &l), Duration::from_millis(500));
    }

    #[test]
    fn backoff_treats_attempt_zero_as_the_first_attempt() {
        let l = limits(250, 10_000, 10, 60_000);
        assert_eq!(backoff_delay(0, &l), backoff_delay(1, &l));
    }

    #[test]
    fn backoff_of_zero_stays_zero() {
        let l = limits(0, 10_000, 10, 60_000);
        assert_eq!(backoff_delay(5, &l), Duration::ZERO);
    }

    #[test]
    fn prune_attempts_keeps_only_entries_inside_the_window() {
        let attempts = [1_000_u64, 5_000, 9_000];
        assert_eq!(prune_attempts(&attempts, 10_000, 6_000), vec![5_000, 9_000]);
    }

    #[test]
    fn prune_attempts_drops_an_entry_exactly_at_the_window_edge() {
        let attempts = [4_000_u64];
        assert!(prune_attempts(&attempts, 10_000, 6_000).is_empty());
    }

    #[test]
    fn prune_attempts_survives_a_window_larger_than_now() {
        let attempts = [10_u64, 20];
        assert_eq!(prune_attempts(&attempts, 100, u64::MAX), vec![10, 20]);
    }

    #[test]
    fn never_policy_does_not_restart_on_failure() {
        let d = decide_restart(
            RestartPolicy::Never,
            ExitOutcome::Failure,
            &[],
            &RestartLimits::default(),
            1_000,
            false,
        );
        assert_eq!(d, RestartDecision::NoRestart);
    }

    #[test]
    fn never_policy_does_not_restart_on_clean_exit() {
        let d = decide_restart(
            RestartPolicy::Never,
            ExitOutcome::Clean,
            &[],
            &RestartLimits::default(),
            1_000,
            false,
        );
        assert_eq!(d, RestartDecision::NoRestart);
    }

    #[test]
    fn on_failure_restarts_after_a_failure() {
        let l = limits(500, 30_000, 5, 60_000);
        let d = decide_restart(
            RestartPolicy::OnFailure,
            ExitOutcome::Failure,
            &[],
            &l,
            1_000,
            false,
        );
        assert_eq!(
            d,
            RestartDecision::Restart {
                delay: Duration::from_millis(500),
                attempt: 1,
            }
        );
    }

    #[test]
    fn on_failure_ignores_a_clean_exit() {
        let d = decide_restart(
            RestartPolicy::OnFailure,
            ExitOutcome::Clean,
            &[],
            &RestartLimits::default(),
            1_000,
            false,
        );
        assert_eq!(d, RestartDecision::NoRestart);
    }

    #[test]
    fn always_restarts_a_clean_exit() {
        let l = limits(500, 30_000, 5, 60_000);
        let d = decide_restart(
            RestartPolicy::Always,
            ExitOutcome::Clean,
            &[],
            &l,
            1_000,
            false,
        );
        assert_eq!(
            d,
            RestartDecision::Restart {
                delay: Duration::from_millis(500),
                attempt: 1,
            }
        );
    }

    #[test]
    fn shutdown_suppresses_every_policy() {
        for policy in [
            RestartPolicy::Never,
            RestartPolicy::OnFailure,
            RestartPolicy::Always,
        ] {
            for outcome in [ExitOutcome::Clean, ExitOutcome::Failure] {
                let d =
                    decide_restart(policy, outcome, &[], &RestartLimits::default(), 1_000, true);
                assert_eq!(
                    d,
                    RestartDecision::SuppressedByShutdown,
                    "{policy:?}/{outcome:?}"
                );
            }
        }
    }

    #[test]
    fn attempt_number_grows_with_recorded_history() {
        let l = limits(100, 10_000, 5, 60_000);
        let d = decide_restart(
            RestartPolicy::OnFailure,
            ExitOutcome::Failure,
            &[10_000, 10_500],
            &l,
            11_000,
            false,
        );
        assert_eq!(
            d,
            RestartDecision::Restart {
                delay: Duration::from_millis(400),
                attempt: 3,
            }
        );
    }

    #[test]
    fn restarts_are_exhausted_once_the_budget_is_spent() {
        let l = limits(100, 10_000, 3, 60_000);
        let d = decide_restart(
            RestartPolicy::OnFailure,
            ExitOutcome::Failure,
            &[10_000, 10_500, 10_900],
            &l,
            11_000,
            false,
        );
        assert_eq!(
            d,
            RestartDecision::Exhausted {
                attempts: 3,
                window_ms: 60_000,
            }
        );
    }

    #[test]
    fn attempts_outside_the_window_do_not_count_against_the_budget() {
        let l = limits(100, 10_000, 3, 1_000);
        // Three restarts, but all older than the 1 s window.
        let d = decide_restart(
            RestartPolicy::OnFailure,
            ExitOutcome::Failure,
            &[10_000, 10_100, 10_200],
            &l,
            20_000,
            false,
        );
        assert_eq!(
            d,
            RestartDecision::Restart {
                delay: Duration::from_millis(100),
                attempt: 1,
            }
        );
    }

    #[test]
    fn a_zero_retry_budget_never_restarts() {
        let l = limits(100, 10_000, 0, 60_000);
        let d = decide_restart(
            RestartPolicy::Always,
            ExitOutcome::Failure,
            &[],
            &l,
            1_000,
            false,
        );
        assert_eq!(
            d,
            RestartDecision::Exhausted {
                attempts: 0,
                window_ms: 60_000,
            }
        );
    }

    #[test]
    fn default_limits_match_the_documented_defaults() {
        let l = RestartLimits::default();
        assert_eq!(l.backoff_ms, 500);
        assert_eq!(l.max_backoff_ms, 30_000);
        assert_eq!(l.max_retries, 5);
        assert_eq!(l.window_ms, 60_000);
    }
}
