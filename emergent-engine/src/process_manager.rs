//! Process management for spawning and monitoring Sources, Handlers, and Sinks.
//!
//! The ProcessManager coordinates actor-based lifecycle management:
//! - Ordered startup (Sinks → Handlers → Sources)
//! - Graceful shutdown (Sources → Handlers → Sinks)
//! - Health monitoring via actor state
//!
//! Each primitive is managed by a PrimitiveActor that handles:
//! - Process spawning in `after_start` (after message loop is active)
//! - System event broadcasting (`system.started.*`, `system.stopped.*`)
//! - Graceful termination in `before_stop`

use crate::config::{HandlerConfig, SinkConfig, SourceConfig};
use crate::primitive_actor::{
    PrimitiveActorConfig, StopPrimitive, build_primitive_actor, create_shutdown_event,
};
use crate::primitives::{PrimitiveInfo, PrimitiveKind};
use acton_reactive::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{RwLock, watch};
use tracing::{error, info, warn};

/// Process manager errors.
#[derive(Debug, Error)]
pub enum ProcessManagerError {
    #[error("Primitive not found: {0}")]
    NotFound(String),

    #[error("Primitive already exists: {0}")]
    AlreadyExists(String),

    #[error("Runtime not available")]
    RuntimeUnavailable,

    #[error("Failed to start actor: {0}")]
    StartError(String),
}

/// Actor entry tracking handle and primitive info.
struct ActorEntry {
    /// Handle to the running actor.
    handle: ActorHandle,
    /// Kind of the primitive, fixed at registration and used for ordering.
    kind: PrimitiveKind,
    /// Live primitive information published by the actor that owns it.
    ///
    /// Reading through this receiver is what makes topology queries report the
    /// current state and pid instead of a registration-time copy.
    status: watch::Receiver<PrimitiveInfo>,
    /// Startup configuration (kept for restart capability).
    #[allow(dead_code)]
    config: PrimitiveActorConfig,
}

impl ActorEntry {
    /// Snapshot the primitive's current information.
    fn info(&self) -> PrimitiveInfo {
        self.status.borrow().clone()
    }
}

/// Manages the lifecycle of Source, Handler, and Sink processes.
///
/// Uses actor-based lifecycle management where each primitive is managed
/// by a PrimitiveActor that handles process spawning, monitoring, and
/// termination.
#[derive(Clone)]
pub struct ProcessManager {
    /// Socket path for clients to connect to.
    socket_path: PathBuf,
    /// HTTP API port for topology queries.
    api_port: u16,
    /// Actor handles by name.
    actors: Arc<RwLock<HashMap<String, ActorEntry>>>,
}

impl ProcessManager {
    /// Create a new process manager.
    #[must_use]
    pub fn new(socket_path: PathBuf, api_port: u16) -> Self {
        Self {
            socket_path,
            api_port,
            actors: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a source from configuration.
    ///
    /// Creates a PrimitiveActor for the source but does not start it.
    /// Call `start_all` or `start_one` to begin the lifecycle.
    pub async fn register_source(
        &self,
        runtime: &mut ActorRuntime,
        config: &SourceConfig,
    ) -> Result<(), ProcessManagerError> {
        let info = PrimitiveInfo::source(&config.name, config.publishes.clone());
        self.register_primitive(runtime, info, &config.path, &config.args, &config.env)
            .await
    }

    /// Register a handler from configuration.
    pub async fn register_handler(
        &self,
        runtime: &mut ActorRuntime,
        config: &HandlerConfig,
    ) -> Result<(), ProcessManagerError> {
        let info = PrimitiveInfo::handler(
            &config.name,
            config.subscribes.clone(),
            config.publishes.clone(),
        );
        let mut env = config.env.clone();
        if config.unwrap_stdout {
            env.insert("EMERGENT_UNWRAP_STDOUT".into(), "true".into());
        }
        self.register_primitive(runtime, info, &config.path, &config.args, &env)
            .await
    }

    /// Register a sink from configuration.
    pub async fn register_sink(
        &self,
        runtime: &mut ActorRuntime,
        config: &SinkConfig,
    ) -> Result<(), ProcessManagerError> {
        let info = PrimitiveInfo::sink(&config.name, config.subscribes.clone());
        let mut env = config.env.clone();
        if config.unwrap_stdout {
            env.insert("EMERGENT_UNWRAP_STDOUT".into(), "true".into());
        }
        self.register_primitive(runtime, info, &config.path, &config.args, &env)
            .await
    }

    /// Register a primitive and create its actor.
    async fn register_primitive(
        &self,
        runtime: &mut ActorRuntime,
        info: PrimitiveInfo,
        path: &std::path::Path,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<(), ProcessManagerError> {
        let name = info.name.clone();
        let kind = info.kind;

        // Check for duplicates
        {
            let actors = self.actors.read().await;
            if actors.contains_key(&name) {
                return Err(ProcessManagerError::AlreadyExists(name));
            }
        }

        // Build actor configuration
        let actor_config = PrimitiveActorConfig {
            info: info.clone(),
            path: path.to_path_buf(),
            args: args.to_vec(),
            env: env.clone(),
            socket_path: self.socket_path.clone(),
            api_port: self.api_port,
        };

        // Build the actor (does not start it yet). The actor owns the live
        // state; `status` is the read side of what it publishes.
        let (actor, status) = build_primitive_actor(runtime, actor_config.clone());

        // Start the actor - this triggers after_start which spawns the process
        let handle = actor.start().await;

        // Store the entry
        let entry = ActorEntry {
            handle,
            kind,
            status,
            config: actor_config,
        };

        let mut actors = self.actors.write().await;
        actors.insert(name.clone(), entry);

        info!("Registered {}", name);
        Ok(())
    }

    /// Start all registered primitives in the correct order.
    ///
    /// Order: Sinks → Handlers → Sources
    ///
    /// This ensures that:
    /// 1. Sinks are ready to receive messages
    /// 2. Handlers are ready to process messages
    /// 3. Sources start producing messages
    pub async fn start_all(
        &self,
        runtime: &mut ActorRuntime,
        sinks: &[&SinkConfig],
        handlers: &[&HandlerConfig],
        sources: &[&SourceConfig],
    ) -> Result<(), ProcessManagerError> {
        // Register and start sinks first
        for sink in sinks {
            info!("Starting sink: {}", sink.name);
            if let Err(e) = self.register_sink(runtime, sink).await {
                error!("Failed to start sink {}: {}", sink.name, e);
                return Err(e);
            }
            // Small delay to ensure IPC connection is established
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        // Then handlers
        for handler in handlers {
            info!("Starting handler: {}", handler.name);
            if let Err(e) = self.register_handler(runtime, handler).await {
                error!("Failed to start handler {}: {}", handler.name, e);
                return Err(e);
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        // Finally sources
        for source in sources {
            info!("Starting source: {}", source.name);
            if let Err(e) = self.register_source(runtime, source).await {
                error!("Failed to start source {}: {}", source.name, e);
                return Err(e);
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        Ok(())
    }

    /// Stop all primitives in the correct order.
    ///
    /// Order: Sources → Handlers → Sinks
    ///
    /// This ensures that:
    /// 1. Sources stop producing messages
    /// 2. Handlers finish processing in-flight messages
    /// 3. Sinks finish handling remaining messages
    pub async fn stop_all(&self) {
        // Collect names by kind
        let (sources, handlers, sinks) = {
            let actors = self.actors.read().await;
            let mut sources = Vec::new();
            let mut handlers = Vec::new();
            let mut sinks = Vec::new();

            for (name, entry) in actors.iter() {
                match entry.kind {
                    PrimitiveKind::Source => sources.push(name.clone()),
                    PrimitiveKind::Handler => handlers.push(name.clone()),
                    PrimitiveKind::Sink => sinks.push(name.clone()),
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

        // Allow time for handlers to finish
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Finally sinks
        for name in sinks {
            info!("Stopping sink: {}", name);
            self.stop_one(&name).await;
        }
    }

    /// Stop a single primitive by name.
    async fn stop_one(&self, name: &str) {
        let handle = {
            let actors = self.actors.read().await;
            actors.get(name).map(|e| e.handle.clone())
        };

        if let Some(handle) = handle {
            // Send stop message to the actor
            // The actor's before_stop hook will:
            // 1. Send SIGTERM to the child process
            // 2. Broadcast system.stopped event
            handle.send(StopPrimitive).await;

            // Give the actor time to handle the stop
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Stop the actor itself
            let _ = handle.stop().await;
        } else {
            warn!("Primitive not found for stop: {}", name);
        }
    }

    /// Get information about all registered primitives.
    ///
    /// The state, pid and error come from each primitive's actor, so a running
    /// primitive reports `Running` with its pid, a cleanly exited one reports
    /// `Stopped`, and a crashed one reports `Failed` with the error.
    pub async fn list_all(&self) -> Vec<PrimitiveInfo> {
        let actors = self.actors.read().await;
        actors.values().map(ActorEntry::info).collect()
    }

    /// Get information about a specific primitive.
    pub async fn get_info(&self, name: &str) -> Option<PrimitiveInfo> {
        let actors = self.actors.read().await;
        actors.get(name).map(ActorEntry::info)
    }

    /// Get the count of registered primitives.
    pub async fn count(&self) -> usize {
        let actors = self.actors.read().await;
        actors.len()
    }

    /// Check if a primitive is registered.
    pub async fn contains(&self, name: &str) -> bool {
        let actors = self.actors.read().await;
        actors.contains_key(name)
    }

    /// Get primitives by kind.
    pub async fn get_by_kind(&self, kind: PrimitiveKind) -> Vec<PrimitiveInfo> {
        let actors = self.actors.read().await;
        actors
            .values()
            .filter(|e| e.kind == kind)
            .map(ActorEntry::info)
            .collect()
    }

    /// Graceful shutdown with coordinated drain protocol.
    ///
    /// This implements a cascading shutdown where each tier drains completely
    /// before the next tier is signaled:
    ///
    /// 1. Sources are stopped via SIGTERM (they can't subscribe to broadcasts)
    /// 2. Handlers receive `system.shutdown` and finish processing
    /// 3. Sinks receive `system.shutdown` and finish output
    ///
    /// This ensures all `system.stopped.*` messages are visible to sinks.
    pub async fn graceful_shutdown(&self, broker: &ActorHandle) {
        // Phase 1: Stop sources
        // Sources are publish-only and can't subscribe, so we signal them directly
        info!("Stopping sources...");
        self.broadcast_shutdown(broker, "source").await;
        self.signal_and_wait(PrimitiveKind::Source).await;

        // Phase 2: Drain handlers
        // Handlers can subscribe and will receive the shutdown message
        info!("Draining handlers...");
        self.broadcast_shutdown(broker, "handler").await;
        self.wait_for_kind_exit(PrimitiveKind::Handler).await;

        // Phase 3: Drain sinks
        // Sinks can subscribe and will receive the shutdown message
        info!("Draining sinks...");
        self.broadcast_shutdown(broker, "sink").await;
        self.wait_for_kind_exit(PrimitiveKind::Sink).await;

        info!("All primitives stopped.");
    }

    /// Stop primitives by signaling children, then stopping actors.
    ///
    /// Used for sources that can't subscribe to broadcast messages.
    /// Phase A: Send StopPrimitive to all actors (SIGTERM sent, actor stays alive)
    /// Phase B: Sleep 2s for children to exit (monitor tasks broadcast system.stopped.*)
    /// Phase C: handle.stop() for each actor (cleanup; before_stop sees child_pid=None, skips SIGTERM)
    async fn signal_and_wait(&self, kind: PrimitiveKind) {
        // Get handles for primitives of this kind
        let entries: Vec<(String, ActorHandle)> = {
            let actors = self.actors.read().await;
            actors
                .iter()
                .filter(|(_, e)| e.kind == kind)
                .map(|(name, e)| (name.clone(), e.handle.clone()))
                .collect()
        };

        // Phase A: Send StopPrimitive to all actors (SIGTERM, actor stays alive)
        for (name, handle) in &entries {
            info!("Signaling {} to stop", name);
            handle.send(StopPrimitive).await;
        }

        // Phase B: Wait for children to exit and monitor tasks to broadcast system.stopped.*
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Phase C: Stop the actors themselves (cleanup)
        for (name, handle) in &entries {
            info!("Stopping actor {}", name);
            let _ = handle.stop().await;
        }
    }

    /// Broadcast a shutdown message for a specific primitive kind.
    async fn broadcast_shutdown(&self, broker: &ActorHandle, kind: &str) {
        broker.broadcast(create_shutdown_event(kind)).await;
    }

    /// Wait for primitives to handle shutdown message, then stop their actors.
    ///
    /// Gives primitives time to process the system.shutdown broadcast and exit
    /// gracefully before falling back to SIGTERM.
    ///
    /// Initial: Sleep 500ms (let system.shutdown propagate, children begin graceful exit)
    /// Phase A: Send StopPrimitive to all actors (SIGTERM as fallback if child didn't exit)
    /// Phase B: Sleep 2s for children to exit (monitor tasks broadcast system.stopped.*)
    /// Phase C: handle.stop() for each actor (cleanup)
    async fn wait_for_kind_exit(&self, kind: PrimitiveKind) {
        // Initial: Let system.shutdown propagate and children begin graceful exit
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Get handles for primitives of this kind
        let entries: Vec<(String, ActorHandle)> = {
            let actors = self.actors.read().await;
            actors
                .iter()
                .filter(|(_, e)| e.kind == kind)
                .map(|(name, e)| (name.clone(), e.handle.clone()))
                .collect()
        };

        // Phase A: Send StopPrimitive (SIGTERM fallback if child didn't exit from system.shutdown)
        for (name, handle) in &entries {
            info!("Signaling {} to stop", name);
            handle.send(StopPrimitive).await;
        }

        // Phase B: Wait for children to exit and monitor tasks to broadcast system.stopped.*
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Phase C: Stop the actors themselves (cleanup)
        for (name, handle) in &entries {
            info!("Stopping actor {}", name);
            let _ = handle.stop().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::PrimitiveState;

    /// Build a source config for a real executable.
    fn source_config(name: &str, path: &str, args: &[&str]) -> SourceConfig {
        SourceConfig {
            name: name.to_string(),
            path: PathBuf::from(path),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            enabled: true,
            publishes: Vec::new(),
            env: HashMap::new(),
        }
    }

    /// Poll the manager until a primitive satisfies `predicate`.
    ///
    /// The actor spawns its child in `after_start`, so the state a query sees
    /// depends on how far that has got. Polling keeps the test honest without
    /// fixing an arbitrary sleep.
    async fn wait_for(
        manager: &ProcessManager,
        name: &str,
        predicate: impl Fn(&PrimitiveInfo) -> bool,
    ) -> Option<PrimitiveInfo> {
        for _ in 0..100 {
            if let Some(info) = manager.get_info(name).await
                && predicate(&info)
            {
                return Some(info);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        None
    }

    #[tokio::test]
    async fn test_process_manager_creation() {
        let manager = ProcessManager::new(PathBuf::from("/tmp/test.sock"), 8891);
        assert_eq!(manager.count().await, 0);
    }

    #[tokio::test]
    async fn test_list_empty() {
        let manager = ProcessManager::new(PathBuf::from("/tmp/test.sock"), 8891);
        let primitives = manager.list_all().await;
        assert!(primitives.is_empty());
    }

    /// The defect behind Govcraft/emergent#40: every managed primitive reported
    /// `Configured` with no pid for the life of the engine.
    #[tokio::test]
    async fn queries_report_the_live_state_of_each_primitive() {
        let mut runtime = ActonApp::launch_async().await;
        let manager = ProcessManager::new(PathBuf::from("/tmp/emergent-issue-40-test.sock"), 0);

        let alive = source_config("alive", "/bin/sleep", &["30"]);
        let one_shot = source_config("one-shot", "/bin/true", &[]);
        let crasher = source_config("crasher", "/bin/false", &[]);

        for config in [&alive, &one_shot, &crasher] {
            assert!(manager.register_source(&mut runtime, config).await.is_ok());
        }

        let running = wait_for(&manager, "alive", |i| i.state == PrimitiveState::Running).await;
        let Some(running) = running else {
            panic!(
                "alive never reported Running: {:?}",
                manager.get_info("alive").await
            );
        };
        assert!(running.pid.is_some(), "a running primitive reports its pid");
        assert_eq!(running.error, None);

        let stopped = wait_for(&manager, "one-shot", |i| i.state == PrimitiveState::Stopped).await;
        let Some(stopped) = stopped else {
            panic!(
                "one-shot never reported Stopped: {:?}",
                manager.get_info("one-shot").await
            );
        };
        assert_eq!(stopped.pid, None, "a stopped primitive has no pid");
        assert_eq!(stopped.error, None);

        let failed = wait_for(&manager, "crasher", |i| i.state == PrimitiveState::Failed).await;
        let Some(failed) = failed else {
            panic!(
                "crasher never reported Failed: {:?}",
                manager.get_info("crasher").await
            );
        };
        assert_eq!(failed.pid, None);
        assert_eq!(failed.error.as_deref(), Some("Exited with status: 1"));

        // list_all agrees with the per-primitive query, since both read the
        // state the actors publish.
        let all = manager.list_all().await;
        assert_eq!(all.len(), 3);
        assert!(
            all.iter()
                .any(|i| i.name == "alive" && i.state == PrimitiveState::Running)
        );

        manager.stop_all().await;

        let after_stop = wait_for(&manager, "alive", |i| i.state != PrimitiveState::Running).await;
        let Some(after_stop) = after_stop else {
            panic!("alive stayed Running after stop_all");
        };
        assert_ne!(after_stop.state, PrimitiveState::Running);
    }
}
