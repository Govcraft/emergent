//! Process management for spawning and monitoring Sources, Handlers, and Sinks.
//!
//! The ProcessManager handles:
//! - Ordered startup (Sinks → Handlers → Sources)
//! - Graceful shutdown (Sources → Handlers → Sinks)
//! - Health monitoring
//! - Restart on failure
//! - System event publishing (system.started.*, system.stopped.*, system.error.*)

use crate::config::{HandlerConfig, SinkConfig, SourceConfig};
use crate::messages::EmergentMessage;
use crate::primitives::{PrimitiveInfo, PrimitiveKind, PrimitiveState};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

/// Payload for system lifecycle events.
#[derive(Debug, Clone, Serialize)]
pub struct SystemEventPayload {
    /// Name of the primitive.
    pub name: String,
    /// Kind of the primitive (source, handler, sink).
    pub kind: String,
    /// Optional error message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Process manager errors.
#[derive(Debug, Error)]
pub enum ProcessManagerError {
    #[error("Failed to spawn process: {0}")]
    SpawnError(#[from] std::io::Error),

    #[error("Primitive not found: {0}")]
    NotFound(String),

    #[error("Primitive already running: {0}")]
    AlreadyRunning(String),

    #[error("Primitive cannot be started: {0}")]
    CannotStart(String),
}

/// A managed process.
pub(crate) struct ManagedProcess {
    /// Process information.
    info: PrimitiveInfo,
    /// Child process handle.
    child: Option<Child>,
    /// Path to the executable.
    path: PathBuf,
    /// Command-line arguments.
    args: Vec<String>,
    /// Environment variables.
    env: HashMap<String, String>,
}

/// Manages the lifecycle of Source, Handler, and Sink processes.
#[derive(Clone)]
pub struct ProcessManager {
    /// Socket path for clients to connect to.
    pub(crate) socket_path: PathBuf,
    /// Managed processes.
    pub(crate) processes: Arc<RwLock<HashMap<String, ManagedProcess>>>,
    /// Channel for publishing system events.
    event_sender: Option<mpsc::Sender<EmergentMessage>>,
}

impl ProcessManager {
    /// Create a new process manager.
    #[must_use]
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            processes: Arc::new(RwLock::new(HashMap::new())),
            event_sender: None,
        }
    }

    /// Set the event sender for publishing system events.
    ///
    /// This should be called after the IPC broker is ready to receive messages.
    pub fn set_event_sender(&mut self, sender: mpsc::Sender<EmergentMessage>) {
        self.event_sender = Some(sender);
    }

    /// Publish a system event.
    async fn publish_system_event(&self, event_type: &str, name: &str, kind: &PrimitiveKind, error: Option<String>) {
        let kind_str = match kind {
            PrimitiveKind::Source => "source",
            PrimitiveKind::Handler => "handler",
            PrimitiveKind::Sink => "sink",
        };

        let payload = SystemEventPayload {
            name: name.to_string(),
            kind: kind_str.to_string(),
            error,
        };

        let message = EmergentMessage::new(&format!("{event_type}.{name}"))
            .with_source("emergent-engine")
            .with_payload(payload);

        if let Some(ref sender) = self.event_sender
            && let Err(e) = sender.send(message).await
        {
            error!("Failed to publish system event: {}", e);
        }
    }

    /// Register a source from configuration.
    pub async fn register_source(&self, config: &SourceConfig) {
        let info = PrimitiveInfo::source(&config.name, config.publishes.clone());

        let process = ManagedProcess {
            info,
            child: None,
            path: config.path.clone(),
            args: config.args.clone(),
            env: config.env.clone(),
        };

        let mut processes = self.processes.write().await;
        processes.insert(config.name.clone(), process);
    }

    /// Register a handler from configuration.
    pub async fn register_handler(&self, config: &HandlerConfig) {
        let info = PrimitiveInfo::handler(
            &config.name,
            config.subscribes.clone(),
            config.publishes.clone(),
        );

        let process = ManagedProcess {
            info,
            child: None,
            path: config.path.clone(),
            args: config.args.clone(),
            env: config.env.clone(),
        };

        let mut processes = self.processes.write().await;
        processes.insert(config.name.clone(), process);
    }

    /// Register a sink from configuration.
    pub async fn register_sink(&self, config: &SinkConfig) {
        let info = PrimitiveInfo::sink(&config.name, config.subscribes.clone());

        let process = ManagedProcess {
            info,
            child: None,
            path: config.path.clone(),
            args: config.args.clone(),
            env: config.env.clone(),
        };

        let mut processes = self.processes.write().await;
        processes.insert(config.name.clone(), process);
    }

    /// Start all registered processes in the correct order.
    ///
    /// Order: Sinks → Handlers → Sources
    pub async fn start_all(&self) -> Result<(), ProcessManagerError> {
        // Collect names by kind
        let (sinks, handlers, sources) = {
            let processes = self.processes.read().await;
            let mut sinks = Vec::new();
            let mut handlers = Vec::new();
            let mut sources = Vec::new();

            for (name, process) in processes.iter() {
                match process.info.kind {
                    PrimitiveKind::Sink => sinks.push(name.clone()),
                    PrimitiveKind::Handler => handlers.push(name.clone()),
                    PrimitiveKind::Source => sources.push(name.clone()),
                }
            }

            (sinks, handlers, sources)
        };

        // Start sinks first
        for name in sinks {
            info!("Starting sink: {}", name);
            self.start_one(&name).await?;
        }

        // Then handlers
        for name in handlers {
            info!("Starting handler: {}", name);
            self.start_one(&name).await?;
        }

        // Finally sources
        for name in sources {
            info!("Starting source: {}", name);
            self.start_one(&name).await?;
        }

        Ok(())
    }

    /// Start a single process by name.
    async fn start_one(&self, name: &str) -> Result<(), ProcessManagerError> {
        let (kind, result) = {
            let mut processes = self.processes.write().await;
            let process = processes
                .get_mut(name)
                .ok_or_else(|| ProcessManagerError::NotFound(name.to_string()))?;

            if !process.info.can_start() {
                return Err(ProcessManagerError::CannotStart(name.to_string()));
            }

            process.info.state = PrimitiveState::Starting;

            // Build the command
            let mut cmd = Command::new(&process.path);
            cmd.args(&process.args);
            cmd.envs(&process.env);

            // Pass socket path as environment variable
            cmd.env("EMERGENT_SOCKET", &self.socket_path);
            cmd.env("EMERGENT_NAME", name);

            let kind = process.info.kind;

            // Spawn the process
            match cmd.spawn() {
                Ok(child) => {
                    process.info.pid = child.id();
                    process.info.state = PrimitiveState::Running;
                    process.child = Some(child);
                    info!("Started {} (pid: {:?})", name, process.info.pid);
                    (kind, Ok(()))
                }
                Err(e) => {
                    process.info.state = PrimitiveState::Failed;
                    process.info.error = Some(e.to_string());
                    error!("Failed to start {}: {}", name, e);
                    (kind, Err(ProcessManagerError::SpawnError(e)))
                }
            }
        };

        // Publish system event after releasing the lock
        match &result {
            Ok(()) => {
                self.publish_system_event("system.started", name, &kind, None).await;
            }
            Err(e) => {
                self.publish_system_event("system.error", name, &kind, Some(e.to_string())).await;
            }
        }

        result
    }

    /// Stop all processes in the correct order.
    ///
    /// Order: Sources → Handlers → Sinks
    pub async fn stop_all(&self) {
        // Collect names by kind
        let (sources, handlers, sinks) = {
            let processes = self.processes.read().await;
            let mut sinks = Vec::new();
            let mut handlers = Vec::new();
            let mut sources = Vec::new();

            for (name, process) in processes.iter() {
                if process.info.is_running() {
                    match process.info.kind {
                        PrimitiveKind::Sink => sinks.push(name.clone()),
                        PrimitiveKind::Handler => handlers.push(name.clone()),
                        PrimitiveKind::Source => sources.push(name.clone()),
                    }
                }
            }

            (sources, handlers, sinks)
        };

        // Stop sources first
        for name in sources {
            info!("Stopping source: {}", name);
            self.stop_one(&name).await;
        }

        // Allow time for in-flight messages to drain
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        // Then handlers
        for name in handlers {
            info!("Stopping handler: {}", name);
            self.stop_one(&name).await;
        }

        // Finally sinks
        for name in sinks {
            info!("Stopping sink: {}", name);
            self.stop_one(&name).await;
        }
    }

    /// Stop a single process by name.
    async fn stop_one(&self, name: &str) {
        let kind = {
            let mut processes = self.processes.write().await;
            if let Some(process) = processes.get_mut(name) {
                process.info.state = PrimitiveState::Stopping;
                let kind = process.info.kind;

                if let Some(ref mut child) = process.child {
                    // Try graceful shutdown first (SIGTERM on Unix)
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{kill, Signal};
                        use nix::unistd::Pid;

                        if let Some(pid) = child.id() {
                            let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
                        }
                    }

                    // Wait a bit for graceful shutdown
                    tokio::select! {
                        _ = child.wait() => {
                            info!("{} stopped gracefully", name);
                        }
                        _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {
                            warn!("{} did not stop gracefully, killing", name);
                            let _ = child.kill().await;
                        }
                    }

                    process.info.state = PrimitiveState::Stopped;
                    process.info.pid = None;
                    process.child = None;
                }

                Some(kind)
            } else {
                None
            }
        };

        // Publish system event after releasing the lock
        if let Some(kind) = kind {
            self.publish_system_event("system.stopped", name, &kind, None).await;
        }
    }

    /// Get information about all registered primitives.
    pub async fn list_all(&self) -> Vec<PrimitiveInfo> {
        let processes = self.processes.read().await;
        processes.values().map(|p| p.info.clone()).collect()
    }

    /// Get information about a specific primitive.
    pub async fn get_info(&self, name: &str) -> Option<PrimitiveInfo> {
        let processes = self.processes.read().await;
        processes.get(name).map(|p| p.info.clone())
    }

    /// Check and update the status of all running processes.
    pub async fn health_check(&self) {
        // Collect events to publish after releasing the lock
        let events: Vec<(String, PrimitiveKind, bool, Option<String>)> = {
            let mut processes = self.processes.write().await;
            let mut events = Vec::new();

            for (name, process) in processes.iter_mut() {
                if let Some(ref mut child) = process.child {
                    // Check if process is still running
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            // Process has exited
                            let kind = process.info.kind;
                            if status.success() {
                                process.info.state = PrimitiveState::Stopped;
                                info!("{} exited successfully", name);
                                events.push((name.clone(), kind, true, None));
                            } else {
                                process.info.state = PrimitiveState::Failed;
                                let error_msg = format!("Exited with status: {status}");
                                process.info.error = Some(error_msg.clone());
                                warn!("{} exited with status: {}", name, status);
                                events.push((name.clone(), kind, false, Some(error_msg)));
                            }
                            process.info.pid = None;
                            process.child = None;
                        }
                        Ok(None) => {
                            // Process still running
                        }
                        Err(e) => {
                            error!("Error checking status of {}: {}", name, e);
                        }
                    }
                }
            }

            events
        };

        // Publish events after releasing the lock
        for (name, kind, success, error) in events {
            if success {
                self.publish_system_event("system.stopped", &name, &kind, None).await;
            } else {
                self.publish_system_event("system.error", &name, &kind, error).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_register_primitives() {
        let manager = ProcessManager::new(PathBuf::from("/tmp/test.sock"));

        let source_config = SourceConfig {
            name: "test_source".to_string(),
            path: PathBuf::from("/bin/true"),
            args: vec![],
            enabled: true,
            publishes: vec!["test.event".to_string()],
            env: HashMap::new(),
        };

        manager.register_source(&source_config).await;

        let info = manager.get_info("test_source").await.unwrap();
        assert_eq!(info.kind, PrimitiveKind::Source);
        assert_eq!(info.state, PrimitiveState::Configured);
    }
}
