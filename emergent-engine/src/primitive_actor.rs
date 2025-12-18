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
use crate::primitives::{PrimitiveInfo, PrimitiveKind, PrimitiveState};
use acton_reactive::prelude::*;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
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
    /// Optional error message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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

fn kind_to_string(kind: PrimitiveKind) -> String {
    match kind {
        PrimitiveKind::Source => "source".to_string(),
        PrimitiveKind::Handler => "handler".to_string(),
        PrimitiveKind::Sink => "sink".to_string(),
    }
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
    let kind = config.info.kind;
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
    let after_start_kind = kind;
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
            let kind = after_start_kind;
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
                            let event = create_system_event(
                                "system.started",
                                &name,
                                kind,
                                Some(pid),
                                None,
                            );
                            broker.broadcast(event).await;

                            // Monitor child in BACKGROUND TASK
                            // The Child handle lives HERE, not in actor state
                            let monitor_handle = self_handle.clone();
                            let monitor_name = name.clone();
                            let monitor_kind = kind;
                            let monitor_broker = broker.clone();
                            tokio::spawn(async move {
                                match child.wait().await {
                                    Ok(status) => {
                                        let exit_code = status.code().unwrap_or(-1);
                                        info!(
                                            "{} exited with status {} (pid: {})",
                                            monitor_name, exit_code, pid
                                        );

                                        // Notify actor of exit
                                        monitor_handle
                                            .send(ChildExited {
                                                pid,
                                                status: exit_code,
                                            })
                                            .await;

                                        // Broadcast exit event
                                        let event_type = if status.success() {
                                            "system.stopped"
                                        } else {
                                            "system.error"
                                        };
                                        let error_msg = if status.success() {
                                            None
                                        } else {
                                            Some(format!("Exited with status: {}", exit_code))
                                        };
                                        let event = create_system_event(
                                            event_type,
                                            &monitor_name,
                                            monitor_kind,
                                            Some(pid),
                                            error_msg,
                                        );
                                        monitor_broker.broadcast(event).await;
                                    }
                                    Err(e) => {
                                        error!("Error waiting for {}: {}", monitor_name, e);
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
                            &name,
                            kind,
                            None,
                            Some(e.to_string()),
                        );
                        broker.broadcast(event).await;
                    }
                }
            }
        })
        .before_stop(move |actor| {
            let broker = actor.broker().clone();
            let name = actor.model.info.name.clone();
            let kind = actor.model.info.kind;
            let pid = actor.model.child_pid;

            async move {
                info!("Stopping {}", name);

                // Use stored PID to send SIGTERM - no Mutex needed!
                if let Some(pid) = pid {
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{kill, Signal};
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

                // Broadcast system.stopped event
                let event = create_system_event("system.stopped", &name, kind, pid, None);
                broker.broadcast(event).await;
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
                if msg.status == 0 {
                    actor.model.info.state = PrimitiveState::Stopped;
                } else {
                    actor.model.info.state = PrimitiveState::Failed;
                    actor.model.info.error = Some(format!("Exited with status: {}", msg.status));
                }
            }
            Reply::ready()
        })
        .mutate_on::<HealthCheck>(|actor, _envelope| {
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
                    use nix::sys::signal::{kill, Signal};
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

/// Create a system event message wrapped for IPC.
fn create_system_event(
    event_type: &str,
    name: &str,
    kind: PrimitiveKind,
    pid: Option<u32>,
    error: Option<String>,
) -> IpcSystemEvent {
    let payload = SystemEventPayload {
        name: name.to_string(),
        kind: kind_to_string(kind),
        pid,
        error,
    };

    let message = EmergentMessage::new(&format!("{}.{}", event_type, name))
        .with_source("emergent-engine")
        .with_payload(json!(payload));

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
