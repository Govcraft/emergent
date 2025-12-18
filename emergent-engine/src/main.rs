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
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

/// Emergent Engine - Event-based workflow platform
#[derive(Parser, Debug)]
#[command(name = "emergent")]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Override the socket path from config
    #[arg(short, long, value_name = "PATH")]
    socket: Option<PathBuf>,

    /// Run in verbose mode (debug logging)
    #[arg(short, long)]
    verbose: bool,
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

/// Request for a primitive's configured subscriptions.
///
/// SDKs send this to get the subscription types from config before subscribing.
#[acton_message(ipc)]
#[derive(Clone)]
struct GetSubscriptions {
    /// Name of the primitive requesting its subscriptions.
    name: String,
}

/// Response containing configured subscription types.
#[acton_message(ipc)]
#[derive(Clone)]
struct SubscriptionsResponse {
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

/// Empty state for config service actor.
#[derive(Default, Clone, Debug)]
struct ConfigServiceState;

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
    let config_path = path.unwrap_or_else(|| {
        // Check for config in current directory first
        let local = PathBuf::from("emergent.toml");
        if local.exists() {
            return local;
        }

        // Then check XDG config directory
        if let Some(dirs) = directories::ProjectDirs::from("ai", "govcraft", "emergent") {
            let xdg_config = dirs.config_dir().join("emergent.toml");
            if xdg_config.exists() {
                return xdg_config;
            }
        }

        // Default to current directory
        local
    });

    info!("Loading configuration from {}", config_path.display());
    Ok(EmergentConfig::load(&config_path)?)
}

/// Create IPC configuration with the socket path from engine config.
fn create_ipc_config(socket_path: &std::path::Path) -> IpcConfig {
    let mut ipc_config = IpcConfig::load();
    ipc_config.socket.path = Some(socket_path.to_path_buf());
    ipc_config
}

// ============================================================================
// Main
// ============================================================================

#[acton_main]
async fn main() -> Result<()> {
    // Parse command-line arguments
    let args = Args::parse();

    // Initialize tracing
    let log_level = if args.verbose {
        "debug,acton=debug"
    } else {
        "info,acton=info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level)),
        )
        .init();

    // Load configuration
    let mut config = load_config(args.config).context("Failed to load configuration")?;

    // Override socket path if specified
    if let Some(socket) = args.socket {
        config.engine.socket_path = socket.display().to_string();
    }
    info!("Engine name: {}", config.engine.name);

    // Resolve socket path
    let socket_path = config.socket_path();
    info!("Socket path: {}", socket_path.display());

    // Initialize event stores
    let event_store = Arc::new(init_event_stores(&config)?);

    // Initialize process manager
    let process_manager = ProcessManager::new(socket_path.clone());

    // Create IPC configuration with our socket path
    let ipc_config = create_ipc_config(&socket_path);

    // Launch the acton runtime
    let mut runtime = ActonApp::launch_async().await;

    // Register IPC message types
    let registry = runtime.ipc_registry();
    registry.register::<IpcEmergentMessage>("EmergentMessage");
    registry.register::<IpcSystemEvent>("SystemEvent");
    registry.register::<GetSubscriptions>("GetSubscriptions");
    registry.register::<SubscriptionsResponse>("SubscriptionsResponse");
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
    let event_store_for_emergent = event_store_clone.clone();
    let sub_mgr_for_emergent = sub_mgr_clone.clone();
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

        // Forward to IPC subscribers based on the inner message_type
        let notification = IpcPushNotification::new(
            msg.inner.message_type.clone(),
            Some(msg.inner.source.clone()),
            serde_json::to_value(&msg.inner).unwrap_or_default(),
        );
        sub_mgr_for_emergent.forward_to_subscribers(&notification);

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
            msg.inner.message_type.clone(),
            Some(msg.inner.source.clone()),
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

    // Create config service actor for SDK subscription lookups
    let mut config_service =
        runtime.new_actor_with_name::<ConfigServiceState>("config_service".to_string());

    // Handle GetSubscriptions requests from SDKs
    // Uses reply_envelope pattern for IPC request-response
    let pm_for_handler = process_manager.clone();
    config_service.mutate_on::<GetSubscriptions>(move |_actor, envelope| {
        let pm = pm_for_handler.clone();
        let msg = envelope.message();
        let name = msg.name.clone();

        let reply_envelope = envelope.reply_envelope();
        Reply::pending(async move {
            // Look up the primitive's configured subscriptions
            let subscribes = if let Some(info) = pm.get_info(&name).await {
                info.subscribes
            } else {
                Vec::new()
            };

            info!("GetSubscriptions for '{}': {:?}", name, subscribes);

            reply_envelope
                .send(SubscriptionsResponse { subscribes })
                .await;
        })
    });

    let config_handle = config_service.start().await;
    runtime.ipc_expose("config_service", config_handle);

    // Wait a moment for the listener to be ready
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    if acton_reactive::ipc::socket_exists(&socket_path) {
        info!("IPC socket ready: {}", socket_path.display());
    } else {
        error!("IPC socket not created: {}", socket_path.display());
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
            .start_all(&mut runtime, &enabled_sinks, &enabled_handlers, &enabled_sources)
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
        use tokio::signal::unix::{signal, SignalKind};
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
