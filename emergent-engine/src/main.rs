//! Emergent Engine - Event-based workflow platform.
//!
//! This is the main entry point for the Emergent engine, which:
//! - Loads configuration from TOML
//! - Initializes the event store (JSON logs + SQLite)
//! - Starts the IPC server for client connections
//! - Manages Source, Handler, and Sink processes
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
    let mut process_manager = ProcessManager::new(socket_path.clone());

    // Create channel for system events
    let (system_event_tx, mut system_event_rx) = tokio::sync::mpsc::channel::<emergent_engine::messages::EmergentMessage>(256);
    process_manager.set_event_sender(system_event_tx);

    // Register sources, handlers, and sinks from config
    for source in &config.sources {
        if source.enabled {
            info!("Registering source: {}", source.name);
            process_manager.register_source(source).await;
        }
    }

    for handler in &config.handlers {
        if handler.enabled {
            info!("Registering handler: {}", handler.name);
            process_manager.register_handler(handler).await;
        }
    }

    for sink in &config.sinks {
        if sink.enabled {
            info!("Registering sink: {}", sink.name);
            process_manager.register_sink(sink).await;
        }
    }

    // List registered primitives
    let primitives = process_manager.list_all().await;
    info!("Registered {} primitive(s)", primitives.len());
    for p in &primitives {
        info!("  {} ({}): {:?}", p.name, p.kind, p.state);
    }

    // Create IPC configuration with our socket path
    let ipc_config = create_ipc_config(&socket_path);

    // Launch the acton runtime
    let mut runtime = ActonApp::launch_async().await;

    // Register IPC message types
    let registry = runtime.ipc_registry();
    registry.register::<IpcEmergentMessage>("EmergentMessage");
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

    broker_actor.mutate_on::<IpcEmergentMessage>(move |actor, envelope| {
        let msg = envelope.message();
        actor.model.message_count += 1;

        // Log to event store
        if let Err(e) = event_store_clone.store(&msg.inner) {
            error!("Failed to store event: {}", e);
        }

        info!(
            "Message #{}: {} from {}",
            actor.model.message_count, msg.inner.message_type, msg.inner.source
        );

        // Forward to IPC subscribers based on the inner message_type
        // This routes "timer.tick" messages to clients subscribed to "timer.tick"
        let notification = IpcPushNotification::new(
            msg.inner.message_type.clone(),
            Some(msg.inner.source.clone()),
            serde_json::to_value(&msg.inner).unwrap_or_default(),
        );
        sub_mgr_clone.forward_to_subscribers(&notification);

        Reply::ready()
    });

    let broker_handle = broker_actor.start().await;
    runtime.ipc_expose("message_broker", broker_handle.clone());

    // Spawn task to forward system events to the broker
    let broker_for_events = broker_handle.clone();
    tokio::spawn(async move {
        while let Some(msg) = system_event_rx.recv().await {
            let ipc_msg = IpcEmergentMessage::from(msg);
            broker_for_events.send(ipc_msg).await;
        }
    });

    // Wait a moment for the listener to be ready
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    if acton_reactive::ipc::socket_exists(&socket_path) {
        info!("IPC socket ready: {}", socket_path.display());
    } else {
        error!("IPC socket not created: {}", socket_path.display());
    }

    // Start all registered processes
    if !primitives.is_empty() {
        info!("Starting registered processes...");
        if let Err(e) = process_manager.start_all().await {
            error!("Failed to start some processes: {}", e);
        }
    }

    info!("Engine ready - socket: {} wire_format: {:?}", socket_path.display(), config.engine.wire_format);

    // Spawn health check task
    let process_manager_clone = process_manager.clone();
    let health_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            process_manager_clone.health_check().await;
        }
    });

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

    // Cancel health check task
    health_task.abort();

    // Stop all processes (Sources → Handlers → Sinks)
    info!("Stopping processes...");
    process_manager.stop_all().await;

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

