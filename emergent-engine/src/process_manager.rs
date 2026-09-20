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
use crate::ipc_identity::{ChildExitObserver, SpawnedChildren};
use crate::primitive_actor::{
    ChildPidWatch, EngineShuttingDown, PrimitiveActorConfig, StopPrimitive, build_primitive_actor,
    create_shutdown_event, sigkill_process_group, wait_for_children_exit,
};
use crate::primitives::{PrimitiveInfo, PrimitiveKind, PrimitiveState};
use crate::readiness::{Ledger, SubscribeSignals, TierMember, absorb, classify, evaluate_tier};
use acton_reactive::prelude::*;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{RwLock, watch};
use tracing::{debug, error, info, warn};

/// How long to wait for a SIGKILLed child to be reaped before giving up.
///
/// SIGKILL is not catchable, so this only covers the kernel delivering the
/// signal and the monitor task observing the exit.
const REAP_TIMEOUT: Duration = Duration::from_millis(500);

/// How often the startup wait re-reads what it can observe about a tier.
///
/// Short enough that a healthy tier is left almost the instant it is ready,
/// and the loop only runs while a tier is actually being waited on.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(2);

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

/// What startup waits on between tiers.
///
/// `timeout` bounds the wait; `signals` is the stream of subscribes the
/// engine's IPC policy observed. Without it nothing can ever be confirmed, so
/// every tier that declares subscriptions runs to its deadline.
pub struct StartupReadiness {
    /// Deadline for one tier, from `[engine].startup_ready_timeout_ms`.
    pub timeout: Duration,
    /// Subscribes the policy authorized, as [`crate::readiness`] describes.
    pub signals: Option<SubscribeSignals>,
}

impl StartupReadiness {
    /// Readiness with no subscribe signal, for tests and for a process manager
    /// driven without an IPC policy.
    #[must_use]
    pub const fn unobserved(timeout: Duration) -> Self {
        Self {
            timeout,
            signals: None,
        }
    }

    /// Everything the policy has observed since the last call.
    async fn drain(&self) -> Vec<crate::readiness::ObservedSubscriber> {
        match self.signals.as_ref() {
            Some(signals) => signals.drain().await,
            None => Vec::new(),
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
    /// Told whenever a primitive's child exits, if anything asked to be.
    child_exits: Option<Arc<dyn ChildExitObserver>>,
}

impl ProcessManager {
    /// Create a new process manager.
    #[must_use]
    pub fn new(socket_path: PathBuf, api_port: u16) -> Self {
        Self {
            socket_path,
            api_port,
            actors: Arc::new(RwLock::new(HashMap::new())),
            child_exits: None,
        }
    }

    /// Report every child exit to this observer.
    ///
    /// The engine's connection registry uses it to revoke the IPC connections
    /// a primitive held, so that a name the engine vouched for dies with the
    /// process it was vouching for.
    #[must_use]
    pub fn observing_child_exits(mut self, observer: Arc<dyn ChildExitObserver>) -> Self {
        self.child_exits = Some(observer);
        self
    }

    /// Register a source from configuration.
    ///
    /// Creates a PrimitiveActor for the source but does not start it.
    /// Call `start_all` to begin the lifecycle.
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

        self.watch_child_exits(&name, pid_rx.clone());

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
    ///
    /// Each tier is waited on before the next one starts, so a slow-starting
    /// subscriber does not miss the first events. The wait is bounded by
    /// `ready_timeout`; see [`Self::wait_for_tier`].
    pub async fn start_all(
        &self,
        runtime: &mut ActorRuntime,
        sinks: &[&SinkConfig],
        handlers: &[&HandlerConfig],
        sources: &[&SourceConfig],
        readiness: &StartupReadiness,
    ) -> Result<(), ProcessManagerError> {
        // Register and start sinks first
        for sink in sinks {
            info!("Starting sink: {}", sink.name);
            if let Err(e) = self.register_sink(runtime, sink).await {
                error!("Failed to start sink {}: {}", sink.name, e);
                return Err(e);
            }
        }
        self.wait_for_tier(PrimitiveKind::Sink, readiness).await;

        // Then handlers
        for handler in handlers {
            info!("Starting handler: {}", handler.name);
            if let Err(e) = self.register_handler(runtime, handler).await {
                error!("Failed to start handler {}: {}", handler.name, e);
                return Err(e);
            }
        }
        self.wait_for_tier(PrimitiveKind::Handler, readiness).await;

        // Finally sources
        for source in sources {
            info!("Starting source: {}", source.name);
            if let Err(e) = self.register_source(runtime, source).await {
                error!("Failed to start source {}: {}", source.name, e);
                return Err(e);
            }
        }

        Ok(())
    }

    /// Wait until every primitive of `kind` that declares subscriptions has
    /// been heard from, or until `timeout` passes.
    ///
    /// Sources declare no subscriptions, so their tier returns at once and no
    /// caller has to special-case them. At the deadline the engine names the
    /// primitives it never heard from and carries on: one that never connects
    /// costs the tier its deadline, not the engine its startup.
    ///
    /// The decision is [`evaluate_tier`]; this is the loop that feeds it. The
    /// signal it feeds on is a subscribe the engine's IPC policy authorized,
    /// reported through [`crate::readiness::StartupObserver`] and attributed to
    /// a name by [`absorb`]. [`crate::readiness`] documents what that rests on.
    async fn wait_for_tier(&self, kind: PrimitiveKind, readiness: &StartupReadiness) {
        let timeout = readiness.timeout;
        // A zero deadline is an operator opting out, not a tier that failed to
        // become ready, so it earns no warning.
        if timeout.is_zero() {
            return;
        }
        let started = Instant::now();
        let mut ledger = Ledger::default();
        loop {
            let observed = readiness.drain().await;
            ledger = absorb(ledger, observed, &self.live_child_pids().await);
            let members = self.tier_members(kind, &ledger.confirmed).await;
            let verdict = evaluate_tier(&members);
            if verdict.ready {
                if !members.is_empty() {
                    debug!(
                        "{} tier ready after {} ms",
                        kind.as_str(),
                        started.elapsed().as_millis()
                    );
                }
                return;
            }

            if started.elapsed() >= timeout {
                warn!(
                    "Starting the next tier after {} ms without hearing from {} {}(s): {}. \
                     Events published now may not reach them. Raise \
                     [engine].startup_ready_timeout_ms if they are only slow to start.",
                    timeout.as_millis(),
                    verdict.waiting_on.len(),
                    kind.as_str(),
                    verdict.waiting_on.join(", ")
                );
                return;
            }

            tokio::time::sleep(READY_POLL_INTERVAL).await;
        }
    }

    /// Tell the child-exit observer whenever this primitive's child goes away.
    ///
    /// The pid watch is cleared by the monitor task the moment `Child::wait`
    /// returns, before the exit is reported to the actor and long before any
    /// restart backoff elapses, so a revocation driven from here cannot reach
    /// the connections of a replacement child. The task ends when the actor
    /// drops its sender.
    fn watch_child_exits(&self, name: &str, mut pid_rx: watch::Receiver<Option<u32>>) {
        let Some(observer) = self.child_exits.clone() else {
            return;
        };
        let name = name.to_owned();
        tokio::spawn(async move {
            let mut live = *pid_rx.borrow_and_update();
            while pid_rx.changed().await.is_ok() {
                let current = *pid_rx.borrow_and_update();
                if let (Some(pid), None) = (live, current) {
                    observer.on_child_exit(&name, pid);
                }
                live = current;
            }
        });
    }

    /// The pid of every child the engine has spawned, by primitive name.
    ///
    /// One table, read by two callers: [`crate::readiness::attribute`] turns a
    /// subscriber's pid into a name with it, and
    /// [`crate::ipc_identity::AncestryResolver`] walks a connecting peer's
    /// ancestry into it.
    pub async fn live_child_pids(&self) -> HashMap<u32, String> {
        let actors = self.actors.read().await;
        actors
            .iter()
            .filter_map(|(name, e)| e.pid_rx.borrow().map(|pid| (pid, name.clone())))
            .collect()
    }

    /// Snapshot the primitives of one kind as the readiness decision sees them.
    async fn tier_members(
        &self,
        kind: PrimitiveKind,
        confirmed: &BTreeSet<String>,
    ) -> Vec<TierMember> {
        let actors = self.actors.read().await;
        actors
            .values()
            .filter(|e| e.kind == kind)
            .map(|e| {
                let info = e.info();
                let running =
                    !matches!(info.state, PrimitiveState::Stopped | PrimitiveState::Failed);
                TierMember {
                    declares_subscriptions: !info.subscribes.is_empty(),
                    evidence: classify(running, confirmed.contains(&info.name)),
                    name: info.name,
                }
            })
            .collect()
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

#[async_trait::async_trait]
impl SpawnedChildren for ProcessManager {
    async fn child_names_by_pid(&self) -> HashMap<u32, String> {
        self.live_child_pids().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readiness::{DISCOVERY_RESPONSE_TOPIC, ObservedSubscriber, StartupObserver};

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

    /// Build a sink config for a real executable.
    fn sink_config(name: &str, path: &str, args: &[&str], subscribes: &[&str]) -> SinkConfig {
        SinkConfig {
            name: name.to_string(),
            path: PathBuf::from(path),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            enabled: true,
            subscribes: subscribes.iter().map(|s| (*s).to_string()).collect(),
            env: HashMap::new(),
            unwrap_stdout: false,
            restart: RestartConfig::default(),
        }
    }

    /// `startup_ready_timeout_ms = 0` opts out of the wait entirely, which is
    /// the pre-0.10.10 behaviour minus the sleeps.
    #[tokio::test]
    async fn a_zero_deadline_skips_the_wait() {
        let mut runtime = ActonApp::launch_async().await;
        let manager = ProcessManager::new(PathBuf::from("/tmp/emergent-issue-66-zero.sock"), 0);
        let readiness = StartupReadiness::unobserved(Duration::ZERO);

        let mute = sink_config("mute", "/bin/sleep", &["30"], &["burst.event"]);

        let started = Instant::now();
        assert!(
            manager
                .start_all(&mut runtime, &[&mute], &[], &[], &readiness)
                .await
                .is_ok()
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "a zero deadline still waited: {:?}",
            started.elapsed()
        );

        let timings = ShutdownTimings {
            drain: Duration::ZERO,
            grace: Duration::from_secs(2),
        };
        manager.graceful_shutdown(&runtime.broker(), timings).await;
    }

    /// Govcraft/emergent#66: the wait must be bounded. `/bin/sleep` never
    /// connects to the engine, so nothing will ever satisfy the tier and only
    /// the deadline can end it.
    #[tokio::test]
    async fn a_primitive_that_never_connects_holds_its_tier_only_to_the_deadline() {
        let mut runtime = ActonApp::launch_async().await;
        let manager = ProcessManager::new(PathBuf::from("/tmp/emergent-issue-66-deadline.sock"), 0);
        let readiness = StartupReadiness::unobserved(Duration::from_millis(200));

        let mute = sink_config("mute", "/bin/sleep", &["30"], &["burst.event"]);

        let started = Instant::now();
        assert!(
            manager
                .start_all(&mut runtime, &[&mute], &[], &[], &readiness)
                .await
                .is_ok()
        );
        let waited = started.elapsed();

        assert!(
            waited >= Duration::from_millis(200),
            "start_all returned before the deadline: {waited:?}"
        );
        assert!(
            waited < Duration::from_secs(2),
            "start_all overran the deadline: {waited:?}"
        );

        let timings = ShutdownTimings {
            drain: Duration::ZERO,
            grace: Duration::from_secs(2),
        };
        manager.graceful_shutdown(&runtime.broker(), timings).await;
    }

    /// A sink that declares no subscriptions has nothing to be ready for, so
    /// its tier must not wait at all.
    #[tokio::test]
    async fn a_tier_with_nothing_to_subscribe_does_not_wait() {
        let mut runtime = ActonApp::launch_async().await;
        let manager = ProcessManager::new(PathBuf::from("/tmp/emergent-issue-66-nowait.sock"), 0);
        let readiness = StartupReadiness::unobserved(Duration::from_secs(30));

        let quiet = sink_config("quiet", "/bin/sleep", &["30"], &[]);

        let started = Instant::now();
        assert!(
            manager
                .start_all(&mut runtime, &[&quiet], &[], &[], &readiness)
                .await
                .is_ok()
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a tier with no subscribers waited: {:?}",
            started.elapsed()
        );

        let timings = ShutdownTimings {
            drain: Duration::ZERO,
            grace: Duration::from_secs(2),
        };
        manager.graceful_shutdown(&runtime.broker(), timings).await;
    }

    /// A primitive that exits during the wait can never become ready, so it
    /// must release the tier instead of holding it to the deadline.
    #[tokio::test]
    async fn a_primitive_that_exits_during_the_wait_releases_its_tier() {
        let mut runtime = ActonApp::launch_async().await;
        let manager = ProcessManager::new(PathBuf::from("/tmp/emergent-issue-66-exits.sock"), 0);
        let readiness = StartupReadiness::unobserved(Duration::from_secs(30));

        let gone = sink_config("gone", "/bin/true", &[], &["burst.event"]);

        let started = Instant::now();
        assert!(
            manager
                .start_all(&mut runtime, &[&gone], &[], &[], &readiness)
                .await
                .is_ok()
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "an exited primitive held its tier: {:?}",
            started.elapsed()
        );

        let timings = ShutdownTimings {
            drain: Duration::ZERO,
            grace: Duration::from_secs(2),
        };
        manager.graceful_shutdown(&runtime.broker(), timings).await;
    }

    /// A subscribe the policy reported releases the tier at once, instead of
    /// the tier running to its deadline as it would with no signal at all.
    #[tokio::test]
    async fn an_observed_subscribe_releases_its_tier() {
        let mut runtime = ActonApp::launch_async().await;
        let manager = ProcessManager::new(PathBuf::from("/tmp/emergent-issue-66-observed.sock"), 0);

        let (observer, signals) = StartupObserver::channel();
        let readiness = StartupReadiness {
            timeout: Duration::from_secs(30),
            signals: Some(signals),
        };

        // The policy reports the subscribe before the tier is waited on, which
        // is the ordering the channel exists to tolerate.
        observer.note_subscribe(
            ObservedSubscriber::Named("mute".to_string()),
            &["burst.event".to_string(), "system.shutdown".to_string()],
        );

        let mute = sink_config("mute", "/bin/sleep", &["30"], &["burst.event"]);

        let started = Instant::now();
        assert!(
            manager
                .start_all(&mut runtime, &[&mute], &[], &[], &readiness)
                .await
                .is_ok()
        );
        let waited = started.elapsed();

        assert!(
            waited < Duration::from_secs(5),
            "the tier waited although its subscribe was observed: {waited:?}"
        );

        let timings = ShutdownTimings {
            drain: Duration::ZERO,
            grace: Duration::from_secs(2),
        };
        manager.graceful_shutdown(&runtime.broker(), timings).await;
    }

    /// The discovery connection is not a subscription, so it must not release
    /// a tier that has not really subscribed.
    #[tokio::test]
    async fn a_discovery_only_report_does_not_release_its_tier() {
        let mut runtime = ActonApp::launch_async().await;
        let manager =
            ProcessManager::new(PathBuf::from("/tmp/emergent-issue-66-discovery.sock"), 0);

        let (observer, signals) = StartupObserver::channel();
        let readiness = StartupReadiness {
            timeout: Duration::from_millis(200),
            signals: Some(signals),
        };
        observer.note_subscribe(
            ObservedSubscriber::Named("mute".to_string()),
            &[DISCOVERY_RESPONSE_TOPIC.to_string()],
        );

        let mute = sink_config("mute", "/bin/sleep", &["30"], &["burst.event"]);

        let started = Instant::now();
        assert!(
            manager
                .start_all(&mut runtime, &[&mute], &[], &[], &readiness)
                .await
                .is_ok()
        );
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "a discovery-only report released the tier"
        );

        let timings = ShutdownTimings {
            drain: Duration::ZERO,
            grace: Duration::from_secs(2),
        };
        manager.graceful_shutdown(&runtime.broker(), timings).await;
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
