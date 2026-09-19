//! Actor-based primitive lifecycle management.
//!
//! Each primitive (Source, Handler, Sink) is managed by a PrimitiveActor that:
//! - Spawns the child process in `after_start` (after message loop is active)
//! - Publishes `system.started.*` events after successful spawn
//! - Applies the configured restart policy when the child exits, publishing
//!   `system.restarted.*` on a successful respawn
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

use crate::lifecycle::{LifecycleEvent, PrimitiveStatus, exit_error_message, next_status};
use crate::messages::EmergentMessage;
use crate::primitives::PrimitiveInfo;
use crate::supervision::{
    RestartDecision, RestartLimits, RestartPolicy, decide_restart, outcome_from_exit_code,
    prune_attempts,
};
use crate::topology::ENGINE_PRIMITIVE_NAME;
use acton_reactive::prelude::*;
use emergent_client::types::{InvalidMessageType, PrimitiveName, Timestamp};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

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
    /// Restart attempt number, present only on `system.restarted.<name>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_attempt: Option<u32>,
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
            restart_attempt: None,
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
            restart_attempt: None,
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
            restart_attempt: None,
        }
    }

    /// Create a payload for a restarted event (pure function).
    #[must_use]
    pub fn restarted(info: &PrimitiveInfo, pid: u32, attempt: u32) -> Self {
        Self {
            name: info.name.clone(),
            kind: info.kind.as_str().to_string(),
            pid: Some(pid),
            publishes: info.publishes.clone(),
            subscribes: info.subscribes.clone(),
            error: None,
            restart_attempt: Some(attempt),
        }
    }

    /// Create a payload for a shutdown event (pure function).
    #[must_use]
    pub fn shutdown(kind: &str) -> serde_json::Value {
        json!({ "kind": kind })
    }
}

/// Single-writer view of a primitive's live child PID.
///
/// The actor writes to it (`Some(pid)` on spawn, `None` on exit); the process
/// manager reads and awaits it. This is what lets shutdown wait on the actual
/// child instead of sleeping a fixed interval, and it gives the manager the
/// PID it needs to escalate to SIGKILL.
#[derive(Clone, Debug)]
pub struct ChildPidWatch {
    tx: Arc<watch::Sender<Option<u32>>>,
}

impl Default for ChildPidWatch {
    fn default() -> Self {
        Self::new().0
    }
}

impl ChildPidWatch {
    /// Create a writer and its paired reader.
    #[must_use]
    pub fn new() -> (Self, watch::Receiver<Option<u32>>) {
        let (tx, rx) = watch::channel(None);
        (Self { tx: Arc::new(tx) }, rx)
    }

    /// Record that a child with this PID is now running.
    pub fn set(&self, pid: u32) {
        let _ = self.tx.send(Some(pid));
    }

    /// Record that no child is running.
    pub fn clear(&self) {
        let _ = self.tx.send(None);
    }
}

/// Report whether every watched child has exited, within a time budget.
///
/// Returns the primitives still running when the budget expired, with the PID
/// to escalate against. An empty result means the phase drained on its own and
/// the caller can move on immediately.
pub async fn wait_for_children_exit(
    watched: &mut [(String, watch::Receiver<Option<u32>>)],
    budget: std::time::Duration,
) -> Vec<(String, u32)> {
    let deadline = tokio::time::Instant::now() + budget;

    for (_, rx) in watched.iter_mut() {
        while rx.borrow_and_update().is_some() {
            match tokio::time::timeout_at(deadline, rx.changed()).await {
                // Value changed: loop round and re-read it.
                Ok(Ok(())) => {}
                // Sender gone (actor dropped) or deadline reached.
                Ok(Err(_)) | Err(_) => break,
            }
        }
    }

    watched
        .iter()
        .filter_map(|(name, rx)| rx.borrow().map(|pid| (name.clone(), pid)))
        .collect()
}

/// Send SIGKILL to a child's process group, falling back to the process itself.
///
/// Children are spawned with `process_group(0)`, so each child leads its own
/// group and the group ID equals its PID. Killing the group therefore reaches
/// the child and anything it spawned (a shell's backgrounded `sleep`, for
/// instance) without touching the engine or any sibling primitive.
#[cfg(unix)]
pub fn sigkill_process_group(pid: u32) {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let Ok(raw) = i32::try_from(pid) else {
        warn!("Cannot SIGKILL pid {}: out of range", pid);
        return;
    };

    match kill(Pid::from_raw(-raw), Signal::SIGKILL) {
        Ok(()) => {}
        // The group is already gone; nothing to escalate against.
        Err(Errno::ESRCH) => {}
        Err(e) => {
            warn!("Failed to SIGKILL process group {}: {}", pid, e);
            if let Err(e) = kill(Pid::from_raw(raw), Signal::SIGKILL) {
                warn!("Failed to SIGKILL pid {}: {}", pid, e);
            }
        }
    }
}

/// SIGKILL is not available on this platform.
#[cfg(not(unix))]
pub fn sigkill_process_group(pid: u32) {
    warn!("SIGKILL not supported on this platform for pid {}", pid);
}

/// Ask the kernel to SIGTERM this process when the engine goes away.
///
/// Runs in the forked child, between `fork` and `exec`. Everything it does is
/// a bare syscall, which is what makes it safe in that window.
///
/// # Which death arms the signal
///
/// `PR_SET_PDEATHSIG` fires when the parent *thread* that forked this child
/// exits, not when the parent process exits. The engine forks from a tokio
/// multi-threaded runtime worker: `tokio::process::Command::spawn` calls
/// `std::process::Command::spawn` inline on the polling thread, and a worker
/// thread runs `worker::run` for the lifetime of the runtime, which the engine
/// holds open in `block_on` until `main` returns. No engine code on the run
/// path calls `block_in_place`, the one tokio API that can retire a worker
/// thread early, so a worker thread dying while the engine lives is not a case
/// that arises and the signal is not delivered spuriously.
///
/// # The fork/prctl race
///
/// The engine can die between `fork` and this call, in which case the kernel
/// already delivered the parent-death signal to nobody and never will. After
/// arming, the child compares its current parent against the engine PID read
/// before the fork; if they differ it has been reparented and exits rather than
/// living on as the orphan this guard exists to prevent.
///
/// The signal reaches this process only, not its process group, so a primitive
/// that spawns its own children is still responsible for them.
#[cfg(target_os = "linux")]
fn arm_parent_death_signal(engine_pid: nix::unistd::Pid) -> std::io::Result<()> {
    use nix::sys::prctl::set_pdeathsig;
    use nix::sys::signal::Signal;
    use nix::unistd::getppid;

    // A raw `prctl(2)`: no allocation, no locks.
    set_pdeathsig(Signal::SIGTERM).map_err(std::io::Error::from)?;

    if getppid() != engine_pid {
        // SAFETY: `_exit(2)` is async-signal-safe. `std::process::exit` is not:
        // it runs at-exit handlers and flushes buffers inherited from the
        // engine, which would be wrong in a forked child.
        unsafe { nix::libc::_exit(1) };
    }

    Ok(())
}

/// Message sent just before the actor spawns its child process.
#[acton_message]
pub struct ChildSpawning;

/// Message sent when a child process has been spawned successfully.
///
/// This message carries the PID only. The Child handle is NOT included
/// (doesn't implement Clone), and the primitive's info is already owned by
/// the actor.
#[acton_message]
pub struct ChildSpawned {
    /// Process ID of the spawned child.
    pub pid: u32,
}

/// Message sent when the child process could not be spawned at all.
#[acton_message]
pub struct ChildSpawnFailed {
    /// Why the spawn failed.
    pub error: String,
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

/// Message asking the actor to respawn its child after a backoff.
#[acton_message]
pub struct RestartChild {
    /// 1-based restart attempt this respawn represents.
    pub attempt: u32,
}

/// Message telling the actor the engine is tearing the topology down.
///
/// Sent to every actor before the first shutdown phase begins, so that a child
/// which exits during the drain is not mistaken for a crash and respawned.
#[acton_message]
pub struct EngineShuttingDown;

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
    ///
    /// The actor is the only writer of the live parts of this value: `state`,
    /// `pid` and `error`.
    pub info: PrimitiveInfo,
    /// Process ID of the child (if running).
    pub child_pid: Option<u32>,
    /// Whether the engine is shutting down, which suppresses restarts.
    pub shutting_down: bool,
    /// Unix-millisecond timestamps of restarts inside the current window.
    pub restart_attempts: Vec<u64>,
    /// Publishes the live info to whoever reads the topology.
    ///
    /// The process manager holds the matching receiver, so it reports what the
    /// actor last published instead of a registration-time copy.
    pub status_tx: Option<watch::Sender<PrimitiveInfo>>,
}

impl PrimitiveActorState {
    /// The live status currently held in `info`.
    fn status(&self) -> PrimitiveStatus {
        PrimitiveStatus {
            state: self.info.state,
            pid: self.info.pid,
            error: self.info.error.clone(),
        }
    }

    /// Apply a lifecycle event and publish the resulting status.
    ///
    /// Every transition goes through the pure [`next_status`] function, so the
    /// state, the pid and the error can never disagree, and the published copy
    /// is written here and nowhere else.
    fn apply(&mut self, event: &LifecycleEvent) {
        let next = next_status(&self.status(), event);
        self.info.state = next.state;
        self.info.pid = next.pid;
        self.info.error = next.error;
        self.child_pid = next.pid;

        if let Some(tx) = &self.status_tx {
            // An error here only means nothing is watching any more.
            let _ = tx.send(self.info.clone());
        }
    }
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
    /// HTTP API port for topology queries.
    pub api_port: u16,
    /// Writer for the live child PID, read by the process manager.
    pub pid_watch: ChildPidWatch,
    /// What to do when the child exits.
    pub restart: RestartPolicy,
    /// How restarts are paced and capped.
    pub restart_limits: RestartLimits,
}

/// Spawn the child process for a primitive and start monitoring it.
///
/// Used both for the initial spawn and for every restart. `attempt` is `None`
/// for the first spawn and `Some(n)` for restart number `n`, which selects
/// between the `system.started.<name>` and `system.restarted.<name>` events.
async fn spawn_primitive_child(
    config: &PrimitiveActorConfig,
    self_handle: ActorHandle,
    broker: ActorHandle,
    attempt: Option<u32>,
) {
    let spawn_info = config.info.clone();
    let name = spawn_info.name.clone();
    let pid_watch = config.pid_watch.clone();

    // Record the spawn attempt before it happens, so a slow or failing spawn
    // is visible as `starting` rather than `configured`.
    self_handle.send(ChildSpawning).await;

    let mut cmd = Command::new(&config.path);
    cmd.args(&config.args);

    for (key, value) in &config.env {
        cmd.env(key, value);
    }

    cmd.env(
        "EMERGENT_SOCKET",
        config.socket_path.to_string_lossy().as_ref(),
    );
    cmd.env("EMERGENT_NAME", &name);
    cmd.env("EMERGENT_API_PORT", config.api_port.to_string());
    cmd.env("EMERGENT_PUBLISHES", spawn_info.publishes.join(","));
    cmd.env("EMERGENT_SUBSCRIBES", spawn_info.subscribes.join(","));

    // Isolate child from terminal SIGINT - only engine handles Ctrl+C.
    // Children get their own process group so Ctrl+C only affects the engine,
    // and so shutdown can escalate to a group-wide SIGKILL.
    #[cfg(unix)]
    cmd.process_group(0);

    // Tie the child's lifetime to the engine's, so a SIGKILLed or aborting
    // engine does not leave the primitive running with no parent.
    #[cfg(target_os = "linux")]
    {
        // Read in the parent, before the fork, so the child can tell whether
        // the engine it was forked from is still its parent.
        let engine_pid = nix::unistd::Pid::this();
        // SAFETY: `arm_parent_death_signal` runs in the forked child between
        // `fork` and `exec`, where only async-signal-safe work is allowed. It
        // makes three bare syscalls (`prctl`, `getppid`, `_exit`) and nothing
        // else: no allocation, no locking, no Rust runtime re-entry. The
        // `io::Error` it can return is built only on the failure path, after
        // which the child is about to be reaped anyway.
        unsafe {
            cmd.pre_exec(move || arm_parent_death_signal(engine_pid));
        }
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            pid_watch.clear();
            error!("Failed to spawn {}: {}", name, e);
            self_handle
                .send(ChildSpawnFailed {
                    error: e.to_string(),
                })
                .await;
            let event = create_system_event("system.error", &spawn_info, None, Some(e.to_string()));
            broadcast_lifecycle_event(&broker, &name, "system.error", event).await;
            return;
        }
    };

    let Some(pid) = child.id() else {
        pid_watch.clear();
        error!("Failed to get PID for {}", name);
        self_handle
            .send(ChildSpawnFailed {
                error: "Spawned child reported no PID".to_string(),
            })
            .await;
        return;
    };

    match attempt {
        None => info!("Started {} (pid: {})", name, pid),
        Some(n) => info!("Restarted {} (pid: {}, attempt: {})", name, pid, n),
    }

    // Publish the PID before anything else so the process manager can wait on
    // (and if need be signal) this child.
    pid_watch.set(pid);

    // Store the PID via self-message; the actor owns the live state.
    self_handle.send(ChildSpawned { pid }).await;

    let (event_type, event) = match attempt {
        None => (
            "system.started",
            create_system_event("system.started", &spawn_info, Some(pid), None),
        ),
        Some(n) => (
            "system.restarted",
            create_restarted_event(&spawn_info, pid, n),
        ),
    };
    broadcast_lifecycle_event(&broker, &name, event_type, event).await;

    // Monitor the child in a BACKGROUND TASK. The Child handle lives HERE,
    // not in actor state, because it is not Clone.
    let monitor_handle = self_handle.clone();
    let monitor_info = spawn_info;
    let monitor_broker = broker;
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => {
                // Clear the watch first: the process manager may be waiting on
                // exactly this signal.
                pid_watch.clear();
                let exit_code = exit_code_from_status(&status);
                let clean = is_clean_exit(&status);
                if clean {
                    debug!(
                        "{} exited with status {} (pid: {})",
                        monitor_info.name, exit_code, pid
                    );
                } else {
                    warn!(
                        "{} exited with status {} (pid: {})",
                        monitor_info.name, exit_code, pid
                    );
                }

                monitor_handle
                    .send(ChildExited {
                        pid,
                        status: exit_code,
                    })
                    .await;

                let event_type = if clean {
                    "system.stopped"
                } else {
                    "system.error"
                };
                let error_msg = if clean {
                    None
                } else {
                    Some(exit_error_message(exit_code))
                };
                let event = create_system_event(event_type, &monitor_info, Some(pid), error_msg);
                broadcast_lifecycle_event(&monitor_broker, &monitor_info.name, event_type, event)
                    .await;
            }
            Err(e) => {
                pid_watch.clear();
                error!("Error waiting for {}: {}", monitor_info.name, e);
            }
        }
    });
}

/// Build and configure a primitive actor.
///
/// The actor will:
/// - Spawn the child process in `after_start`
/// - Broadcast `system.started.<name>` after successful spawn
/// - Handle `ChildSpawned` messages to track the PID
/// - Handle `ChildExited` messages when the child terminates, applying the
///   configured restart policy
/// - Broadcast `system.restarted.<name>` after a successful respawn
/// - Terminate the child via SIGTERM in `before_stop`
/// - Broadcast `system.stopped.<name>` after shutdown
///
/// Returns the actor together with a receiver for its live [`PrimitiveInfo`].
/// The actor owns that info and publishes every change to the receiver, which
/// is what the process manager reports for topology queries.
pub fn build_primitive_actor(
    runtime: &mut ActorRuntime,
    config: PrimitiveActorConfig,
) -> (
    ManagedActor<Idle, PrimitiveActorState>,
    watch::Receiver<PrimitiveInfo>,
) {
    let name = config.info.name.clone();

    let mut actor = runtime.new_actor_with_name::<PrimitiveActorState>(name);

    // The actor owns the live info from the start, so a primitive that has not
    // spawned yet still reports its own name, kind and wiring.
    let (status_tx, status_rx) = watch::channel(config.info.clone());
    actor.model.info = config.info.clone();
    actor.model.status_tx = Some(status_tx);

    let start_config = config.clone();
    let restart_config = config.clone();
    let exit_policy = config.restart;
    let exit_limits = config.restart_limits;

    actor
        .after_start(move |actor| {
            let config = start_config.clone();
            let self_handle = actor.handle().clone();
            let broker = actor.broker().clone();

            async move {
                spawn_primitive_child(&config, self_handle, broker, None).await;
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
        .mutate_on::<ChildSpawning>(|actor, _envelope| {
            actor.model.apply(&LifecycleEvent::SpawnRequested);
            Reply::ready()
        })
        .mutate_on::<ChildSpawned>(|actor, envelope| {
            let pid = envelope.message().pid;
            actor.model.apply(&LifecycleEvent::Spawned { pid });
            Reply::ready()
        })
        .mutate_on::<ChildSpawnFailed>(|actor, envelope| {
            let error = envelope.message().error.clone();
            actor.model.apply(&LifecycleEvent::SpawnFailed { error });
            Reply::ready()
        })
        .mutate_on::<ChildExited>(move |actor, envelope| {
            let msg = envelope.message();

            // A stale notice from a PID we have already replaced: ignore it.
            if actor.model.child_pid != Some(msg.pid) {
                return Reply::ready();
            }

            actor.model.apply(&LifecycleEvent::Exited {
                pid: msg.pid,
                code: msg.status,
            });

            let now_ms = Timestamp::now().as_millis();
            actor.model.restart_attempts =
                prune_attempts(&actor.model.restart_attempts, now_ms, exit_limits.window_ms);

            let decision = decide_restart(
                exit_policy,
                outcome_from_exit_code(msg.status),
                &actor.model.restart_attempts,
                &exit_limits,
                now_ms,
                actor.model.shutting_down,
            );

            let name = actor.model.info.name.clone();
            match decision {
                RestartDecision::NoRestart | RestartDecision::SuppressedByShutdown => {
                    Reply::ready()
                }
                RestartDecision::Restart { delay, attempt } => {
                    actor.model.restart_attempts.push(now_ms);
                    // The backoff is part of the respawn, so the primitive
                    // reads as `starting` while it waits.
                    actor.model.apply(&LifecycleEvent::SpawnRequested);
                    let self_handle = actor.handle().clone();
                    info!(
                        "Restarting {} in {} ms (attempt {} of {})",
                        name,
                        delay.as_millis(),
                        attempt,
                        exit_limits.max_retries
                    );
                    Reply::pending(async move {
                        tokio::time::sleep(delay).await;
                        self_handle.send(RestartChild { attempt }).await;
                    })
                }
                RestartDecision::Exhausted {
                    attempts,
                    window_ms,
                } => {
                    let reason = format!(
                        "Restarts exhausted: {} attempts within {} ms",
                        attempts, window_ms
                    );
                    warn!("{} will not be restarted. {}", name, reason);
                    actor.model.apply(&LifecycleEvent::RestartsExhausted {
                        reason: reason.clone(),
                    });
                    let info = actor.model.info.clone();
                    let broker = actor.broker().clone();
                    Reply::pending(async move {
                        let event = create_system_event("system.error", &info, None, Some(reason));
                        broadcast_lifecycle_event(&broker, &info.name, "system.error", event).await;
                    })
                }
            }
        })
        .mutate_on::<RestartChild>(move |actor, envelope| {
            let attempt = envelope.message().attempt;

            // Shutdown may have started while the backoff was running, or the
            // child may already be back: either way there is nothing to do.
            if actor.model.shutting_down || actor.model.child_pid.is_some() {
                return Reply::ready();
            }

            let config = restart_config.clone();
            let self_handle = actor.handle().clone();
            let broker = actor.broker().clone();
            Reply::pending(async move {
                spawn_primitive_child(&config, self_handle, broker, Some(attempt)).await;
            })
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
        .mutate_on::<EngineShuttingDown>(|actor, _envelope| {
            actor.model.shutting_down = true;
            Reply::ready()
        })
        .mutate_on::<StopPrimitive>(|actor, _envelope| {
            let name = actor.model.info.name.clone();
            // A stop request is always engine-initiated, so the child exiting
            // from here on is expected rather than a failure to recover from.
            actor.model.shutting_down = true;

            let child_pid = actor.model.child_pid;
            actor.model.apply(&LifecycleEvent::StopRequested);

            if let Some(pid) = child_pid {
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

    (actor, status_rx)
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

/// Message type of the event the engine emits when it is shutting down.
///
/// A literal, so it cannot fail validation. `shutdown_event_type_is_valid`
/// guards that against a future rename.
const SHUTDOWN_EVENT_TYPE: &str = "system.shutdown";

/// The name the engine publishes its own events under.
///
/// [`ENGINE_PRIMITIVE_NAME`] is a valid primitive name, so this never falls
/// back; the fallback exists only to keep the function total.
/// `engine_source_is_the_engine_primitive_name` guards the invariant.
fn engine_source() -> PrimitiveName {
    PrimitiveName::new(ENGINE_PRIMITIVE_NAME).unwrap_or_else(|_| PrimitiveName::unknown())
}

/// Create a system event message wrapped for IPC (pure function).
///
/// The message type is built from the primitive's configured name, which is
/// runtime data, so this returns an error instead of panicking. Config
/// validation rejects such a name before anything is spawned; this is the
/// second line of defense, because a panic here aborts the whole engine and
/// orphans every child it has spawned.
///
/// # Errors
///
/// Returns [`InvalidMessageType`] if `event_type` and the primitive's name do
/// not form a valid message type.
fn create_system_event(
    event_type: &str,
    info: &PrimitiveInfo,
    pid: Option<u32>,
    error: Option<String>,
) -> Result<IpcSystemEvent, InvalidMessageType> {
    let payload = match (event_type, error) {
        ("system.started", None) => SystemEventPayload::started(info, pid.unwrap_or(0)),
        ("system.stopped", None) => SystemEventPayload::stopped(info, pid),
        (_, Some(err)) => SystemEventPayload::error(info, pid, err),
        _ => SystemEventPayload::stopped(info, pid),
    };

    let message = EmergentMessage::try_new(&format!("{}.{}", event_type, info.name))?
        .with_source_name(engine_source())
        .with_payload(json!(payload));

    Ok(IpcSystemEvent { inner: message })
}

/// Create a `system.restarted.<name>` event wrapped for IPC (pure function).
///
/// # Errors
///
/// Returns [`InvalidMessageType`] if the primitive's name does not form a valid
/// message type, for the same reason as [`create_system_event`].
fn create_restarted_event(
    info: &PrimitiveInfo,
    pid: u32,
    attempt: u32,
) -> Result<IpcSystemEvent, InvalidMessageType> {
    let message = EmergentMessage::try_new(&format!("system.restarted.{}", info.name))?
        .with_source_name(engine_source())
        .with_payload(json!(SystemEventPayload::restarted(info, pid, attempt)));

    Ok(IpcSystemEvent { inner: message })
}

/// Broadcast a lifecycle event, or log why it could not be built.
///
/// A name that cannot form a message type is rejected at config load, so the
/// error arm is a second line of defense: it keeps a bad name from taking the
/// engine down, at the cost of one missing event.
async fn broadcast_lifecycle_event(
    broker: &ActorHandle,
    primitive: &str,
    event_type: &str,
    event: Result<IpcSystemEvent, InvalidMessageType>,
) {
    match event {
        Ok(event) => broker.broadcast(event).await,
        Err(e) => warn!(
            primitive = %primitive,
            error = %e,
            "Skipping {event_type} event: the primitive name does not form a valid message type"
        ),
    }
}

/// Create a shutdown system event wrapped for IPC.
///
/// The message type is a literal here, not runtime data, so this cannot fail.
/// `kind` only ever reaches the payload.
pub fn create_shutdown_event(kind: &str) -> IpcSystemEvent {
    let message = EmergentMessage::new(SHUTDOWN_EVENT_TYPE)
        .with_source_name(engine_source())
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
    fn test_system_event_payload_restarted_carries_the_attempt() {
        let info = make_handler_info(
            "victim",
            vec!["in.event".to_string()],
            vec!["out.event".to_string()],
        );
        let payload = SystemEventPayload::restarted(&info, 4321, 3);

        assert_eq!(payload.name, "victim");
        assert_eq!(payload.kind, "handler");
        assert_eq!(payload.pid, Some(4321));
        assert_eq!(payload.restart_attempt, Some(3));
        assert!(payload.error.is_none());
    }

    #[test]
    fn test_restart_attempt_is_absent_from_other_payloads() -> Result<(), serde_json::Error> {
        let info = make_source_info("s", vec!["e".to_string()]);
        for payload in [
            SystemEventPayload::started(&info, 1),
            SystemEventPayload::stopped(&info, Some(1)),
            SystemEventPayload::error(&info, Some(1), "boom".to_string()),
        ] {
            let json = serde_json::to_string(&payload)?;
            assert!(
                !json.contains("restart_attempt"),
                "restart_attempt leaked into {json}"
            );
        }
        Ok(())
    }

    #[test]
    fn test_restarted_event_type_is_namespaced_by_primitive() {
        let info = make_sink_info("logger", vec!["a".to_string()]);
        let Ok(event) = create_restarted_event(&info, 77, 2) else {
            panic!("a valid primitive name must form a system.restarted event");
        };

        assert_eq!(event.inner.message_type.as_str(), "system.restarted.logger");
        assert_eq!(event.inner.payload["name"], "logger");
        assert_eq!(event.inner.payload["pid"], 77);
        assert_eq!(event.inner.payload["restart_attempt"], 2);
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

    #[tokio::test]
    async fn wait_returns_immediately_when_nothing_is_running() {
        let (_w1, rx1) = ChildPidWatch::new();
        let (_w2, rx2) = ChildPidWatch::new();
        let mut watched = vec![("a".to_string(), rx1), ("b".to_string(), rx2)];

        let start = tokio::time::Instant::now();
        let alive = wait_for_children_exit(&mut watched, std::time::Duration::from_secs(5)).await;

        assert!(alive.is_empty());
        assert!(
            start.elapsed() < std::time::Duration::from_millis(250),
            "should not have waited: {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn wait_returns_as_soon_as_the_last_child_exits() {
        let (writer, rx) = ChildPidWatch::new();
        writer.set(4242);
        let mut watched = vec![("late".to_string(), rx)];

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            writer.clear();
        });

        let start = tokio::time::Instant::now();
        let alive = wait_for_children_exit(&mut watched, std::time::Duration::from_secs(5)).await;

        assert!(alive.is_empty());
        assert!(
            start.elapsed() < std::time::Duration::from_millis(1_000),
            "should have returned on exit, not on the budget: {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn wait_reports_survivors_with_their_pids_at_the_deadline() {
        let (stubborn, stubborn_rx) = ChildPidWatch::new();
        stubborn.set(1234);
        let (quick, quick_rx) = ChildPidWatch::new();
        quick.set(5678);
        quick.clear();

        let mut watched = vec![
            ("quick".to_string(), quick_rx),
            ("stubborn".to_string(), stubborn_rx),
        ];

        let alive =
            wait_for_children_exit(&mut watched, std::time::Duration::from_millis(50)).await;

        assert_eq!(alive, vec![("stubborn".to_string(), 1234)]);
    }

    #[tokio::test]
    async fn wait_shares_one_deadline_across_every_child() {
        let (a, a_rx) = ChildPidWatch::new();
        a.set(1);
        let (b, b_rx) = ChildPidWatch::new();
        b.set(2);
        let mut watched = vec![("a".to_string(), a_rx), ("b".to_string(), b_rx)];

        let start = tokio::time::Instant::now();
        let alive =
            wait_for_children_exit(&mut watched, std::time::Duration::from_millis(100)).await;

        assert_eq!(alive.len(), 2);
        // Two children must not cost two budgets.
        assert!(
            start.elapsed() < std::time::Duration::from_millis(180),
            "budget should be shared, not per child: {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn wait_treats_a_zero_budget_as_a_liveness_snapshot() {
        let (w, rx) = ChildPidWatch::new();
        w.set(99);
        let mut watched = vec![("x".to_string(), rx)];

        let alive = wait_for_children_exit(&mut watched, std::time::Duration::ZERO).await;
        assert_eq!(alive, vec![("x".to_string(), 99)]);
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

    #[test]
    fn engine_source_is_the_engine_primitive_name() {
        assert_eq!(engine_source().as_str(), ENGINE_PRIMITIVE_NAME);
        assert!(!engine_source().is_default());
    }

    #[test]
    fn shutdown_event_type_is_valid() {
        assert!(emergent_client::types::MessageType::new(SHUTDOWN_EVENT_TYPE).is_ok());
        let event = create_shutdown_event("signal");
        assert_eq!(event.inner.message_type.as_str(), SHUTDOWN_EVENT_TYPE);
        assert_eq!(event.inner.source.as_str(), ENGINE_PRIMITIVE_NAME);
    }

    #[test]
    fn create_system_event_builds_the_type_from_the_primitive_name() {
        let info = make_source_info("my-source", vec!["timer.tick".to_string()]);
        let Ok(event) = create_system_event("system.started", &info, Some(7), None) else {
            panic!("a valid name must produce an event");
        };
        assert_eq!(
            event.inner.message_type.as_str(),
            "system.started.my-source"
        );
        assert_eq!(event.inner.source.as_str(), ENGINE_PRIMITIVE_NAME);
    }

    #[test]
    fn create_system_event_reports_an_invalid_name_instead_of_panicking() {
        // The engine used to panic here, and with panic = "abort" that killed
        // the engine after the child was already spawned (Govcraft/emergent#42).
        let info = make_sink_info("Bad Name", vec!["tick.out".to_string()]);
        for event_type in ["system.started", "system.stopped", "system.error"] {
            assert!(
                create_system_event(event_type, &info, Some(7), None).is_err(),
                "expected {event_type} for 'Bad Name' to report an error"
            );
        }
    }
}
