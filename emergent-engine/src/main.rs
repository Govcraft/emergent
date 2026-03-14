//! Emergent Engine - Event-based workflow platform.
//!
//! This is the main entry point for the Emergent engine, which:
//! - Loads configuration from TOML
//! - Initializes the event store (JSON logs + SQLite)
//! - Starts the IPC server for client connections
//! - Manages Source, Handler, and Sink processes via actors
//! - Handles graceful shutdown

use acton_reactive::ipc::{IpcConfig, IpcPushNotification};
use acton_reactive::prelude::*;
use anyhow::{Context, Result};
use axum::{Json, Router, routing::get};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

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

use emergent_engine::config::EmergentConfig;
use emergent_engine::event_store::{EventStore, EventStoreError, JsonEventLog, SqliteEventStore};
use emergent_engine::messages::EmergentMessage;
use emergent_engine::primitive_actor::IpcSystemEvent;
use emergent_engine::process_manager::ProcessManager;

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

/// Information about a primitive in the topology response.
///
/// Used in `system.response.topology` message payloads.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct TopologyPrimitive {
    /// Unique name of the primitive.
    name: String,
    /// Kind of primitive (source, handler, sink).
    kind: String,
    /// Current state (running, stopped, failed, etc.).
    state: String,
    /// Message types this primitive publishes.
    publishes: Vec<String>,
    /// Message types this primitive subscribes to.
    subscribes: Vec<String>,
    /// Process ID if running.
    pid: Option<u32>,
    /// Error message if failed.
    error: Option<String>,
}

/// Payload for topology response messages.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct TopologyResponsePayload {
    /// All primitives in the system.
    primitives: Vec<TopologyPrimitive>,
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
    Ok(EmergentConfig::load(&config_path)?)
}

/// Create IPC configuration with the socket path from engine config.
fn create_ipc_config(socket_path: &std::path::Path) -> IpcConfig {
    let mut ipc_config = IpcConfig::load();
    ipc_config.socket.path = Some(socket_path.to_path_buf());
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
            .open(log_dir.join("emergent.log"))
            .unwrap_or_else(|_| {
                std::fs::File::open("/dev/null").unwrap_or_else(|_| panic!("cannot open /dev/null"))
            });
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(std::sync::Mutex::new(log_file))
            .with_ansi(false)
            .init();
    }

    // Override socket path if specified
    if let Some(socket) = args.socket {
        config.engine.socket_path = socket.display().to_string();
    }
    info!("Engine name: {}", config.engine.name);

    // Resolve socket path
    let socket_path = config.socket_path();
    info!("Socket path: {}", socket_path.display());

    // Check for stale socket from a previous unclean shutdown
    check_and_cleanup_stale_socket(&socket_path)
        .await
        .context("Socket pre-flight check failed")?;

    // Initialize event stores
    let event_store = Arc::new(init_event_stores(&config)?);

    // Initialize process manager
    let process_manager = ProcessManager::new(socket_path.clone(), config.engine.api_port);

    // Create IPC configuration with our socket path
    let ipc_config = create_ipc_config(&socket_path);

    // Launch the acton runtime
    let mut runtime = ActonApp::launch_async().await;

    // Register IPC message types
    let registry = runtime.ipc_registry();
    registry.register::<IpcEmergentMessage>("EmergentMessage");
    registry.register::<IpcSystemEvent>("SystemEvent");
    info!("Registered {} IPC message type(s)", registry.len());

    // Start the IPC listener first to get the subscription manager
    let listener_handle = runtime
        .start_ipc_listener_with_config(ipc_config)
        .await
        .context("Failed to start IPC listener")?;

    // Get subscription manager for message routing to IPC clients
    let subscription_manager = listener_handle.subscription_manager();

    // Create the message broker actor that:
    // 1. Logs events to the event store
    // 2. Forwards messages to IPC subscribers based on inner message_type
    let event_store_clone = event_store.clone();
    let sub_mgr_clone = subscription_manager.clone();
    let mut broker_actor =
        runtime.new_actor_with_name::<MessageBrokerState>("message_broker".to_string());

    // Handle IpcEmergentMessage (from external clients)
    // system.request.subscriptions is handled directly by the engine (SDK needs it)
    // system.request.topology flows through normal pub/sub to topology-query handler
    let event_store_for_emergent = event_store_clone.clone();
    let sub_mgr_for_emergent = sub_mgr_clone.clone();
    let pm_for_subscriptions = process_manager.clone();
    broker_actor.mutate_on::<IpcEmergentMessage>(move |actor, envelope| {
        let msg = envelope.message();
        actor.model.message_count += 1;

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
            });
        }

        // Forward all other messages to IPC subscribers based on the inner message_type
        // This includes system.request.topology which goes to the topology-query handler
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
    runtime.ipc_expose("message_broker", broker_handle.clone());

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
        tokio::spawn(async move {
            let app = Router::new().route(
                "/api/topology",
                get(move || {
                    let pm = pm_for_http.clone();
                    async move {
                        // Build the engine primitive
                        let engine_primitive = TopologyPrimitive {
                            name: "emergent-engine".to_string(),
                            kind: "source".to_string(),
                            state: "running".to_string(),
                            publishes: vec![
                                "system.started.*".to_string(),
                                "system.stopped.*".to_string(),
                                "system.error.*".to_string(),
                                "system.shutdown".to_string(),
                            ],
                            subscribes: vec![],
                            pid: Some(std::process::id()),
                            error: None,
                        };

                        // Get all registered primitives
                        let all_primitives = pm.list_all().await;

                        let mut primitives: Vec<TopologyPrimitive> =
                            Vec::with_capacity(all_primitives.len() + 1);
                        primitives.push(engine_primitive);
                        primitives.extend(all_primitives.into_iter().map(|p| TopologyPrimitive {
                            name: p.name,
                            kind: p.kind.to_string().to_lowercase(),
                            state: p.state.to_string().to_lowercase(),
                            publishes: p.publishes,
                            subscribes: p.subscribes,
                            pid: p.pid,
                            error: p.error,
                        }));

                        info!("HTTP /api/topology: {} primitive(s)", primitives.len());

                        Json(TopologyResponsePayload { primitives })
                    }
                }),
            );

            let bind_addr = format!("127.0.0.1:{api_port}");
            match TcpListener::bind(&bind_addr).await {
                Ok(listener) => {
                    info!("HTTP API server listening on http://{}", bind_addr);
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

    // Start all registered processes in order: Sinks → Handlers → Sources
    // Each primitive is started by its actor in after_start, which broadcasts system.started.*
    if total_primitives > 0
        && let Err(e) = process_manager
            .start_all(
                &mut runtime,
                &enabled_sinks,
                &enabled_handlers,
                &enabled_sources,
            )
            .await
    {
        error!("Failed to start some processes: {}", e);
    }

    info!(
        "Engine ready - socket: {} wire_format: {:?}",
        socket_path.display(),
        config.engine.wire_format
    );

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

    // Graceful shutdown with coordinated drain protocol
    // Sources → Handlers → Sinks (each tier drains before the next)
    let broker = runtime.broker();
    process_manager.graceful_shutdown(&broker).await;

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
