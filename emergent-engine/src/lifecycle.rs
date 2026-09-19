//! The pure lifecycle state machine for a managed primitive.
//!
//! A primitive's live status is a function of the events its actor observes:
//! a spawn attempt, a spawned child, a failed spawn, a stop request, and an
//! exit. [`next_status`] computes the next status from the current one and an
//! event, with no IO and no interior state, so the transitions can be tested
//! on their own.
//!
//! The actor is the single owner of this status. It applies each event as it
//! happens and publishes the result, which is what `GET /api/topology` and
//! `system.response.topology` report.

use crate::primitives::PrimitiveState;

/// The part of a primitive's information that changes while it runs.
///
/// `state`, `pid` and `error` move together: a running primitive has a pid and
/// no error, a failed one has an error and no pid. Keeping them in one value
/// makes every transition set all three, so they cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrimitiveStatus {
    /// Current lifecycle state.
    pub state: PrimitiveState,
    /// Process ID of the child, when one is running.
    pub pid: Option<u32>,
    /// Why the primitive failed, when it did.
    pub error: Option<String>,
}

impl PrimitiveStatus {
    /// The status of a primitive that is registered but has not been spawned.
    #[must_use]
    pub fn configured() -> Self {
        Self {
            state: PrimitiveState::Configured,
            pid: None,
            error: None,
        }
    }

    /// The status of a running primitive with the given pid.
    #[must_use]
    pub fn running(pid: u32) -> Self {
        Self {
            state: PrimitiveState::Running,
            pid: Some(pid),
            error: None,
        }
    }
}

/// Something that happened to a primitive's child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// The actor is about to spawn the child process.
    SpawnRequested,
    /// The child process started under this pid.
    Spawned {
        /// Process ID reported by the operating system.
        pid: u32,
    },
    /// The child process could not be spawned.
    SpawnFailed {
        /// Why the spawn failed.
        error: String,
    },
    /// The restart policy gave up on the primitive.
    RestartsExhausted {
        /// Why no further restart will be attempted.
        reason: String,
    },
    /// The engine asked the primitive to stop and signalled the child.
    StopRequested,
    /// The child process exited.
    Exited {
        /// Process ID that exited.
        pid: u32,
        /// Exit code, using 128 + signal for a signalled exit.
        code: i32,
    },
}

/// Compute the status a primitive has after observing `event`.
///
/// The function is total and pure: every event maps to a status, and an event
/// that does not apply leaves the status untouched. Two cases are deliberate:
///
/// - An [`Exited`](LifecycleEvent::Exited) whose pid is not the one currently
///   running is ignored. A restarted primitive has a new pid, and the monitor
///   task of the previous child can report its exit afterwards.
/// - A [`StopRequested`](LifecycleEvent::StopRequested) with no child running
///   leaves the status alone, so a stop cannot move an already stopped or
///   failed primitive back to `Stopping`.
///
/// A restart is therefore representable: `Running` to `Failed`, through
/// `Starting` while the backoff runs, and back to `Running` under a new pid,
/// clearing the error along the way. When the restart policy gives up,
/// [`RestartsExhausted`](LifecycleEvent::RestartsExhausted) replaces the last
/// exit's error with the reason, so the topology says why the primitive is
/// staying down.
#[must_use]
pub fn next_status(current: &PrimitiveStatus, event: &LifecycleEvent) -> PrimitiveStatus {
    match event {
        LifecycleEvent::SpawnRequested => PrimitiveStatus {
            state: PrimitiveState::Starting,
            pid: None,
            error: None,
        },
        LifecycleEvent::Spawned { pid } => PrimitiveStatus::running(*pid),
        LifecycleEvent::SpawnFailed { error } => PrimitiveStatus {
            state: PrimitiveState::Failed,
            pid: None,
            error: Some(error.clone()),
        },
        LifecycleEvent::RestartsExhausted { reason } => PrimitiveStatus {
            state: PrimitiveState::Failed,
            pid: None,
            error: Some(reason.clone()),
        },
        LifecycleEvent::StopRequested => {
            if current.pid.is_none() {
                return current.clone();
            }
            PrimitiveStatus {
                state: PrimitiveState::Stopping,
                pid: current.pid,
                error: current.error.clone(),
            }
        }
        LifecycleEvent::Exited { pid, code } => {
            if current.pid != Some(*pid) {
                return current.clone();
            }
            if is_clean_exit_code(*code) {
                PrimitiveStatus {
                    state: PrimitiveState::Stopped,
                    pid: None,
                    error: None,
                }
            } else {
                PrimitiveStatus {
                    state: PrimitiveState::Failed,
                    pid: None,
                    error: Some(exit_error_message(*code)),
                }
            }
        }
    }
}

/// Check whether an integer exit code represents a clean shutdown.
///
/// Returns `true` for exit codes 0 (success) and 143 (SIGTERM: 128+15).
#[must_use]
pub fn is_clean_exit_code(code: i32) -> bool {
    code == 0 || code == 143
}

/// The error string reported for a primitive that exited badly.
#[must_use]
pub fn exit_error_message(code: i32) -> String {
    format!("Exited with status: {code}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_request_moves_a_configured_primitive_to_starting() {
        let status = next_status(
            &PrimitiveStatus::configured(),
            &LifecycleEvent::SpawnRequested,
        );
        assert_eq!(status.state, PrimitiveState::Starting);
        assert_eq!(status.pid, None);
        assert_eq!(status.error, None);
    }

    #[test]
    fn exhausted_restarts_leave_the_primitive_failed_with_the_reason() {
        let crashed = next_status(
            &PrimitiveStatus::running(7),
            &LifecycleEvent::Exited { pid: 7, code: 1 },
        );
        let status = next_status(
            &crashed,
            &LifecycleEvent::RestartsExhausted {
                reason: "Restarts exhausted: 3 attempts within 60000 ms".to_string(),
            },
        );
        assert_eq!(status.state, PrimitiveState::Failed);
        assert_eq!(status.pid, None);
        assert_eq!(
            status.error.as_deref(),
            Some("Restarts exhausted: 3 attempts within 60000 ms")
        );
    }

    #[test]
    fn a_scheduled_restart_reads_as_starting_until_the_new_pid_arrives() {
        let crashed = next_status(
            &PrimitiveStatus::running(7),
            &LifecycleEvent::Exited { pid: 7, code: 1 },
        );
        let waiting = next_status(&crashed, &LifecycleEvent::SpawnRequested);
        assert_eq!(waiting.state, PrimitiveState::Starting);
        assert_eq!(waiting.pid, None);
        let back = next_status(&waiting, &LifecycleEvent::Spawned { pid: 8 });
        assert_eq!(back, PrimitiveStatus::running(8));
    }

    #[test]
    fn spawn_records_the_pid_and_runs() {
        let status = next_status(
            &PrimitiveStatus::configured(),
            &LifecycleEvent::Spawned { pid: 4242 },
        );
        assert_eq!(status, PrimitiveStatus::running(4242));
    }

    #[test]
    fn spawn_failure_reports_the_error_without_a_pid() {
        let status = next_status(
            &PrimitiveStatus::configured(),
            &LifecycleEvent::SpawnFailed {
                error: "No such file or directory (os error 2)".to_string(),
            },
        );
        assert_eq!(status.state, PrimitiveState::Failed);
        assert_eq!(status.pid, None);
        assert_eq!(
            status.error.as_deref(),
            Some("No such file or directory (os error 2)")
        );
    }

    #[test]
    fn stop_request_keeps_the_pid_while_the_child_is_signalled() {
        let status = next_status(
            &PrimitiveStatus::running(99),
            &LifecycleEvent::StopRequested,
        );
        assert_eq!(status.state, PrimitiveState::Stopping);
        assert_eq!(status.pid, Some(99));
    }

    #[test]
    fn stop_request_without_a_child_changes_nothing() {
        let stopped = PrimitiveStatus {
            state: PrimitiveState::Stopped,
            pid: None,
            error: None,
        };
        assert_eq!(
            next_status(&stopped, &LifecycleEvent::StopRequested),
            stopped
        );

        let configured = PrimitiveStatus::configured();
        assert_eq!(
            next_status(&configured, &LifecycleEvent::StopRequested),
            configured
        );
    }

    #[test]
    fn clean_exit_stops_the_primitive_and_clears_the_pid() {
        let status = next_status(
            &PrimitiveStatus::running(7),
            &LifecycleEvent::Exited { pid: 7, code: 0 },
        );
        assert_eq!(status.state, PrimitiveState::Stopped);
        assert_eq!(status.pid, None);
        assert_eq!(status.error, None);
    }

    #[test]
    fn sigterm_exit_counts_as_a_clean_stop() {
        let status = next_status(
            &PrimitiveStatus::running(7),
            &LifecycleEvent::Exited { pid: 7, code: 143 },
        );
        assert_eq!(status.state, PrimitiveState::Stopped);
    }

    #[test]
    fn exit_after_a_stop_request_is_still_a_clean_stop() {
        let stopping = next_status(&PrimitiveStatus::running(7), &LifecycleEvent::StopRequested);
        let status = next_status(&stopping, &LifecycleEvent::Exited { pid: 7, code: 143 });
        assert_eq!(status.state, PrimitiveState::Stopped);
        assert_eq!(status.pid, None);
    }

    #[test]
    fn bad_exit_fails_the_primitive_with_the_code_in_the_error() {
        let status = next_status(
            &PrimitiveStatus::running(7),
            &LifecycleEvent::Exited { pid: 7, code: 2 },
        );
        assert_eq!(status.state, PrimitiveState::Failed);
        assert_eq!(status.pid, None);
        assert_eq!(status.error.as_deref(), Some("Exited with status: 2"));
    }

    #[test]
    fn an_exit_from_an_earlier_incarnation_is_ignored() {
        // A restarted primitive runs under a new pid. The monitor task of the
        // previous child can report its exit afterwards, and must not knock
        // the live one out of Running.
        let running = PrimitiveStatus::running(200);
        let status = next_status(&running, &LifecycleEvent::Exited { pid: 100, code: 1 });
        assert_eq!(status, running);
    }

    #[test]
    fn a_restart_returns_to_running_under_a_new_pid_and_clears_the_error() {
        let failed = next_status(
            &PrimitiveStatus::running(100),
            &LifecycleEvent::Exited { pid: 100, code: 1 },
        );
        assert_eq!(failed.state, PrimitiveState::Failed);

        let starting = next_status(&failed, &LifecycleEvent::SpawnRequested);
        assert_eq!(starting.state, PrimitiveState::Starting);
        assert_eq!(starting.error, None);

        let running = next_status(&starting, &LifecycleEvent::Spawned { pid: 201 });
        assert_eq!(running, PrimitiveStatus::running(201));
        assert_eq!(running.error, None);
    }

    #[test]
    fn is_clean_exit_code_zero() {
        assert!(is_clean_exit_code(0));
    }

    #[test]
    fn is_clean_exit_code_sigterm_143() {
        assert!(is_clean_exit_code(143));
    }

    #[test]
    fn is_clean_exit_code_other_nonzero_is_error() {
        assert!(!is_clean_exit_code(1));
        assert!(!is_clean_exit_code(2));
        assert!(!is_clean_exit_code(127));
        assert!(!is_clean_exit_code(137)); // SIGKILL: 128+9
        assert!(!is_clean_exit_code(139)); // SIGSEGV: 128+11
        assert!(!is_clean_exit_code(-1));
    }

    #[test]
    fn exit_error_message_names_the_code() {
        assert_eq!(exit_error_message(9), "Exited with status: 9");
    }
}
