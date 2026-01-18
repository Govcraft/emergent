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
    build_primitive_actor, create_shutdown_event, PrimitiveActorConfig, StopPrimitive,
};
use crate::primitives::{PrimitiveInfo, PrimitiveKind};
use acton_reactive::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::RwLock;
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
    /// Primitive information (name, kind, state, etc.).
    info: PrimitiveInfo,
    /// Startup configuration (kept for restart capability).
    #[allow(dead_code)]
    config: PrimitiveActorConfig,
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
    /// Actor handles by name.
    actors: Arc<RwLock<HashMap<String, ActorEntry>>>,
}

impl ProcessManager {
    /// Create a new process manager.
    #[must_use]
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
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
        self.register_primitive(runtime, info, &config.path, &config.args, &config.env)
            .await
    }

    /// Register a sink from configuration.
    pub async fn register_sink(
        &self,
        runtime: &mut ActorRuntime,
        config: &SinkConfig,
    ) -> Result<(), ProcessManagerError> {
        let info = PrimitiveInfo::sink(&config.name, config.subscribes.clone());
        self.register_primitive(runtime, info, &config.path, &config.args, &config.env)
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
        };

        // Build the actor (does not start it yet)
        let actor = build_primitive_actor(runtime, actor_config.clone());

        // Start the actor - this triggers after_start which spawns the process
        let handle = actor.start().await;

        // Store the entry
        let entry = ActorEntry {
            handle,
            info,
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
                match entry.info.kind {
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
    pub async fn list_all(&self) -> Vec<PrimitiveInfo> {
        let actors = self.actors.read().await;
        actors.values().map(|e| e.info.clone()).collect()
    }

    /// Get information about a specific primitive.
    pub async fn get_info(&self, name: &str) -> Option<PrimitiveInfo> {
        let actors = self.actors.read().await;
        actors.get(name).map(|e| e.info.clone())
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
            .filter(|e| e.info.kind == kind)
            .map(|e| e.info.clone())
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

    /// Stop primitives by stopping their actors.
    ///
    /// Used for sources that can't subscribe to broadcast messages.
    /// Stopping the actor triggers before_stop which sends SIGTERM and broadcasts system.stopped.
    async fn signal_and_wait(&self, kind: PrimitiveKind) {
        // Get handles for primitives of this kind
        let entries: Vec<(String, ActorHandle)> = {
            let actors = self.actors.read().await;
            actors
                .iter()
                .filter(|(_, e)| e.info.kind == kind)
                .map(|(name, e)| (name.clone(), e.handle.clone()))
                .collect()
        };

        // Stop each actor (triggers before_stop which sends SIGTERM)
        for (name, handle) in entries {
            info!("Stopping {}", name);
            let _ = handle.stop().await;
            // Brief delay to allow child to receive SIGTERM and exit
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Broadcast a shutdown message for a specific primitive kind.
    async fn broadcast_shutdown(&self, broker: &ActorHandle, kind: &str) {
        broker.broadcast(create_shutdown_event(kind)).await;
    }

    /// Wait for primitives to handle shutdown message, then stop their actors.
    ///
    /// Gives primitives time to process remaining messages before stopping.
    async fn wait_for_kind_exit(&self, kind: PrimitiveKind) {
        // Brief wait to allow primitives to receive and process shutdown message
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Get handles for primitives of this kind
        let entries: Vec<(String, ActorHandle)> = {
            let actors = self.actors.read().await;
            actors
                .iter()
                .filter(|(_, e)| e.info.kind == kind)
                .map(|(name, e)| (name.clone(), e.handle.clone()))
                .collect()
        };

        // Stop each actor (triggers before_stop which sends SIGTERM and broadcasts system.stopped)
        for (name, handle) in entries {
            info!("Stopping {}", name);
            let _ = handle.stop().await;
            // Brief delay to allow child to exit and system.stopped to be broadcast
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_process_manager_creation() {
        let manager = ProcessManager::new(PathBuf::from("/tmp/test.sock"));
        assert_eq!(manager.count().await, 0);
    }

    #[tokio::test]
    async fn test_list_empty() {
        let manager = ProcessManager::new(PathBuf::from("/tmp/test.sock"));
        let primitives = manager.list_all().await;
        assert!(primitives.is_empty());
    }
}
