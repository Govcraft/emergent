//! Actor-based primitive lifecycle management.
//!
//! Each primitive (Source, Handler, Sink) is managed by a PrimitiveActor that:
//! - Spawns the child process in `after_start` (after message loop is active)
//! - Publishes `system.started.*` events after successful spawn
//! - Terminates the child process in `before_stop`
//! - Publishes `system.stopped.*` events after graceful shutdown
//!
//! This eliminates the race condition where events were published before
//! subscribers had connected.
//!
//! # Design Pattern
//!
//! Since `tokio::process::Child` doesn't implement `Clone` (required by acton messages),
//! we use a PID-based pattern:
//! - Store only the **PID** (u32) in actor state
//! - Keep the Child alive in a **background tokio::spawn task** that monitors it
//! - Use **self-messaging** to update state when child spawns/exits
//! - Use stored **PID for cleanup** (SIGTERM) in `before_stop`

use crate::messages::EmergentMessage;
use crate::primitives::{PrimitiveInfo, PrimitiveState};
use acton_reactive::prelude::*;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitStatus;
use tokio::process::Command;
use tracing::{error, info, warn};

/// Payload for system lifecycle events.
#[derive(Debug, Clone, Serialize)]
pub struct SystemEventPayload {
    /// Name of the primitive.
    pub name: String,
    /// Kind of the primitive (source, handler, sink).
    pub kind: String,
    /// Process ID if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Message types this primitive publishes (Sources and Handlers).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub publishes: Vec<String>,
    /// Message types this primitive subscribes to (Handlers and Sinks).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subscribes: Vec<String>,
    /// Optional error message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SystemEventPayload {
    /// Create a payload for a started event (pure function).
    #[must_use]
    pub fn started(info: &PrimitiveInfo, pid: u32) -> Self {
        Self {
            name: info.name.clone(),
            kind: info.kind.as_str().to_string(),
            pid: Some(pid),
            publishes: info.publishes.clone(),
            subscribes: info.subscribes.clone(),
            error: None,
        }
    }

    /// Create a payload for a stopped event (pure function).
    #[must_use]
    pub fn stopped(info: &PrimitiveInfo, pid: Option<u32>) -> Self {
        Self {
            name: info.name.clone(),
            kind: info.kind.as_str().to_string(),
            pid,
            publishes: info.publishes.clone(),
            subscribes: info.subscribes.clone(),
            error: None,
        }
    }

    /// Create a payload for an error event (pure function).
    #[must_use]
    pub fn error(info: &PrimitiveInfo, pid: Option<u32>, error_msg: String) -> Self {
        Self {
            name: info.name.clone(),
            kind: info.kind.as_str().to_string(),
            pid,
            publishes: info.publishes.clone(),
            subscribes: info.subscribes.clone(),
            error: Some(error_msg),
        }
    }

    /// Create a payload for a shutdown event (pure function).
    #[must_use]
    pub fn shutdown(kind: &str) -> serde_json::Value {
        json!({ "kind": kind })
    }
}

/// Message sent when a child process has been spawned.
///
/// This message carries the PID and primitive info for state initialization.
/// The Child handle is NOT included (doesn't implement Clone).
#[acton_message]
pub struct ChildSpawned {
    /// Process ID of the spawned child.
    pub pid: u32,
    /// Primitive info to initialize actor state.
    pub info: PrimitiveInfo,
}

/// Message sent when a child process has exited.
///
/// Sent by the background monitoring task when `child.wait()` completes.
#[acton_message]
pub struct ChildExited {
    /// Process ID of the exited child.
    pub pid: u32,
    /// Exit status code (-1 if unknown).
    pub status: i32,
}

/// Message to trigger a health check.
#[acton_message]
pub struct HealthCheck;

/// Message to request graceful stop.
#[acton_message]
pub struct StopPrimitive;

/// State for a primitive actor managing a child process.
///
/// Note: We store only the PID, not the Child handle. The Child lives
/// in a background tokio::spawn task that monitors it and sends
/// `ChildExited` messages when it terminates.
///
/// Configuration values (path, args, env, socket_path) are NOT stored
/// in state - they are captured by the lifecycle hook closures.
#[derive(Default, Debug)]
pub struct PrimitiveActorState {
    /// Information about the primitive (name, kind, state, etc.).
    pub info: PrimitiveInfo,
    /// Process ID of the child (if running).
    pub child_pid: Option<u32>,
}

/// Configuration for building a primitive actor.
#[derive(Clone)]
pub struct PrimitiveActorConfig {
    /// Primitive information (name, kind, publishes, subscribes).
    pub info: PrimitiveInfo,
    /// Path to the executable.
    pub path: PathBuf,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Socket path for IPC connections.
    pub socket_path: PathBuf,
}

/// Build and configure a primitive actor.
///
/// The actor will:
/// - Spawn the child process in `after_start`
/// - Broadcast `system.started.<name>` after successful spawn
/// - Handle `ChildSpawned` messages to track the PID
/// - Handle `ChildExited` messages when the child terminates
/// - Terminate the child via SIGTERM in `before_stop`
/// - Broadcast `system.stopped.<name>` after shutdown
pub fn build_primitive_actor(
    runtime: &mut ActorRuntime,
    config: PrimitiveActorConfig,
) -> ManagedActor<Idle, PrimitiveActorState> {
    let name = config.info.name.clone();
    let path = config.path.clone();
    let args = config.args.clone();
    let env = config.env.clone();
    let socket_path = config.socket_path.clone();

    // Create actor with default state - info will be initialized via ChildSpawned message
    let mut actor = runtime.new_actor_with_name::<PrimitiveActorState>(name.clone());

    // Clone info for the ChildSpawned message
    let spawn_info = config.info.clone();

    // Clone values for the after_start closure
    let after_start_name = name.clone();
    let after_start_path = path;
    let after_start_args = args;
    let after_start_env = env;
    let after_start_socket = socket_path;

    actor
        .after_start(move |actor| {
            // Clone spawn_info for use in the async block
            let spawn_info = spawn_info.clone();
            let self_handle = actor.handle().clone();
            let broker = actor.broker().clone();
            let name = after_start_name.clone();
            let path = after_start_path.clone();
            let args = after_start_args.clone();
            let env = after_start_env.clone();
            let socket_path = after_start_socket.clone();

            async move {
                // Build the command
                let mut cmd = Command::new(&path);
                cmd.args(&args);

                // Add environment variables
                for (key, value) in &env {
                    cmd.env(key, value);
                }

                // Add standard environment variables
                cmd.env("EMERGENT_SOCKET", socket_path.to_string_lossy().as_ref());
                cmd.env("EMERGENT_NAME", &name);

                // Isolate child from terminal SIGINT - only engine handles Ctrl+C
                // Children get their own process group so Ctrl+C only affects the engine
                #[cfg(unix)]
                cmd.process_group(0);

                // Spawn the child process
                match cmd.spawn() {
                    Ok(mut child) => {
                        let pid = child.id();
                        info!("Started {} (pid: {:?})", name, pid);

                        if let Some(pid) = pid {
                            // Store PID and info via self-message
                            let mut info = spawn_info.clone();
                            info.pid = Some(pid);
                            info.state = PrimitiveState::Running;
                            self_handle.send(ChildSpawned { pid, info }).await;

                            // Broadcast system.started event
                            let event =
                                create_system_event("system.started", &spawn_info, Some(pid), None);
                            broker.broadcast(event).await;

                            // Monitor child in BACKGROUND TASK
                            // The Child handle lives HERE, not in actor state
                            let monitor_handle = self_handle.clone();
                            let monitor_info = spawn_info.clone();
                            let monitor_broker = broker.clone();
                            tokio::spawn(async move {
                                match child.wait().await {
                                    Ok(status) => {
                                        let exit_code = exit_code_from_status(&status);
                                        let clean = is_clean_exit(&status);
                                        info!(
                                            "{} exited with status {} (pid: {})",
                                            monitor_info.name, exit_code, pid
                                        );

                                        // Notify actor of exit
                                        monitor_handle
                                            .send(ChildExited {
                                                pid,
                                                status: exit_code,
                                            })
                                            .await;

                                        // Broadcast exit event
                                        let event_type = if clean {
                                            "system.stopped"
                                        } else {
                                            "system.error"
                                        };
                                        let error_msg = if clean {
                                            None
                                        } else {
                                            Some(format!("Exited with status: {}", exit_code))
                                        };
                                        let event = create_system_event(
                                            event_type,
                                            &monitor_info,
                                            Some(pid),
                                            error_msg,
                                        );
                                        monitor_broker.broadcast(event).await;
                                    }
                                    Err(e) => {
                                        error!("Error waiting for {}: {}", monitor_info.name, e);
                                    }
                                }
                            });
                        } else {
                            error!("Failed to get PID for {}", name);
                        }
                    }
                    Err(e) => {
                        error!("Failed to spawn {}: {}", name, e);

                        // Broadcast system.error event
                        let event = create_system_event(
                            "system.error",
                            &spawn_info,
                            None,
                            Some(e.to_string()),
                        );
                        broker.broadcast(event).await;
                    }
                }
            }
        })
        .before_stop(move |actor| {
            let name = actor.model.info.name.clone();
            let pid = actor.model.child_pid;

            async move {
                info!("Stopping {}", name);

                // Use stored PID to send SIGTERM - no Mutex needed!
                // The monitor task will broadcast system.stopped when child exits
                if let Some(pid) = pid {
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{Signal, kill};
                        use nix::unistd::Pid;

                        if let Err(e) = kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
                            warn!("Failed to send SIGTERM to {} (pid: {}): {}", name, pid, e);
                        } else {
                            info!("Sent SIGTERM to {} (pid: {})", name, pid);
                        }
                    }

                    #[cfg(not(unix))]
                    {
                        warn!("SIGTERM not supported on this platform for {}", name);
                    }
                }
            }
        })
        .mutate_on::<ChildSpawned>(|actor, envelope| {
            let msg = envelope.message();
            // Initialize actor state with info from the message
            actor.model.child_pid = Some(msg.pid);
            actor.model.info = msg.info.clone();
            Reply::ready()
        })
        .mutate_on::<ChildExited>(|actor, envelope| {
            let msg = envelope.message();
            if actor.model.child_pid == Some(msg.pid) {
                actor.model.child_pid = None;
                actor.model.info.pid = None;
                if is_clean_exit_code(msg.status) {
                    actor.model.info.state = PrimitiveState::Stopped;
                } else {
                    actor.model.info.state = PrimitiveState::Failed;
                    actor.model.info.error = Some(format!("Exited with status: {}", msg.status));
                }
            }
            Reply::ready()
        })
        .act_on::<HealthCheck>(|actor, _envelope| {
            // Health check is now passive - we're notified via ChildExited
            // This handler can be used for explicit status queries
            info!(
                "Health check for {}: state={:?}, pid={:?}",
                actor.model.info.name, actor.model.info.state, actor.model.child_pid
            );
            Reply::ready()
        })
        .mutate_on::<StopPrimitive>(|actor, _envelope| {
            let name = actor.model.info.name.clone();

            if let Some(pid) = actor.model.child_pid {
                actor.model.info.state = PrimitiveState::Stopping;

                // Send SIGTERM on Unix
                #[cfg(unix)]
                {
                    use nix::sys::signal::{Signal, kill};
                    use nix::unistd::Pid;

                    let nix_pid = Pid::from_raw(pid as i32);
                    if let Err(e) = kill(nix_pid, Signal::SIGTERM) {
                        warn!("Failed to send SIGTERM to {}: {}", name, e);
                    }
                }

                // On Windows, we'd need a different approach
                #[cfg(not(unix))]
                {
                    warn!("SIGTERM not supported on this platform for {}", name);
                }
            }

            Reply::ready()
        });

    actor
}

/// Check whether an exit status represents a clean shutdown.
///
/// Returns `true` for:
/// - Exit code 0 (normal success)
/// - Exit code 143 (process caught SIGTERM and exited with 128+15)
/// - Killed by signal 15/SIGTERM (process did not catch the signal)
///
/// This prevents SIGTERM-killed processes from being logged as errors
/// during engine-initiated graceful shutdown.
#[must_use]
fn is_clean_exit(status: &ExitStatus) -> bool {
    if status.success() {
        return true;
    }

    // Process caught SIGTERM and exited with conventional status 128+15=143
    if status.code() == Some(143) {
        return true;
    }

    // On Unix, process was killed directly by SIGTERM (didn't catch the signal)
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if status.signal() == Some(15) {
            return true;
        }
    }

    false
}

/// Determine the exit code to report from an `ExitStatus`.
///
/// On Unix, if the process was killed by a signal (no exit code), the
/// conventional representation 128+signal is returned. Falls back to -1
/// if neither code nor signal is available.
#[must_use]
fn exit_code_from_status(status: &ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }

    -1
}

/// Check whether an integer exit code represents a clean shutdown.
///
/// This is the integer-based counterpart to [`is_clean_exit`] for use
/// in contexts where only the exit code integer is available (e.g.,
/// the `ChildExited` message handler).
///
/// Returns `true` for exit codes 0 (success) and 143 (SIGTERM: 128+15).
#[must_use]
fn is_clean_exit_code(code: i32) -> bool {
    code == 0 || code == 143
}

/// Create a system event message wrapped for IPC.
fn create_system_event(
    event_type: &str,
    info: &PrimitiveInfo,
    pid: Option<u32>,
    error: Option<String>,
) -> IpcSystemEvent {
    let payload = match (event_type, error) {
        ("system.started", None) => SystemEventPayload::started(info, pid.unwrap_or(0)),
        ("system.stopped", None) => SystemEventPayload::stopped(info, pid),
        (_, Some(err)) => SystemEventPayload::error(info, pid, err),
        _ => SystemEventPayload::stopped(info, pid),
    };

    let message = EmergentMessage::new(&format!("{}.{}", event_type, info.name))
        .with_source("emergent-engine")
        .with_payload(json!(payload));

    IpcSystemEvent { inner: message }
}

/// Create a shutdown system event wrapped for IPC.
pub fn create_shutdown_event(kind: &str) -> IpcSystemEvent {
    let message = EmergentMessage::new("system.shutdown")
        .with_source("emergent-engine")
        .with_payload(SystemEventPayload::shutdown(kind));

    IpcSystemEvent { inner: message }
}

/// IPC wrapper for system events.
///
/// This allows system events to be broadcast through the acton broker
/// and forwarded to IPC subscribers.
#[acton_message(ipc)]
pub struct IpcSystemEvent {
    /// The wrapped emergent message.
    pub inner: EmergentMessage,
}

impl From<EmergentMessage> for IpcSystemEvent {
    fn from(msg: EmergentMessage) -> Self {
        Self { inner: msg }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_source_info(name: &str, publishes: Vec<String>) -> PrimitiveInfo {
        PrimitiveInfo::source(name, publishes)
    }

    fn make_handler_info(
        name: &str,
        subscribes: Vec<String>,
        publishes: Vec<String>,
    ) -> PrimitiveInfo {
        PrimitiveInfo::handler(name, subscribes, publishes)
    }

    fn make_sink_info(name: &str, subscribes: Vec<String>) -> PrimitiveInfo {
        PrimitiveInfo::sink(name, subscribes)
    }

    #[test]
    fn test_system_event_payload_started() {
        let info = make_source_info("my-source", vec!["timer.tick".to_string()]);
        let payload = SystemEventPayload::started(&info, 1234);

        assert_eq!(payload.name, "my-source");
        assert_eq!(payload.kind, "source");
        assert_eq!(payload.pid, Some(1234));
        assert_eq!(payload.publishes, vec!["timer.tick".to_string()]);
        assert!(payload.subscribes.is_empty());
        assert!(payload.error.is_none());
    }

    #[test]
    fn test_system_event_payload_stopped() {
        let info = make_handler_info(
            "my-handler",
            vec!["timer.tick".to_string()],
            vec!["timer.filtered".to_string()],
        );
        let payload = SystemEventPayload::stopped(&info, Some(5678));

        assert_eq!(payload.name, "my-handler");
        assert_eq!(payload.kind, "handler");
        assert_eq!(payload.pid, Some(5678));
        assert_eq!(payload.subscribes, vec!["timer.tick".to_string()]);
        assert_eq!(payload.publishes, vec!["timer.filtered".to_string()]);
        assert!(payload.error.is_none());
    }

    #[test]
    fn test_system_event_payload_stopped_without_pid() {
        let info = make_sink_info("my-sink", vec!["timer.filtered".to_string()]);
        let payload = SystemEventPayload::stopped(&info, None);

        assert_eq!(payload.name, "my-sink");
        assert_eq!(payload.kind, "sink");
        assert!(payload.pid.is_none());
        assert!(payload.publishes.is_empty());
        assert_eq!(payload.subscribes, vec!["timer.filtered".to_string()]);
        assert!(payload.error.is_none());
    }

    #[test]
    fn test_system_event_payload_error() {
        let info = make_source_info("failing-source", vec!["data.event".to_string()]);
        let payload =
            SystemEventPayload::error(&info, Some(9999), "Connection refused".to_string());

        assert_eq!(payload.name, "failing-source");
        assert_eq!(payload.kind, "source");
        assert_eq!(payload.pid, Some(9999));
        assert_eq!(payload.publishes, vec!["data.event".to_string()]);
        assert_eq!(payload.error, Some("Connection refused".to_string()));
    }

    #[test]
    fn test_system_event_payload_shutdown() {
        let payload = SystemEventPayload::shutdown("handler");

        assert_eq!(payload["kind"], "handler");
    }

    #[test]
    fn test_system_event_payload_is_serializable() -> Result<(), serde_json::Error> {
        let info = make_source_info("test", vec!["test.event".to_string()]);
        let payload = SystemEventPayload::started(&info, 100);
        let json = serde_json::to_string(&payload)?;

        assert!(json.contains("\"name\":\"test\""));
        assert!(json.contains("\"kind\":\"source\""));
        assert!(json.contains("\"pid\":100"));
        assert!(json.contains("\"publishes\":[\"test.event\"]"));
        // error should not be present when None
        assert!(!json.contains("\"error\""));
        // subscribes should not be present when empty (skip_serializing_if)
        assert!(!json.contains("\"subscribes\""));
        Ok(())
    }

    #[test]
    fn test_system_event_payload_includes_both_publishes_and_subscribes()
    -> Result<(), serde_json::Error> {
        let info = make_handler_info(
            "enricher",
            vec!["input.event".to_string()],
            vec!["output.enriched".to_string()],
        );
        let payload = SystemEventPayload::started(&info, 42);
        let json = serde_json::to_string(&payload)?;

        assert!(json.contains("\"publishes\":[\"output.enriched\"]"));
        assert!(json.contains("\"subscribes\":[\"input.event\"]"));
        Ok(())
    }

    #[test]
    fn test_is_clean_exit_code_zero() {
        assert!(is_clean_exit_code(0));
    }

    #[test]
    fn test_is_clean_exit_code_sigterm_143() {
        assert!(is_clean_exit_code(143));
    }

    #[test]
    fn test_is_clean_exit_code_other_nonzero_is_error() {
        assert!(!is_clean_exit_code(1));
        assert!(!is_clean_exit_code(2));
        assert!(!is_clean_exit_code(127));
        assert!(!is_clean_exit_code(137)); // SIGKILL: 128+9
        assert!(!is_clean_exit_code(139)); // SIGSEGV: 128+11
        assert!(!is_clean_exit_code(-1));
    }

    #[cfg(unix)]
    mod unix_exit_status_tests {
        use super::*;
        use std::os::unix::process::ExitStatusExt;

        #[test]
        fn test_is_clean_exit_success() {
            let status = ExitStatus::from_raw(0);
            assert!(is_clean_exit(&status));
        }

        #[test]
        fn test_is_clean_exit_sigterm_signal() {
            // Process killed by SIGTERM (signal 15), raw wait status = signal << 0
            // On Unix, raw status for signal N is just N (no high byte set)
            let status = ExitStatus::from_raw(15);
            assert!(is_clean_exit(&status));
        }

        #[test]
        fn test_is_clean_exit_sigterm_exit_code_143() {
            // Process caught SIGTERM and exited with 143 (128+15)
            // On Unix, raw status for exit code N is N << 8
            let status = ExitStatus::from_raw(143 << 8);
            assert!(is_clean_exit(&status));
        }

        #[test]
        fn test_is_clean_exit_sigkill_is_error() {
            // SIGKILL (signal 9) is not a clean exit
            let status = ExitStatus::from_raw(9);
            assert!(!is_clean_exit(&status));
        }

        #[test]
        fn test_is_clean_exit_nonzero_code_is_error() {
            // Exit code 1 is not clean
            let status = ExitStatus::from_raw(1 << 8);
            assert!(!is_clean_exit(&status));
        }

        #[test]
        fn test_exit_code_from_status_normal() {
            let status = ExitStatus::from_raw(0);
            assert_eq!(exit_code_from_status(&status), 0);
        }

        #[test]
        fn test_exit_code_from_status_exit_code() {
            let status = ExitStatus::from_raw(42 << 8);
            assert_eq!(exit_code_from_status(&status), 42);
        }

        #[test]
        fn test_exit_code_from_status_signal_gives_128_plus_signal() {
            // Signal 15 (SIGTERM) => 128 + 15 = 143
            let status = ExitStatus::from_raw(15);
            assert_eq!(exit_code_from_status(&status), 143);
        }
    }
}
