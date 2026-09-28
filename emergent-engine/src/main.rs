//! Emergent Engine - Event-based workflow platform.
//!
//! This is the main entry point for the Emergent engine, which:
//! - Loads configuration from TOML
//! - Initializes the event store (JSON logs + SQLite)
//! - Starts the IPC server for client connections
//! - Manages Source, Handler, and Sink processes via actors
//! - Handles graceful shutdown

use acton_reactive::ipc::{IpcPushNotification, SubscriptionManager};
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

  # Start in a container, logging to stdout instead of emergent.log
  emergent --config /etc/emergent/emergent.toml --log-stdout

  # Check a config without starting anything
  emergent validate --config ./config/emergent.toml

  # Check it on a host where the primitives are not installed, as JSON
  emergent validate --config ./emergent.toml --skip-path-check --json

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

    /// Log to stdout instead of ~/.local/share/emergent/<name>/emergent.log,
    /// for containers and supervisors that collect stdout. Also set by
    /// EMERGENT_LOG_STDOUT=1
    #[arg(long)]
    log_stdout: bool,

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
    /// Check a configuration with the engine's startup checks, without starting it
    Validate(ValidateArgs),
}

/// Arguments for `emergent validate`.
///
/// The file comes from the global `--config`, then `./emergent.toml`, then the
/// XDG config directory, exactly as when starting the engine.
#[derive(clap::Args, Debug)]
#[command(after_long_help = "\
Runs the checks the engine runs before it spawns anything: the TOML schema
(unknown keys are errors), primitive names, duplicate names, restart policies,
subscription topics, [engine].api_allowed_hosts, that every enabled primitive's
path exists, and that the IPC connection limit covers every enabled primitive.
Every problem is reported, not only the first. Exits 0 when the engine would
start and 1 when it would not.

--json prints {\"ok\", \"engine_version\", \"errors\", \"warnings\"}, where each
issue has a \"code\", a \"message\" and, when it has one, a \"path\" such as
\"sinks[2].publishes\".")]
struct ValidateArgs {
    /// Print the result as JSON on stdout
    #[arg(long)]
    json: bool,

    /// Do not check that each enabled primitive's path exists, for a host
    /// where the primitives are installed elsewhere
    #[arg(long)]
    skip_path_check: bool,
}

use emergent_engine::api_host::{describe_allowed_hosts, guard_host};
use emergent_engine::config::{ConfigError, ConfigIssue, EmergentConfig, IssueCode, PathCheck};
use emergent_engine::declarations::{RejectionReport, rejection_event_type};
use emergent_engine::event_store::{EventStore, EventStoreError, JsonEventLog, SqliteEventStore};
use emergent_engine::ipc_identity::{
    AncestryResolver, ConnectionRegistry, SpawnedChildren, ancestry_available,
};
use emergent_engine::ipc_policy::{
    EnginePolicy, Observers, PolicyObserver, UnmanagedNames, policy_is_needed,
};
use emergent_engine::logging::{LOG_STDOUT_ENV, LogDestination, log_destination};
use emergent_engine::messages::EmergentMessage;
use emergent_engine::preflight::{self, Findings, ValidationReport};
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

/// Find the configuration file: the given path, else `./emergent.toml`, else
/// the XDG config directory.
fn resolve_config_path(path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = path {
        if !p.exists() {
            anyhow::bail!(
                "Configuration file not found: {}\n\n\
                 To get started, create a config file:\n  \
                 emergent init\n  \
                 emergent --config path/to/emergent.toml",
                p.display()
            );
        }
        return Ok(p);
    }

    // Check for config in current directory first
    let local = PathBuf::from("emergent.toml");
    if local.exists() {
        return Ok(local);
    }
    if let Some(dirs) = directories::ProjectDirs::from("ai", "govcraft", "emergent") {
        let xdg_config = dirs.config_dir().join("emergent.toml");
        if xdg_config.exists() {
            return Ok(xdg_config);
        }
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
    anyhow::bail!(
        "No configuration file found.\n\n\
         Searched:\n  \
         ./emergent.toml\n\n\
         To get started, create a config file:\n  \
         emergent init\n  \
         emergent --config path/to/emergent.toml"
    );
}

/// Refuse to start on the first pre-flight error, returning the warnings
/// otherwise.
///
/// The checks are the ones `emergent validate` runs, through the same
/// [`preflight::preflight`] call; startup names the first problem and points
/// at `validate` when there are more.
fn refuse_on_errors(findings: Findings, config_path: &Path) -> Result<Vec<ConfigIssue>> {
    let more = findings.errors.len().saturating_sub(1);
    findings.into_result().map_err(|e| {
        let context = if more > 0 {
            format!(
                "Failed to load {path} ({more} more problem(s): run `emergent validate --config {path}` to list them all)",
                path = config_path.display()
            )
        } else {
            format!("Failed to load {}", config_path.display())
        };
        anyhow::Error::new(e).context(context)
    })
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

/// Run `emergent validate` and return the process exit code.
///
/// Reads the file with [`EmergentConfig::read`] and checks it with
/// [`preflight::preflight`], which is what startup does, so a config that
/// passes here is one the engine will start (the socket and the event store
/// aside, which are only known by trying).
fn run_validate(config: Option<PathBuf>, args: &ValidateArgs) -> Result<i32> {
    let paths = if args.skip_path_check {
        PathCheck::Skip
    } else {
        PathCheck::Check
    };

    let (display_path, report) = match resolve_config_path(config) {
        Err(e) => (
            PathBuf::from("emergent.toml"),
            ValidationReport::failed(ConfigIssue::new(
                IssueCode::ConfigNotFound,
                format!("{e:#}"),
            )),
        ),
        Ok(path) => {
            let report = match std::fs::read_to_string(&path) {
                Err(e) => ValidationReport::failed(preflight::load_issue(
                    &ConfigError::ReadError(e),
                    None,
                )),
                Ok(text) => match EmergentConfig::parse_unchecked(&text) {
                    Err(e) => ValidationReport::failed(preflight::load_issue(&e, Some(&text))),
                    Ok(config) => {
                        let max_connections = preflight::effective_max_connections(&config);
                        ValidationReport::from_findings(preflight::preflight(
                            &config,
                            max_connections,
                            paths,
                        ))
                    }
                },
            };
            (path, report)
        }
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.to_human(&display_path));
    }
    Ok(report.exit_code())
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
        // Subcommands log at info level. validate keeps stdout for its result,
        // so its log lines go to stderr.
        let subscriber =
            tracing_subscriber::fmt().with_env_filter(EnvFilter::new("info,acton_reactive=off"));
        if matches!(command, Command::Validate(_)) {
            subscriber.with_writer(std::io::stderr).init();
        } else {
            subscriber.init();
        }

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
            Command::Validate(validate_args) => {
                let code = run_validate(args.config, &validate_args)?;
                std::process::exit(code);
            }
        }
    }

    // Load configuration (before tracing init so we know the engine name)
    let config_path = resolve_config_path(args.config).context("Failed to load configuration")?;
    let mut config = EmergentConfig::read(&config_path)
        .with_context(|| format!("Failed to load {}", config_path.display()))
        .context("Failed to load configuration")?;

    // Override socket path if specified
    if let Some(socket) = args.socket {
        config.engine.socket_path = socket.display().to_string();
    }
    let socket_path = config.socket_path();

    // Resolve the IPC configuration now: the connection limit it settles on is
    // one of the pre-flight checks.
    let ipc_config = preflight::ipc_config(Some(&socket_path), config.engine.max_connections);
    let max_connections = ipc_config.limits.max_connections;

    // Refuse a config the engine cannot run before anything is created: the
    // same checks `emergent validate` runs, through the same call. The
    // capacity check sees the limit acton actually resolved, not just the one
    // written in emergent.toml; without it the primitives that lose the race
    // to the accept semaphore are dropped silently, and the engine reports
    // them as running.
    let warnings = refuse_on_errors(
        preflight::preflight(&config, max_connections, PathCheck::Check),
        &config_path,
    )
    .context("Failed to load configuration")?;

    // Initialize tracing
    // --verbose: log to the terminal
    // --log-stdout or EMERGENT_LOG_STDOUT=1: log to stdout, for containers
    // default: log to file at ~/.local/share/emergent/<engine-name>/emergent.log
    let log_level = "info,acton_reactive=off";
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level));
    let log_stdout_env = std::env::var(LOG_STDOUT_ENV).ok();

    match log_destination(args.verbose, args.log_stdout, log_stdout_env.as_deref()) {
        LogDestination::Terminal => {
            tracing_subscriber::fmt().with_env_filter(env_filter).init();
        }
        LogDestination::Stdout => {
            use std::io::IsTerminal;
            tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_writer(std::io::stdout)
                .with_ansi(std::io::stdout().is_terminal())
                .init();
        }
        LogDestination::File => {
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
    }

    info!("Loaded configuration from {}", config_path.display());
    info!("Engine name: {}", config.engine.name);

    // Pre-flight warnings, such as the inert wire_format key
    for warning in &warnings {
        warn!("{}", warning.message);
    }

    info!("Socket path: {}", socket_path.display());
    info!(
        "IPC connection limit: {} ({} enabled primitive(s) plus {} reserved)",
        max_connections,
        config.enabled_primitive_count(),
        emergent_engine::config::RESERVED_IPC_CONNECTIONS
    );

    // Check for stale socket from a previous unclean shutdown
    check_and_cleanup_stale_socket(&socket_path)
        .await
        .context("Socket pre-flight check failed")?;

    // Initialize event stores
    let event_store = Arc::new(init_event_stores(&config)?);

    // Enforce [event_store].retention_days: prune now, then once a day
    start_retention(&event_store, &config);

    // Initialize process manager.
    //
    // The connection registry has to exist first: the policy that feeds it is
    // an argument to starting the listener, and the process manager is what
    // tells it a child has died. It gets the listener to revoke through once
    // that listener exists.
    let connections = Arc::new(ConnectionRegistry::new());
    let process_manager = ProcessManager::new(socket_path.clone(), config.engine.api_port)
        .observing_child_exits(connections.clone());

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
    }

    // Admission names a connection by walking the peer's process ancestry to a
    // child the engine spawned. Where that cannot be read, a primitive behind
    // a launcher arrives unnamed through no fault of its own, so enforcement
    // falls back to the source the client writes and the operator is told the
    // mode is worth less there than it says.
    let unmanaged_names = if ancestry_available() {
        UnmanagedNames::Ignored
    } else {
        if declarations.mode().is_enforcing() {
            warn!(
                "This platform does not report process ancestry, so a primitive launched \
                 through a wrapper such as `uv` cannot be named: its publishes are checked \
                 against the source it writes itself and it may not subscribe to anything \
                 beyond the engine protocol"
            );
        }
        UnmanagedNames::Claimed
    };
    if config.engine.authenticate_connections.is_enforcing() {
        info!(
            "Connection authentication: {}",
            config.engine.authenticate_connections
        );
    }

    // Rejections are reported from a task of their own. `authorize` runs on
    // acton's connection task and must not block, and the subscription manager
    // it would need does not exist until the listener below has started, so the
    // policy only hands the report over and returns.
    let (rejections_tx, mut rejections_rx) = tokio::sync::mpsc::unbounded_channel();

    // Start the IPC listener first to get the subscription manager.
    //
    // Two observers watch what the policy decides: startup readiness needs the
    // subscribes it authorizes, which is the only way the engine learns a tier
    // is listening, and the connection registry needs the admissions, so that
    // a primitive's connections can be closed when its child exits.
    // Registering an observer is itself enough to install the policy, so this
    // works with enforcement off.
    let (startup_observer, startup_signals) = StartupObserver::channel();
    let observers: Arc<dyn PolicyObserver> = Arc::new(Observers::new(vec![
        Arc::new(startup_observer),
        connections.clone(),
    ]));
    let children: Arc<dyn SpawnedChildren> = Arc::new(process_manager.clone());

    let listener_handle = if policy_is_needed(declarations.mode(), true) {
        let resolver = Arc::new(AncestryResolver::new(
            children,
            config.engine.authenticate_connections,
        ));
        let policy = Arc::new(
            EnginePolicy::new(declarations.clone(), resolver, Some(observers))
                .naming_unmanaged(unmanaged_names)
                .reporting_to(rejections_tx),
        );
        runtime
            .start_ipc_listener_with_policy(ipc_config, policy)
            .await
    } else {
        runtime.start_ipc_listener_with_config(ipc_config).await
    }
    .context("Failed to start IPC listener")?;
    let listener_handle = Arc::new(listener_handle);
    connections.install_revoker(listener_handle.clone());

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

    #[test]
    fn ipc_idle_override_keeps_default_admission_deadline() {
        let config = preflight::apply_engine_ipc_overrides(
            IpcConfig::default(),
            Some(Path::new("engine.sock")),
            None,
        );
        assert_eq!(config.read_timeout(), None);
        assert_eq!(
            config.admission_timeout(),
            Some(std::time::Duration::from_secs(60))
        );
    }

    #[test]
    fn ipc_overrides_preserve_resolved_settings_and_optional_capacity() {
        let mut resolved = IpcConfig::default();
        resolved.timeouts.admission = 175;
        resolved.timeouts.read = 25;
        resolved.timeouts.subscription_read = 350;
        resolved.timeouts.write = 450;
        resolved.limits.max_connections = 77;
        let socket = Path::new("engine.sock");
        let config = preflight::apply_engine_ipc_overrides(resolved.clone(), Some(socket), None);
        assert_eq!(config.socket.path.as_deref(), Some(socket));
        assert_eq!(config.read_timeout(), None);
        assert_eq!(config.timeouts.admission, 175);
        assert_eq!(config.timeouts.subscription_read, 350);
        assert_eq!(config.timeouts.write, 450);
        assert_eq!(config.limits.max_connections, 77);
        let overridden = preflight::apply_engine_ipc_overrides(resolved, Some(socket), Some(123));
        assert_eq!(overridden.limits.max_connections, 123);
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct IdlePublishProbe {
        sequence: usize,
    }

    #[tokio::test]
    async fn ipc_publish_only_source_delivers_after_resolved_idle_deadline() -> Result<()> {
        use acton_reactive::ipc::{IpcClient, IpcEnvelope};
        use std::time::Duration;

        let dir = TempDir::new()?;
        let socket = dir.path().join("idle.sock");
        let mut resolved = IpcConfig::default();
        resolved.timeouts.read = 25;
        let config = preflight::apply_engine_ipc_overrides(resolved, Some(&socket), None);
        let mut runtime = ActonApp::launch_async().await;
        runtime
            .ipc_registry()
            .register::<IdlePublishProbe>("IdlePublishProbe");
        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut actor = runtime.new_actor::<()>();
        actor.act_on::<IdlePublishProbe>(move |_, context| {
            let _ = seen_tx.send(context.message().sequence);
            Reply::ready()
        });
        runtime.ipc_expose("probe", actor.start().await)?;
        let listener = runtime.start_ipc_listener_with_config(config).await?;
        let client = IpcClient::connect(&socket).await?;
        for sequence in 0..2 {
            client
                .send(IpcEnvelope::new(
                    "probe",
                    "IdlePublishProbe",
                    serde_json::json!({ "sequence": sequence }),
                ))
                .await?;
            let observed = tokio::time::timeout(Duration::from_secs(2), seen_rx.recv()).await?;
            assert_eq!(observed, Some(sequence));
            if sequence == 0 {
                // Deliberately remain idle longer than the loaded read deadline.
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        listener.stop();
        runtime.shutdown_all().await?;
        Ok(())
    }

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
