//! Emergent Engine - Event-based workflow platform.
//!
//! This is the main entry point for the Emergent engine, which:
//! - Loads configuration from TOML
//! - Initializes the event store (JSON logs + SQLite)
//! - Starts the IPC server for client connections
//! - Manages Source, Handler, and Sink processes via actors
//! - Handles graceful shutdown

use acton_reactive::ipc::{IpcConfig, IpcPushNotification, SubscriptionManager};
use acton_reactive::prelude::*;
use anyhow::{Context, Result};
use axum::{Json, Router, routing::get};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::writer::BoxMakeWriter;

use emergent_engine::scaffold;

/// Emergent Engine - Event-based workflow platform
#[derive(Parser, Debug)]
#[command(name = "emergent")]
#[command(version, about, long_about = None)]
#[command(after_long_help = "\
Examples:
  # Initialize a new config file
  emergent init

  # Initialize with a custom engine name
  emergent init --name my-pipeline

  # Start the engine with a config file
  emergent --config ./config/emergent.toml

  # Start with verbose logging
  emergent --config ./config/emergent.toml --verbose

  # Scaffold a new primitive (interactive wizard)
  emergent scaffold

  # Scaffold a Rust handler non-interactively
  emergent scaffold -t handler -n my_filter -l rust -S timer.tick -p timer.filtered

  # List available marketplace primitives
  emergent marketplace list

  # Install a marketplace primitive
  emergent marketplace install http-source

  # Install multiple marketplace primitives at once
  emergent marketplace install exec-source exec-handler exec-sink
")]
struct Args {
    /// Path to configuration file
    #[arg(
        short,
        long,
        value_name = "FILE",
        global = true,
        help_heading = "Global Options"
    )]
    config: Option<PathBuf>,

    /// Override the socket path from config
    #[arg(
        short,
        long,
        value_name = "PATH",
        global = true,
        help_heading = "Global Options"
    )]
    socket: Option<PathBuf>,

    /// Run in verbose mode (debug logging)
    #[arg(short, long, global = true, help_heading = "Global Options")]
    verbose: bool,

    /// Subcommand to run
    #[command(subcommand)]
    command: Option<Command>,
}

/// Available subcommands
#[derive(Subcommand, Debug)]
enum Command {
    /// Initialize a new emergent.toml configuration file
    Init(emergent_engine::init::InitArgs),
    /// Generate a new primitive (source, handler, or sink)
    Scaffold(scaffold::cli::ScaffoldArgs),
    /// Manage marketplace primitives
    Marketplace(emergent_engine::marketplace::MarketplaceArgs),
    /// Update emergent to the latest release
    Update(emergent_engine::update::UpdateArgs),
}

use emergent_engine::api_host::{describe_allowed_hosts, guard_host};
use emergent_engine::config::EmergentConfig;
use emergent_engine::declarations::{RejectionReport, rejection_event_type};
use emergent_engine::event_store::{EventStore, EventStoreError, JsonEventLog, SqliteEventStore};
use emergent_engine::ipc_identity::StubResolver;
use emergent_engine::ipc_policy::{EnginePolicy, PolicyObserver, policy_is_needed};
use emergent_engine::messages::EmergentMessage;
use emergent_engine::primitive_actor::IpcSystemEvent;
use emergent_engine::process_manager::{ProcessManager, ShutdownTimings, StartupReadiness};
use emergent_engine::publish_reply::should_reply;
use emergent_engine::readiness::StartupObserver;
use emergent_engine::retention;
use emergent_engine::topology::build_topology_payload;

// ============================================================================
// IPC Message Registration
// ============================================================================

/// Register EmergentMessage for IPC.
///
/// This allows external clients to send and receive messages through the broker.
#[acton_message(ipc)]
struct IpcEmergentMessage {
    /// The wrapped emergent message.
    inner: EmergentMessage,
}

/// Acknowledgment reply sent back to publishers using request-response mode.
///
/// When a client publishes with `expects_reply: true`, the broker sends this
/// back after storing and forwarding the message, providing backpressure.
#[acton_message]
struct PublishAck;

/// Send the broker's `PublishAck` if anybody other than the broker is waiting
/// for it.
///
/// acton addresses a fire-and-forget publish's reply at the recipient itself,
/// so replying unconditionally posts an ack into the broker's own bounded inbox
/// for every publish. See [`emergent_engine::publish_reply::should_reply`].
fn ack_publish(reply_envelope: &OutboundEnvelope) {
    let recipient = reply_envelope
        .recipient()
        .as_ref()
        .map(MessageAddress::name);
    if should_reply(reply_envelope.reply_to().name(), recipient) {
        let _ = reply_envelope.reply(PublishAck);
    }
}

impl From<EmergentMessage> for IpcEmergentMessage {
    fn from(msg: EmergentMessage) -> Self {
        Self { inner: msg }
    }
}

impl From<IpcEmergentMessage> for EmergentMessage {
    fn from(msg: IpcEmergentMessage) -> Self {
        msg.inner
    }
}

/// Payload for subscriptions response messages.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct SubscriptionsResponsePayload {
    /// The message types this primitive should subscribe to.
    subscribes: Vec<String>,
}

// ============================================================================
// Engine State
// ============================================================================

/// Internal state for the message broker actor.
#[derive(Default, Clone, Debug)]
struct MessageBrokerState {
    /// Count of messages processed.
    message_count: u64,
}

/// Event store wrapper for async operations.
struct EventStoreWrapper {
    json_log: Option<JsonEventLog>,
    sqlite: Option<SqliteEventStore>,
}

impl EventStoreWrapper {
    fn new(json_log: Option<JsonEventLog>, sqlite: Option<SqliteEventStore>) -> Self {
        Self { json_log, sqlite }
    }

    /// Returns the SQLite store, when one is configured, for retention pruning.
    fn sqlite(&self) -> Option<&SqliteEventStore> {
        self.sqlite.as_ref()
    }

    fn store(&self, message: &EmergentMessage) -> Result<(), EventStoreError> {
        if let Some(ref json_log) = self.json_log {
            json_log.store(message)?;
        }
        if let Some(ref sqlite) = self.sqlite {
            sqlite.store(message)?;
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), EventStoreError> {
        if let Some(ref json_log) = self.json_log {
            json_log.flush()?;
        }
        if let Some(ref sqlite) = self.sqlite {
            sqlite.flush()?;
        }
        Ok(())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Store and forward the `system.error.<name>` event a strict rejection owes.
///
/// Dispatched straight to the subscription manager rather than broadcast
/// through the broker, because the broker is what just refused a message:
/// sending the report back through it would put the report on the path being
/// reported on. The event is the engine's own, with the engine as its source,
/// so the publish check never sees it and a rejection cannot cascade.
fn report_rejection(
    event_store: &EventStoreWrapper,
    sub_mgr: &SubscriptionManager,
    report: &RejectionReport,
) {
    let event_type = rejection_event_type(&report.primitive);
    let message = match EmergentMessage::try_new(&event_type) {
        Ok(message) => message
            .with_source("emergent-engine")
            .with_payload(serde_json::to_value(report).unwrap_or_default()),
        Err(e) => {
            warn!(
                primitive = %report.primitive,
                error = %e,
                "Rejection could not be reported: the primitive name does not form a message type"
            );
            return;
        }
    };

    if let Err(e) = event_store.store(&message) {
        error!("Failed to store rejection event: {}", e);
    }

    let notification = IpcPushNotification::new(
        message.message_type.to_string(),
        Some(message.source.to_string()),
        serde_json::to_value(&message).unwrap_or_default(),
    );
    sub_mgr.forward_to_subscribers(&notification);
}

/// Initialize the event stores based on configuration.
fn init_event_stores(config: &EmergentConfig) -> Result<EventStoreWrapper> {
    info!(
        "Initializing JSON event log at {}",
        config.event_store.json_log_dir.display()
    );
    let json_log = Some(
        JsonEventLog::new(&config.event_store.json_log_dir)
            .context("Failed to create JSON event log")?,
    );

    info!(
        "Initializing SQLite event store at {}",
        config.event_store.sqlite_path.display()
    );
    let sqlite = Some(
        SqliteEventStore::new(&config.event_store.sqlite_path)
            .context("Failed to create SQLite event store")?,
    );

    Ok(EventStoreWrapper::new(json_log, sqlite))
}

/// Interval between retention passes after the one at startup.
const RETENTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Run one retention pass and log what it removed.
fn run_retention_pass(event_store: &EventStoreWrapper, log_dir: &Path, retention_days: u32) {
    let report = retention::prune(
        event_store.sqlite(),
        log_dir,
        retention_days,
        chrono::Utc::now(),
    );

    for failure in &report.errors {
        warn!("Retention prune problem: {}", failure);
    }

    if report.is_empty() {
        debug!("Retention prune found nothing older than {retention_days} day(s)");
        return;
    }

    let files: Vec<String> = report
        .files_deleted
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    info!(
        "Retention prune removed {} event(s) from SQLite and {} JSON log file(s) older than {} day(s){}",
        report.events_deleted,
        files.len(),
        retention_days,
        if files.is_empty() {
            String::new()
        } else {
            format!(": {}", files.join(", "))
        }
    );
}

/// Prune at startup, then once a day, unless retention is disabled.
fn start_retention(event_store: &Arc<EventStoreWrapper>, config: &EmergentConfig) {
    let retention_days = config.event_store.retention_days;
    let log_dir = config.event_store.json_log_dir.clone();

    if !retention::is_pruning_enabled(retention_days) {
        info!("Event retention disabled (retention_days = 0): nothing is pruned");
        return;
    }

    info!("Event retention: keeping {retention_days} day(s), pruning now and once a day");
    run_retention_pass(event_store, &log_dir, retention_days);

    let store = event_store.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RETENTION_INTERVAL);
        // The first tick completes immediately and is the startup pass already run.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let store = store.clone();
            let log_dir = log_dir.clone();
            // Deleting rows and files blocks, so keep it off the async runtime.
            let pass = tokio::task::spawn_blocking(move || {
                run_retention_pass(&store, &log_dir, retention_days);
            });
            if let Err(e) = pass.await {
                warn!("Retention prune task failed: {}", e);
            }
        }
    });
}

/// Load configuration from the given path or default locations.
fn load_config(path: Option<PathBuf>) -> Result<EmergentConfig> {
    let config_path = if let Some(p) = path {
        if !p.exists() {
            anyhow::bail!(
                "Configuration file not found: {}\n\n\
                 To get started, create a config file:\n  \
                 emergent init\n  \
                 emergent --config path/to/emergent.toml",
                p.display()
            );
        }
        p
    } else {
        // Check for config in current directory first
        let local = PathBuf::from("emergent.toml");
        if local.exists() {
            local
        } else if let Some(dirs) = directories::ProjectDirs::from("ai", "govcraft", "emergent") {
            let xdg_config = dirs.config_dir().join("emergent.toml");
            if xdg_config.exists() {
                xdg_config
            } else {
                anyhow::bail!(
                    "No configuration file found.\n\n\
                     Searched:\n  \
                     ./emergent.toml\n  \
                     {}\n\n\
                     To get started, create a config file:\n  \
                     emergent init\n  \
                     emergent --config path/to/emergent.toml",
                    xdg_config.display()
                );
            }
        } else {
            anyhow::bail!(
                "No configuration file found.\n\n\
                 Searched:\n  \
                 ./emergent.toml\n\n\
                 To get started, create a config file:\n  \
                 emergent init\n  \
                 emergent --config path/to/emergent.toml"
            );
        }
    };

    info!("Loading configuration from {}", config_path.display());
    EmergentConfig::load(&config_path)
        .with_context(|| format!("Failed to load {}", config_path.display()))
}

/// Create IPC configuration from the engine config.
///
/// [`IpcConfig::load`] resolves acton-reactive's own defaults and
/// `$XDG_CONFIG_HOME/acton/ipc.toml` first. The socket path always comes from
/// `[engine]`. `max_connections` overrides the resolved limit only when
/// `[engine].max_connections` is set, so leaving the key out keeps whatever
/// acton resolved.
fn create_ipc_config(socket_path: &std::path::Path, max_connections: Option<usize>) -> IpcConfig {
    let mut ipc_config = IpcConfig::load();
    ipc_config.socket.path = Some(socket_path.to_path_buf());
    if let Some(limit) = max_connections {
        ipc_config.limits.max_connections = limit;
    }
    ipc_config
}

/// Check for a stale socket file and clean it up if no listener is active.
///
/// After an unclean shutdown (kill -9, OOM, crash), the Unix socket file may
/// remain on disk. This function detects whether the socket is stale by
/// attempting a connection with a timeout:
///
/// - If the socket file does not exist, returns `Ok(())`.
/// - If a connection succeeds, another engine instance is running -- returns an error.
/// - If the connection fails or times out, the socket is stale and is removed.
async fn check_and_cleanup_stale_socket(socket_path: &Path) -> Result<()> {
    if !socket_path.exists() {
        return Ok(());
    }

    let connect_timeout = std::time::Duration::from_secs(2);
    let connect_result = tokio::time::timeout(
        connect_timeout,
        tokio::net::UnixStream::connect(socket_path),
    )
    .await;

    match connect_result {
        Ok(Ok(_stream)) => {
            // Connection succeeded -- a live listener is responding
            anyhow::bail!(
                "Another engine instance is already running (socket: {})\n\n\
                 If this is unexpected, stop the other instance first, or remove the \
                 socket manually:\n  rm {}",
                socket_path.display(),
                socket_path.display()
            );
        }
        Ok(Err(_)) | Err(_) => {
            // Connection refused or timed out -- stale socket from a previous crash
            warn!(
                "Detected stale socket from a previous unclean shutdown: {}",
                socket_path.display()
            );
            tokio::fs::remove_file(socket_path).await.with_context(|| {
                format!(
                    "Failed to remove stale socket file: {}",
                    socket_path.display()
                )
            })?;
            info!("Removed stale socket, proceeding with startup");
        }
    }

    Ok(())
}

// ============================================================================
// Main
// ============================================================================

#[acton_main]
async fn main() -> Result<()> {
    // Parse command-line arguments
    let args = Args::parse();

    // Handle subcommands first (they don't need full tracing setup)
    if let Some(command) = args.command {
        // Subcommands use stderr logging at info level
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("info,acton_reactive=off"))
            .init();

        match command {
            Command::Init(init_args) => {
                emergent_engine::init::execute(init_args).await?;
                return Ok(());
            }
            Command::Scaffold(scaffold_args) => {
                scaffold::run_scaffold(scaffold_args).await?;
                return Ok(());
            }
            Command::Marketplace(marketplace_args) => {
                emergent_engine::marketplace::execute(marketplace_args).await?;
                return Ok(());
            }
            Command::Update(update_args) => {
                emergent_engine::update::execute(update_args).await?;
                return Ok(());
            }
        }
    }

    // Load configuration (before tracing init so we know the engine name)
    let mut config = load_config(args.config).context("Failed to load configuration")?;

    // Initialize tracing
    // --verbose: log to stderr (human-readable)
    // default: log to file at ~/.local/share/emergent/<engine-name>/emergent.log
    let log_level = "info,acton_reactive=off";
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));

    if args.verbose {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        let log_dir = directories::ProjectDirs::from("ai", "govcraft", "emergent")
            .map(|dirs| dirs.data_dir().join(&config.engine.name))
            .unwrap_or_else(|| PathBuf::from("."));
        let _ = std::fs::create_dir_all(&log_dir);
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("emergent.log"));
        // Without a writable log file, log to stderr rather than lose the logs
        let writer = match log_file {
            Ok(file) => BoxMakeWriter::new(std::sync::Mutex::new(file)),
            Err(e) => {
                eprintln!(
                    "Cannot open {}: {e}. Logging to stderr instead.",
                    log_dir.join("emergent.log").display()
                );
                BoxMakeWriter::new(std::io::stderr)
            }
        };
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(writer)
            .with_ansi(false)
            .init();
    }

    // Override socket path if specified
    if let Some(socket) = args.socket {
        config.engine.socket_path = socket.display().to_string();
    }
    info!("Engine name: {}", config.engine.name);

    // wire_format is accepted for compatibility and selects nothing
    if let Some(warning) = emergent_engine::config::wire_format_warning(config.engine.wire_format) {
        warn!("{}", warning);
    }

    // Resolve socket path
    let socket_path = config.socket_path();
    info!("Socket path: {}", socket_path.display());

    // Check for stale socket from a previous unclean shutdown
    check_and_cleanup_stale_socket(&socket_path)
        .await
        .context("Socket pre-flight check failed")?;

    // Initialize event stores
    let event_store = Arc::new(init_event_stores(&config)?);

    // Enforce [event_store].retention_days: prune now, then once a day
    start_retention(&event_store, &config);

    // Initialize process manager
    let process_manager = ProcessManager::new(socket_path.clone(), config.engine.api_port);

    // Create IPC configuration with our socket path
    let ipc_config = create_ipc_config(&socket_path, config.engine.max_connections);

    // Refuse to start a topology that cannot fit under the effective connection
    // limit. This runs after IpcConfig::load so it sees the limit acton
    // actually resolved, not just the one written in emergent.toml. Without it
    // the primitives that lose the race to the accept semaphore are dropped
    // silently, and the engine reports them as running.
    let max_connections = ipc_config.limits.max_connections;
    config
        .check_connection_capacity(max_connections)
        .context("Connection capacity pre-flight check failed")?;
    info!(
        "IPC connection limit: {} ({} enabled primitive(s) plus {} reserved)",
        max_connections,
        config.enabled_primitive_count(),
        emergent_engine::config::RESERVED_IPC_CONNECTIONS
    );

    // Launch the acton runtime
    let mut runtime = ActonApp::launch_async().await;

    // Register IPC message types
    let registry = runtime.ipc_registry();
    registry.register::<IpcEmergentMessage>("EmergentMessage");
    registry.register::<IpcSystemEvent>("SystemEvent");
    info!("Registered {} IPC message type(s)", registry.len());

    // Build the declaration table before the listener, because the security
    // policy that enforces it has to be installed as the listener starts.
    let declarations = Arc::new(config.declaration_table());
    if declarations.mode().is_enforcing() {
        info!(
            "Declaration enforcement: {} ({} primitive(s) declared)",
            declarations.mode(),
            declarations.len()
        );
        // The stub resolver below names nobody, so a publish is checked
        // against the source the client writes itself and a subscribe cannot
        // be checked at all. Saying it here is the difference between an
        // operator who knows what the mode is worth and one who does not.
        warn!(
            "Declarations are checked against self-reported names: \
             publishes are held to the source on the message, subscribes are not checked"
        );
    }

    // Rejections are reported from a task of their own. `authorize` runs on
    // acton's connection task and must not block, and the subscription manager
    // it would need does not exist until the listener below has started, so the
    // policy only hands the report over and returns.
    let (rejections_tx, mut rejections_rx) = tokio::sync::mpsc::unbounded_channel();

    // Start the IPC listener first to get the subscription manager.
    //
    // The resolver is the stub one: until issue #24 lands, every peer is
    // Unmanaged and enforcement falls back to the `source` a client writes
    // itself. Swapping the stub for a real resolver is the whole of that
    // change here.
    // Startup readiness observes the subscribes the policy authorizes, which is
    // the only way the engine learns that a tier is listening. Registering an
    // observer is itself enough to install the policy, so this works with
    // enforcement off.
    let (startup_observer, startup_signals) = StartupObserver::channel();
    let startup_observer: Arc<dyn PolicyObserver> = Arc::new(startup_observer);

    let listener_handle = if policy_is_needed(declarations.mode(), true) {
        let policy = Arc::new(
            EnginePolicy::new(
                declarations.clone(),
                Arc::new(StubResolver),
                Some(startup_observer),
            )
            .reporting_to(rejections_tx),
        );
        runtime
            .start_ipc_listener_with_policy(ipc_config, policy)
            .await
    } else {
        runtime.start_ipc_listener_with_config(ipc_config).await
    }
    .context("Failed to start IPC listener")?;

    // Get subscription manager for message routing to IPC clients
    let subscription_manager = listener_handle.subscription_manager();

    // Drain the rejection reports the policy produced into the event store and
    // out to subscribers. The task ends when the policy is dropped.
    {
        let event_store_for_rejections = event_store.clone();
        let sub_mgr_for_rejections = subscription_manager.clone();
        tokio::spawn(async move {
            while let Some(report) = rejections_rx.recv().await {
                report_rejection(
                    &event_store_for_rejections,
                    &sub_mgr_for_rejections,
                    &report,
                );
            }
        });
    }

    // Create the message broker actor that:
    // 1. Logs events to the event store
    // 2. Forwards messages to IPC subscribers based on inner message_type
    let event_store_clone = event_store.clone();
    let sub_mgr_clone = subscription_manager.clone();
    let mut broker_actor =
        runtime.new_actor_with_name::<MessageBrokerState>("message_broker".to_string());

    // Handle IpcEmergentMessage (from external clients)
    // system.request.subscriptions and system.request.topology are both answered
    // directly by the engine, because the engine is the only holder of that state
    // and SDKs block on the answer.
    let event_store_for_emergent = event_store_clone.clone();
    let sub_mgr_for_emergent = sub_mgr_clone.clone();
    let pm_for_subscriptions = process_manager.clone();
    let pm_for_topology = process_manager.clone();
    broker_actor.mutate_on::<IpcEmergentMessage>(move |actor, envelope| {
        let msg = envelope.message();
        actor.model.message_count += 1;

        // A publish that broke its declarations never reaches here: the
        // security policy refuses the IPC request before acton routes it, so a
        // strict rejection leaves no trace in the store or the subscribers.

        // Log to event store
        if let Err(e) = event_store_for_emergent.store(&msg.inner) {
            error!("Failed to store event: {}", e);
        }

        info!(
            "Message #{}: {} from {}",
            actor.model.message_count, msg.inner.message_type, msg.inner.source
        );

        let sub_mgr = sub_mgr_for_emergent.clone();

        // system.request.subscriptions is handled directly by the engine
        // because SDKs need their subscription list before they can subscribe to messages
        if msg.inner.message_type.as_str() == "system.request.subscriptions" {
            let pm = pm_for_subscriptions.clone();
            let inner = msg.inner.clone();
            let reply_envelope = envelope.reply_envelope();

            return Reply::pending(async move {
                // Extract the name from the payload
                let name = inner
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // Look up the primitive's configured subscriptions
                let subscribes = if let Some(p) = pm.get_info(name).await {
                    p.subscribes
                } else {
                    Vec::new()
                };

                info!(
                    "system.request.subscriptions for '{}': {:?}",
                    name, subscribes
                );

                // Create response message with matching correlation_id
                let response_payload = SubscriptionsResponsePayload { subscribes };
                let response = EmergentMessage::new("system.response.subscriptions")
                    .with_source("emergent-engine")
                    .with_correlation_id_option(inner.correlation_id.as_ref())
                    .with_payload(serde_json::to_value(&response_payload).unwrap_or_default());

                // Send response to subscribers
                let notification = IpcPushNotification::new(
                    response.message_type.to_string(),
                    Some(response.source.to_string()),
                    serde_json::to_value(&response).unwrap_or_default(),
                );
                sub_mgr.forward_to_subscribers(&notification);

                // A request is a publish like any other, so it gets the same
                // acknowledgement. Without it a client that used publish_ack is
                // told its request failed although the engine just served it.
                ack_publish(&reply_envelope);
            });
        }

        // system.request.topology is handled directly by the engine because the
        // process manager is the only source of truth for primitive state. The
        // response carries the request's correlation_id on the envelope, which is
        // where every SDK matches it.
        if msg.inner.message_type.as_str() == "system.request.topology" {
            let pm = pm_for_topology.clone();
            let inner = msg.inner.clone();
            let reply_envelope = envelope.reply_envelope();

            return Reply::pending(async move {
                let payload = build_topology_payload(std::process::id(), pm.list_all().await);

                info!(
                    "system.request.topology from '{}': {} primitive(s)",
                    inner.source,
                    payload.primitives.len()
                );

                let response = EmergentMessage::new("system.response.topology")
                    .with_source("emergent-engine")
                    .with_correlation_id_option(inner.correlation_id.as_ref())
                    .with_payload(serde_json::to_value(&payload).unwrap_or_default());

                let notification = IpcPushNotification::new(
                    response.message_type.to_string(),
                    Some(response.source.to_string()),
                    serde_json::to_value(&response).unwrap_or_default(),
                );
                sub_mgr.forward_to_subscribers(&notification);

                // A request is a publish like any other, so it gets the same
                // acknowledgement. Without it a client that used publish_ack is
                // told its request failed although the engine just served it.
                ack_publish(&reply_envelope);
            });
        }

        // Forward all other messages to IPC subscribers based on the inner message_type
        let notification = IpcPushNotification::new(
            msg.inner.message_type.to_string(),
            Some(msg.inner.source.to_string()),
            serde_json::to_value(&msg.inner).unwrap_or_default(),
        );

        // Debug: log subscription state before forwarding
        debug!(
            "Forwarding '{}': {} connections, {} types, {} total subs",
            msg.inner.message_type,
            sub_mgr.connection_count(),
            sub_mgr.subscribed_types_count(),
            sub_mgr.total_subscriptions()
        );

        sub_mgr.forward_to_subscribers(&notification);

        // Acknowledge the publish for a client that is waiting on one. A
        // fire-and-forget publish is not: acton addressed its reply back at the
        // broker, and delivering it would cost a task and an inbox slot for a
        // message no handler accepts.
        ack_publish(&envelope.reply_envelope());

        Reply::ready()
    });

    // Handle IpcSystemEvent (from PrimitiveActors)
    let event_store_for_system = event_store_clone.clone();
    let sub_mgr_for_system = sub_mgr_clone.clone();
    broker_actor.mutate_on::<IpcSystemEvent>(move |actor, envelope| {
        let msg = envelope.message();
        actor.model.message_count += 1;

        // Log to event store
        if let Err(e) = event_store_for_system.store(&msg.inner) {
            error!("Failed to store system event: {}", e);
        }

        info!(
            "System event #{}: {} from {}",
            actor.model.message_count, msg.inner.message_type, msg.inner.source
        );

        // Forward to IPC subscribers based on the inner message_type
        let notification = IpcPushNotification::new(
            msg.inner.message_type.to_string(),
            Some(msg.inner.source.to_string()),
            serde_json::to_value(&msg.inner).unwrap_or_default(),
        );
        sub_mgr_for_system.forward_to_subscribers(&notification);

        Reply::ready()
    });

    // Subscribe to IpcSystemEvent broadcasts from PrimitiveActors
    // This is REQUIRED - .mutate_on() only defines the handler, not the subscription
    broker_actor.handle().subscribe::<IpcSystemEvent>().await;

    let broker_handle = broker_actor.start().await;
    runtime
        .ipc_expose("message_broker", broker_handle.clone())
        .map_err(|e| anyhow::anyhow!("Failed to expose the message broker over IPC: {e}"))?;

    // Wait a moment for the listener to be ready
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    if acton_reactive::ipc::socket_exists(&socket_path) {
        info!("IPC socket ready: {}", socket_path.display());
    } else {
        error!("IPC socket not created: {}", socket_path.display());
    }

    // Start HTTP API server for direct topology queries
    // This allows handlers to query topology without going through pub/sub
    let api_port = config.engine.api_port;
    if config.engine.api_enabled() {
        let pm_for_http = process_manager.clone();
        let allowed_hosts = Arc::new(config.allowed_api_hosts());
        let described = Arc::clone(&allowed_hosts);
        tokio::spawn(async move {
            let app = Router::new()
                .route(
                    "/api/topology",
                    get(move || {
                        let pm = pm_for_http.clone();
                        async move {
                            // Same pure mapping the pub/sub answer uses, so the two
                            // transports always report the same topology.
                            let payload =
                                build_topology_payload(std::process::id(), pm.list_all().await);

                            info!(
                                "HTTP /api/topology: {} primitive(s)",
                                payload.primitives.len()
                            );

                            Json(payload)
                        }
                    }),
                )
                // The API has no authentication, so it answers only to host
                // names it knows to be its own. Without this a page on any
                // domain can rebind that domain to 127.0.0.1 and read the
                // whole topology from the browser.
                .layer(axum::middleware::from_fn(move |request, next| {
                    guard_host(Arc::clone(&allowed_hosts), request, next)
                }));

            let bind_addr = format!("127.0.0.1:{api_port}");
            match TcpListener::bind(&bind_addr).await {
                Ok(listener) => {
                    info!("HTTP API server listening on http://{}", bind_addr);
                    info!(
                        "HTTP API answers to: {}",
                        describe_allowed_hosts(&described)
                    );
                    if let Err(e) = axum::serve(listener, app).await {
                        error!("HTTP API server error: {}", e);
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to bind HTTP API server to {}: {}. Set api_port to a different value in [engine] config, or set api_port = 0 to disable the HTTP API.",
                        bind_addr, e
                    );
                }
            }
        });
    } else {
        info!("HTTP API server disabled (api_port = 0)");
    }

    // Collect enabled primitives
    let enabled_sinks: Vec<_> = config.sinks.iter().filter(|s| s.enabled).collect();
    let enabled_handlers: Vec<_> = config.handlers.iter().filter(|h| h.enabled).collect();
    let enabled_sources: Vec<_> = config.sources.iter().filter(|s| s.enabled).collect();

    let total_primitives = enabled_sinks.len() + enabled_handlers.len() + enabled_sources.len();
    info!("Starting {} primitive(s)...", total_primitives);

    // Startup waits for each tier to reach the engine before starting the next.
    let startup_readiness = StartupReadiness {
        timeout: config.engine.startup_ready_timeout(),
        signals: Some(startup_signals),
    };

    // Start all registered processes in order: Sinks → Handlers → Sources
    // Each primitive is started by its actor in after_start, which broadcasts system.started.*
    if total_primitives > 0
        && let Err(e) = process_manager
            .start_all(
                &mut runtime,
                &enabled_sinks,
                &enabled_handlers,
                &enabled_sources,
                &startup_readiness,
            )
            .await
    {
        error!("Failed to start some processes: {}", e);
    }

    info!("Engine ready - socket: {}", socket_path.display());

    // Wait for Ctrl+C or SIGTERM
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;

        tokio::select! {
            _ = sigterm.recv() => {
                info!("Received SIGTERM");
            }
            _ = sigint.recv() => {
                info!("Received SIGINT (Ctrl+C)");
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .context("Failed to listen for Ctrl+C")?;
    }

    info!("Shutdown signal received...");

    // Broadcast system.shutdown.requested before teardown begins.
    // This gives primitives a window to clean up (close connections,
    // kill background processes, flush buffers) before the drain starts.
    // Unlike system.shutdown, the SDK does NOT intercept this — it reaches
    // user code like any normal message.
    let broker = runtime.broker();
    let requested_event = IpcSystemEvent {
        inner: EmergentMessage::new("system.shutdown.requested")
            .with_source("emergent-engine")
            .with_payload(serde_json::json!({})),
    };
    broker.broadcast(requested_event).await;
    info!("Broadcast system.shutdown.requested");

    // Graceful shutdown with coordinated drain protocol
    // Sources → Handlers → Sinks (each tier drains before the next)
    process_manager
        .graceful_shutdown(&broker, ShutdownTimings::from_engine(&config.engine))
        .await;

    // Flush event stores
    info!("Flushing event stores...");
    if let Err(e) = event_store.flush() {
        error!("Failed to flush event stores: {}", e);
    }

    // Stop the IPC listener
    listener_handle.stop();
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Shutdown the runtime
    runtime.shutdown_all().await?;

    info!("Emergent Engine shutdown complete.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener as StdUnixListener;
    use tempfile::TempDir;

    #[tokio::test]
    async fn stale_socket_check_no_file_is_ok() -> Result<()> {
        let dir = TempDir::new()?;
        let socket_path = dir.path().join("nonexistent.sock");

        check_and_cleanup_stale_socket(&socket_path).await?;
        Ok(())
    }

    #[tokio::test]
    async fn stale_socket_check_removes_stale_file() -> Result<()> {
        let dir = TempDir::new()?;
        let socket_path = dir.path().join("stale.sock");

        // Create a socket, then drop the listener so it becomes stale
        {
            let _listener = StdUnixListener::bind(&socket_path)?;
        }
        assert!(socket_path.exists());

        check_and_cleanup_stale_socket(&socket_path).await?;
        assert!(
            !socket_path.exists(),
            "stale socket should have been removed"
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_socket_check_errors_when_listener_active() -> Result<()> {
        let dir = TempDir::new()?;
        let socket_path = dir.path().join("active.sock");

        // Keep the listener alive so the socket is not stale
        let _listener = StdUnixListener::bind(&socket_path)?;
        assert!(socket_path.exists());

        let result = check_and_cleanup_stale_socket(&socket_path).await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().context("expected error")?);
        assert!(
            err_msg.contains("Another engine instance is already running"),
            "expected 'already running' error, got: {err_msg}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_socket_check_removes_non_socket_file() -> Result<()> {
        let dir = TempDir::new()?;
        let socket_path = dir.path().join("not-a-socket.sock");

        // Create a regular file (not a socket) -- should be treated as stale
        std::fs::write(&socket_path, "not a socket")?;
        assert!(socket_path.exists());

        check_and_cleanup_stale_socket(&socket_path).await?;
        assert!(
            !socket_path.exists(),
            "non-socket file should have been removed"
        );
        Ok(())
    }
}
