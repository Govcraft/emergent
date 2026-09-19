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

use crate::config::{EngineConfig, HandlerConfig, RestartConfig, SinkConfig, SourceConfig};
use crate::primitive_actor::{
    ChildPidWatch, EngineShuttingDown, PrimitiveActorConfig, StopPrimitive, build_primitive_actor,
    create_shutdown_event, sigkill_process_group, wait_for_children_exit,
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

/// How long to wait for a SIGKILLed child to be reaped before giving up.
///
/// SIGKILL is not catchable, so this only covers the kernel delivering the
/// signal and the monitor task observing the exit.
const REAP_TIMEOUT: Duration = Duration::from_millis(500);

/// Time budgets for the shutdown sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownTimings {
    /// How long a phase waits for children to exit on the `system.shutdown`
    /// broadcast alone, before SIGTERM.
    pub drain: Duration,
    /// How long a phase waits after SIGTERM before escalating to SIGKILL.
    pub grace: Duration,
}

impl Default for ShutdownTimings {
    fn default() -> Self {
        Self::from_engine(&EngineConfig::default())
    }
}

impl ShutdownTimings {
    /// Read the timings out of an engine configuration (pure function).
    #[must_use]
    pub const fn from_engine(engine: &EngineConfig) -> Self {
        Self {
            drain: engine.shutdown_drain(),
            grace: engine.shutdown_grace(),
        }
    }
}

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

/// Actor entry tracking handle, primitive info and child liveness.
struct ActorEntry {
    /// Handle to the running actor.
    handle: ActorHandle,
    /// Kind of the primitive. It never changes, so it is kept here to avoid
    /// reading the live info just to filter by tier.
    kind: PrimitiveKind,
    /// Live primitive information, published by the actor that owns it.
    status: watch::Receiver<PrimitiveInfo>,
    /// Live child PID, or `None` when no child is running. Shutdown waits on
    /// this instead of sleeping, and reads it to escalate to SIGKILL.
    pid_rx: watch::Receiver<Option<u32>>,
}

impl ActorEntry {
    /// The primitive's information as its actor last published it.
    fn info(&self) -> PrimitiveInfo {
        self.status.borrow().clone()
    }
}

/// A primitive as the shutdown sequence sees it.
struct ShutdownTarget {
    name: String,
    handle: ActorHandle,
    pid_rx: watch::Receiver<Option<u32>>,
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
        self.register_primitive(
            runtime,
            info,
            &config.path,
            &config.args,
            &config.env,
            &config.restart,
        )
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
        self.register_primitive(
            runtime,
            info,
            &config.path,
            &config.args,
            &env,
            &config.restart,
        )
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
        self.register_primitive(
            runtime,
            info,
            &config.path,
            &config.args,
            &env,
            &config.restart,
        )
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
        supervision: &RestartConfig,
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
        let (pid_watch, pid_rx) = ChildPidWatch::new();
        let actor_config = PrimitiveActorConfig {
            info,
            path: path.to_path_buf(),
            args: args.to_vec(),
            env: env.clone(),
            socket_path: self.socket_path.clone(),
            api_port: self.api_port,
            pid_watch,
            // Configuration validation rejects unknown policies before we get
            // here, so the fallback is unreachable in practice.
            restart: supervision.policy().unwrap_or_default(),
            restart_limits: supervision.limits(),
        };

        // Build the actor (does not start it yet). The actor owns the live
        // info; the receiver is how the manager reads it.
        let (actor, status) = build_primitive_actor(runtime, actor_config);

        // Start the actor - this triggers after_start which spawns the process
        let handle = actor.start().await;

        // Store the entry
        let entry = ActorEntry {
            handle,
            kind,
            status,
            pid_rx,
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

    /// Get information about all registered primitives.
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

    /// Graceful shutdown with a coordinated, bounded drain protocol.
    ///
    /// Three phases run in order, each draining completely before the next is
    /// signalled:
    ///
    /// 1. Sources stop first, so nothing new enters the topology.
    /// 2. Handlers drain, finishing whatever the sources already produced.
    /// 3. Sinks drain last, so every `system.stopped.*` message is still
    ///    delivered somewhere.
    ///
    /// Each phase waits on its children's actual exit rather than a fixed
    /// sleep: it moves on as soon as they are all gone, and at the deadline it
    /// SIGKILLs whatever is left so no child outlives the engine.
    pub async fn graceful_shutdown(&self, broker: &ActorHandle, timings: ShutdownTimings) {
        // Tell every actor the topology is going down before anything is
        // signalled, so a child exiting during the drain is not treated as a
        // crash and respawned underneath us.
        self.announce_shutdown().await;

        // Sources cannot subscribe, so the broadcast is only for observers and
        // there is nothing to gain from a voluntary-exit window: go straight to
        // SIGTERM after announcing.
        info!("Stopping sources...");
        self.shutdown_phase(broker, PrimitiveKind::Source, timings, false)
            .await;

        info!("Draining handlers...");
        self.shutdown_phase(broker, PrimitiveKind::Handler, timings, true)
            .await;

        info!("Draining sinks...");
        self.shutdown_phase(broker, PrimitiveKind::Sink, timings, true)
            .await;

        info!("All primitives stopped.");
    }

    /// Tell every actor that the engine is shutting down.
    async fn announce_shutdown(&self) {
        let handles: Vec<ActorHandle> = {
            let actors = self.actors.read().await;
            actors.values().map(|e| e.handle.clone()).collect()
        };
        for handle in handles {
            handle.send(EngineShuttingDown).await;
        }
    }

    /// Collect the shutdown targets for one primitive kind.
    async fn targets_of_kind(&self, kind: PrimitiveKind) -> Vec<ShutdownTarget> {
        let actors = self.actors.read().await;
        actors
            .iter()
            .filter(|(_, e)| e.kind == kind)
            .map(|(name, e)| ShutdownTarget {
                name: name.clone(),
                handle: e.handle.clone(),
                pid_rx: e.pid_rx.clone(),
            })
            .collect()
    }

    /// Run one shutdown phase for a primitive kind.
    ///
    /// `allow_voluntary_exit` gives subscribers a bounded window to act on the
    /// `system.shutdown` broadcast before SIGTERM arrives. Sources do not
    /// subscribe, so that window is skipped for them.
    async fn shutdown_phase(
        &self,
        broker: &ActorHandle,
        kind: PrimitiveKind,
        timings: ShutdownTimings,
        allow_voluntary_exit: bool,
    ) {
        broker.broadcast(create_shutdown_event(kind.as_str())).await;

        let targets = self.targets_of_kind(kind).await;
        if targets.is_empty() {
            return;
        }

        let mut watched: Vec<(String, watch::Receiver<Option<u32>>)> = targets
            .iter()
            .map(|t| (t.name.clone(), t.pid_rx.clone()))
            .collect();

        // Give well-behaved subscribers a chance to exit on the broadcast alone.
        let mut alive = if allow_voluntary_exit && !timings.drain.is_zero() {
            wait_for_children_exit(&mut watched, timings.drain).await
        } else {
            still_running(&watched)
        };

        if !alive.is_empty() {
            for target in &targets {
                info!("Signaling {} to stop", target.name);
                target.handle.send(StopPrimitive).await;
            }
            alive = wait_for_children_exit(&mut watched, timings.grace).await;
        }

        // Anything still alive ignored SIGTERM. Escalate so it cannot outlive
        // the engine as an orphan.
        if !alive.is_empty() {
            for (name, pid) in &alive {
                warn!(
                    "{} (pid: {}) did not exit within {} ms of SIGTERM; sending SIGKILL",
                    name,
                    pid,
                    timings.grace.as_millis()
                );
                sigkill_process_group(*pid);
            }
            let unreaped = wait_for_children_exit(&mut watched, REAP_TIMEOUT).await;
            for (name, pid) in unreaped {
                error!(
                    "{} (pid: {}) survived SIGKILL and was not reaped",
                    name, pid
                );
            }
        }

        // Stop the actors themselves now their children are gone.
        for target in &targets {
            info!("Stopping actor {}", target.name);
            let _ = target.handle.stop().await;
        }
    }
}

/// The subset of watched primitives whose child is currently running.
fn still_running(watched: &[(String, watch::Receiver<Option<u32>>)]) -> Vec<(String, u32)> {
    watched
        .iter()
        .filter_map(|(name, rx)| rx.borrow().map(|pid| (name.clone(), pid)))
        .collect()
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
            restart: RestartConfig::default(),
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

        let timings = ShutdownTimings {
            drain: Duration::ZERO,
            grace: Duration::from_secs(2),
        };
        manager.graceful_shutdown(&runtime.broker(), timings).await;

        let after_stop = wait_for(&manager, "alive", |i| i.state != PrimitiveState::Running).await;
        let Some(after_stop) = after_stop else {
            panic!("alive stayed Running after stop_all");
        };
        assert_ne!(after_stop.state, PrimitiveState::Running);
    }

    #[tokio::test]
    async fn a_restarting_primitive_ends_failed_with_the_exhaustion_reason() {
        let mut runtime = ActonApp::launch_async().await;
        let manager = ProcessManager::new(PathBuf::from("/tmp/emergent-issue-40-restart.sock"), 0);

        let mut flapper = source_config("flapper", "/bin/false", &[]);
        flapper.restart = RestartConfig {
            restart: "on-failure".to_string(),
            restart_backoff_ms: 10,
            restart_max_backoff_ms: 20,
            restart_max_retries: 2,
            restart_window_ms: 60_000,
        };
        assert!(
            manager
                .register_source(&mut runtime, &flapper)
                .await
                .is_ok()
        );

        let exhausted = wait_for(&manager, "flapper", |i| {
            i.error
                .as_deref()
                .is_some_and(|e| e.starts_with("Restarts exhausted"))
        })
        .await;
        let Some(exhausted) = exhausted else {
            panic!(
                "flapper never reported exhaustion: {:?}",
                manager.get_info("flapper").await
            );
        };
        assert_eq!(exhausted.state, PrimitiveState::Failed);
        assert_eq!(exhausted.pid, None);

        let timings = ShutdownTimings {
            drain: Duration::ZERO,
            grace: Duration::from_secs(2),
        };
        manager.graceful_shutdown(&runtime.broker(), timings).await;
    }
}
