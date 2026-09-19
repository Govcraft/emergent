//! Configuration loading and management for the Emergent Engine.
//!
//! The configuration is stored in TOML format and defines:
//! - Engine settings (name, socket path, wire format)
//! - Event store settings (log directory, SQLite path, retention)
//! - Sources, Handlers, and Sinks to manage

use crate::supervision::{
    RestartLimits, RestartPolicy, default_restart_backoff_ms, default_restart_max_backoff_ms,
    default_restart_max_retries, default_restart_window_ms, parse_restart_policy,
};
use emergent_client::types::PrimitiveName;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Configuration errors.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("Failed to parse config: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("Invalid config: {0}")]
    ValidationError(String),

    #[error("Path does not exist: {0}")]
    PathNotFound(PathBuf),
}

/// Wire format named by the inert `[engine].wire_format` key.
///
/// The key is still accepted so configurations written for 0.10.10 and earlier
/// keep loading, but it selects nothing: IPC is always MessagePack.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WireFormat {
    /// JSON format (human-readable, useful for debugging)
    Json,
    /// MessagePack format (binary, more efficient)
    #[default]
    Messagepack,
}

impl WireFormat {
    /// Returns the TOML spelling of this wire format.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Messagepack => "messagepack",
        }
    }
}

impl std::fmt::Display for WireFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Build the startup warning for an inert `wire_format` key (pure function).
///
/// Returns `None` when the key is absent, which is the only configuration that
/// describes the engine accurately. Returns the warning text otherwise.
#[must_use]
pub fn wire_format_warning(wire_format: Option<WireFormat>) -> Option<String> {
    wire_format.map(|format| {
        format!(
            "[engine] wire_format = \"{format}\" has no effect: IPC is always MessagePack. Remove the key. For human-readable events, read the JSON event log."
        )
    })
}

/// Engine configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    /// Name of this engine instance.
    #[serde(default = "default_engine_name")]
    pub name: String,

    /// Socket path for IPC. Use "auto" for XDG-compliant default.
    #[serde(default = "default_socket_path")]
    pub socket_path: String,

    /// Wire format for IPC communication. Accepted but inert: IPC is always
    /// MessagePack, and setting the key only earns a warning at startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_format: Option<WireFormat>,

    /// HTTP API port for topology queries. Set to 0 to disable.
    #[serde(default = "default_api_port")]
    pub api_port: u16,

    /// How long each shutdown phase waits for children to exit on the
    /// `system.shutdown` broadcast alone, before SIGTERM, in milliseconds.
    #[serde(default = "default_shutdown_drain_ms")]
    pub shutdown_drain_ms: u64,

    /// How long each shutdown phase waits after SIGTERM before escalating to
    /// SIGKILL, in milliseconds.
    #[serde(default = "default_shutdown_grace_ms")]
    pub shutdown_grace_ms: u64,
}

fn default_engine_name() -> String {
    "emergent".to_string()
}

fn default_socket_path() -> String {
    "auto".to_string()
}

const fn default_api_port() -> u16 {
    8891
}

/// Default drain window: matches the 500 ms the engine historically slept
/// between broadcasting `system.shutdown` and sending SIGTERM.
const fn default_shutdown_drain_ms() -> u64 {
    500
}

/// Default grace window: matches the 2 s the engine historically slept after
/// SIGTERM, so the default shutdown is never less patient than before.
const fn default_shutdown_grace_ms() -> u64 {
    2_000
}

impl EngineConfig {
    /// Returns whether the HTTP API server is enabled.
    #[must_use]
    pub fn api_enabled(&self) -> bool {
        self.api_port != 0
    }

    /// The drain window as a [`std::time::Duration`].
    #[must_use]
    pub const fn shutdown_drain(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.shutdown_drain_ms)
    }

    /// The post-SIGTERM grace window as a [`std::time::Duration`].
    #[must_use]
    pub const fn shutdown_grace(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.shutdown_grace_ms)
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            name: default_engine_name(),
            socket_path: default_socket_path(),
            wire_format: None,
            api_port: default_api_port(),
            shutdown_drain_ms: default_shutdown_drain_ms(),
            shutdown_grace_ms: default_shutdown_grace_ms(),
        }
    }
}

/// Event store configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventStoreConfig {
    /// Directory for JSON log files.
    #[serde(default = "default_json_log_dir")]
    pub json_log_dir: PathBuf,

    /// Path to SQLite database.
    #[serde(default = "default_sqlite_path")]
    pub sqlite_path: PathBuf,

    /// Retention period in days for old events.
    ///
    /// The engine prunes both stores at startup and once a day afterwards.
    /// `0` disables pruning and keeps every event forever.
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

/// Sentinel value indicating the event store path should be resolved to an XDG data directory.
const EVENT_STORE_SENTINEL_LOG_DIR: &str = "auto";
const EVENT_STORE_SENTINEL_SQLITE: &str = "auto";

fn default_json_log_dir() -> PathBuf {
    PathBuf::from(EVENT_STORE_SENTINEL_LOG_DIR)
}

fn default_sqlite_path() -> PathBuf {
    PathBuf::from(EVENT_STORE_SENTINEL_SQLITE)
}

const fn default_retention_days() -> u32 {
    30
}

impl Default for EventStoreConfig {
    fn default() -> Self {
        Self {
            json_log_dir: default_json_log_dir(),
            sqlite_path: default_sqlite_path(),
            retention_days: default_retention_days(),
        }
    }
}

/// Per-primitive supervision settings.
///
/// These keys are flattened into each `[[sources]]`, `[[handlers]]` and
/// `[[sinks]]` table, so a primitive's supervision story reads in one place
/// next to the rest of its definition.
///
/// `restart` is kept as a `String` here rather than a `RestartPolicy` so that
/// an unrecognised value produces a validation error naming the primitive and
/// the offending value, instead of serde's anonymous "unknown variant".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestartConfig {
    /// Restart policy: `"never"` (default), `"on-failure"` or `"always"`.
    #[serde(default = "default_restart")]
    pub restart: String,

    /// Delay before the first restart attempt, in milliseconds.
    #[serde(default = "default_restart_backoff_ms")]
    pub restart_backoff_ms: u64,

    /// Ceiling for the doubling backoff, in milliseconds.
    #[serde(default = "default_restart_max_backoff_ms")]
    pub restart_max_backoff_ms: u64,

    /// Maximum restarts allowed inside `restart_window_ms`.
    #[serde(default = "default_restart_max_retries")]
    pub restart_max_retries: u32,

    /// Sliding window over which restarts are counted, in milliseconds.
    #[serde(default = "default_restart_window_ms")]
    pub restart_window_ms: u64,
}

fn default_restart() -> String {
    RestartPolicy::default().as_str().to_string()
}

impl Default for RestartConfig {
    fn default() -> Self {
        Self {
            restart: default_restart(),
            restart_backoff_ms: default_restart_backoff_ms(),
            restart_max_backoff_ms: default_restart_max_backoff_ms(),
            restart_max_retries: default_restart_max_retries(),
            restart_window_ms: default_restart_window_ms(),
        }
    }
}

impl RestartConfig {
    /// Parse the configured policy (pure function).
    ///
    /// Returns `None` when the value is not one of the documented spellings.
    #[must_use]
    pub fn policy(&self) -> Option<RestartPolicy> {
        parse_restart_policy(&self.restart)
    }

    /// The pacing limits described by this configuration (pure function).
    #[must_use]
    pub const fn limits(&self) -> RestartLimits {
        RestartLimits {
            backoff_ms: self.restart_backoff_ms,
            max_backoff_ms: self.restart_max_backoff_ms,
            max_retries: self.restart_max_retries,
            window_ms: self.restart_window_ms,
        }
    }
}

/// Configuration for a Source primitive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    /// Unique name for this source.
    pub name: String,

    /// Path to the executable.
    pub path: PathBuf,

    /// Command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,

    /// Whether this source is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Message types this source publishes.
    #[serde(default)]
    pub publishes: Vec<String>,

    /// Environment variables to set.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,

    /// Supervision settings (`restart`, `restart_backoff_ms`, ...).
    #[serde(flatten, default)]
    pub restart: RestartConfig,
}

/// Configuration for a Handler primitive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandlerConfig {
    /// Unique name for this handler.
    pub name: String,

    /// Path to the executable.
    pub path: PathBuf,

    /// Command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,

    /// Whether this handler is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Message types this handler subscribes to.
    #[serde(default)]
    pub subscribes: Vec<String>,

    /// Message types this handler publishes.
    #[serde(default)]
    pub publishes: Vec<String>,

    /// Environment variables to set.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,

    /// Automatically unwrap exec-source stdout payloads before delivery.
    ///
    /// When `true`, the SDK extracts and parses the `.stdout` field from
    /// exec-source's `{command, stdout, exit_code}` envelope, so handlers
    /// receive the raw data directly without needing a dedicated unwrap step.
    #[serde(default)]
    pub unwrap_stdout: bool,

    /// Supervision settings (`restart`, `restart_backoff_ms`, ...).
    #[serde(flatten, default)]
    pub restart: RestartConfig,
}

/// Configuration for a Sink primitive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SinkConfig {
    /// Unique name for this sink.
    pub name: String,

    /// Path to the executable.
    pub path: PathBuf,

    /// Command-line arguments.
    #[serde(default)]
    pub args: Vec<String>,

    /// Whether this sink is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Message types this sink subscribes to.
    #[serde(default)]
    pub subscribes: Vec<String>,

    /// Environment variables to set.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,

    /// Automatically unwrap exec-source stdout payloads before delivery.
    ///
    /// When `true`, the SDK extracts and parses the `.stdout` field from
    /// exec-source's `{command, stdout, exit_code}` envelope, so sinks
    /// receive the raw data directly without needing a dedicated unwrap step.
    #[serde(default)]
    pub unwrap_stdout: bool,

    /// Supervision settings (`restart`, `restart_backoff_ms`, ...).
    #[serde(flatten, default)]
    pub restart: RestartConfig,
}

const fn default_enabled() -> bool {
    true
}

/// Expand tilde prefix in a path (pure function).
///
/// If the path starts with `~`, replaces it with the provided home directory.
/// Otherwise returns the path unchanged.
///
/// # Arguments
///
/// * `path` - The path to expand
/// * `home_dir` - The home directory to substitute for `~`
///
/// # Returns
///
/// A new `PathBuf` with tilde expanded, or the original path if no expansion needed.
fn expand_tilde(path: &Path, home_dir: Option<&Path>) -> PathBuf {
    let path_str = path.to_string_lossy();

    // Only expand if path starts with ~ and we have a home directory
    if !path_str.starts_with('~') {
        return path.to_path_buf();
    }

    let Some(home) = home_dir else {
        return path.to_path_buf();
    };

    // Handle exactly "~" or "~/..."
    if path_str == "~" {
        return home.to_path_buf();
    }

    if let Some(rest) = path_str.strip_prefix("~/") {
        return home.join(rest);
    }

    // Path starts with ~ but not ~/ (e.g., "~user/path") - don't expand
    path.to_path_buf()
}

/// Resolve a bare command name via PATH lookup (impure function).
///
/// - If the path contains a `/`, it's absolute or relative — return unchanged.
/// - If the path already exists as a file, return unchanged.
/// - Otherwise, attempt `which::which()` to find it on PATH.
///   Returns the resolved absolute path on success, or the original path on failure.
fn resolve_command_path(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();

    // Contains a separator — it's absolute or relative, leave it alone
    if path_str.contains('/') {
        return path.to_path_buf();
    }

    // Already exists as a file at this path
    if path.exists() {
        return path.to_path_buf();
    }

    // Bare command name — try PATH lookup
    match which::which(path) {
        Ok(resolved) => {
            tracing::debug!(
                command = %path_str,
                resolved = %resolved.display(),
                "Resolved bare command via PATH"
            );
            resolved
        }
        Err(_) => path.to_path_buf(),
    }
}

/// Common interface for all primitive configurations.
///
/// This trait abstracts over `SourceConfig`, `HandlerConfig`, and `SinkConfig`
/// to enable generic validation and processing.
pub trait PrimitiveConfig {
    /// Returns the unique name of this primitive.
    fn name(&self) -> &str;

    /// Returns the path to the executable.
    fn path(&self) -> &Path;

    /// Returns whether this primitive is enabled.
    fn is_enabled(&self) -> bool;

    /// Returns this primitive's supervision settings.
    fn restart_config(&self) -> &RestartConfig;
}

impl PrimitiveConfig for SourceConfig {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn restart_config(&self) -> &RestartConfig {
        &self.restart
    }
}

impl PrimitiveConfig for HandlerConfig {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn restart_config(&self) -> &RestartConfig {
        &self.restart
    }
}

impl PrimitiveConfig for SinkConfig {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn restart_config(&self) -> &RestartConfig {
        &self.restart
    }
}

/// Check for duplicate names across a collection of primitives (pure function).
fn check_duplicate_names<'a, T: PrimitiveConfig + 'a>(
    primitives: impl IntoIterator<Item = &'a T>,
    names: &mut std::collections::HashSet<&'a str>,
) -> Result<(), ConfigError> {
    for primitive in primitives {
        if !names.insert(primitive.name()) {
            return Err(ConfigError::ValidationError(format!(
                "Duplicate name: {}",
                primitive.name()
            )));
        }
    }
    Ok(())
}

/// Check that every primitive names a known restart policy (pure function).
///
/// The error names both the primitive and the offending value so a typo in a
/// large config is findable without bisecting the file.
fn check_restart_policies<'a, T: PrimitiveConfig + 'a>(
    primitives: impl IntoIterator<Item = &'a T>,
) -> Result<(), ConfigError> {
    for primitive in primitives {
        let cfg = primitive.restart_config();
        if cfg.policy().is_none() {
            return Err(ConfigError::ValidationError(format!(
                "Invalid restart policy for primitive '{}': '{}' (expected one of {})",
                primitive.name(),
                cfg.restart,
                RestartPolicy::variants().join(", ")
            )));
        }
    }
    Ok(())
}

/// Validate that a primitive name can form a message type segment (pure function).
///
/// The engine publishes `system.started.<name>`, `system.stopped.<name>` and
/// `system.error.<name>` for every primitive it manages. The last segment is the
/// configured name, so a name the SDK would reject as a `PrimitiveName` produces
/// a message type the SDK also rejects. The rule lives in the SDK, this reuses it
/// rather than restating it, and reports it before anything is spawned.
///
/// # Errors
///
/// Returns [`ConfigError::ValidationError`] naming the primitive and the rule.
fn validate_primitive_name(name: &str) -> Result<(), ConfigError> {
    PrimitiveName::new(name).map(|_| ()).map_err(|e| {
        ConfigError::ValidationError(format!(
            "{e}. A primitive name becomes the last segment of its \
             system.started.<name> event, so it must start with a lowercase letter and \
             use only lowercase letters, digits, hyphens, or underscores, at most {max} \
             characters",
            max = PrimitiveName::MAX_LENGTH
        ))
    })
}

/// Check that every primitive in a collection has a usable name (pure function).
fn check_valid_names<'a, T: PrimitiveConfig + 'a>(
    primitives: impl IntoIterator<Item = &'a T>,
) -> Result<(), ConfigError> {
    for primitive in primitives {
        validate_primitive_name(primitive.name())?;
    }
    Ok(())
}

/// Check that every subscription topic can match something (pure function).
///
/// Delegates the rule to the SDK so the engine and every primitive agree on
/// what a wildcard means: a single terminal `*`, or no `*` at all.
fn check_subscription_topics(
    name: &str,
    kind: &str,
    subscribes: &[String],
) -> Result<(), ConfigError> {
    for topic in subscribes {
        emergent_client::classify_topic(topic).map_err(|e| {
            ConfigError::ValidationError(format!(
                "{kind} '{name}' subscribes to an invalid topic: {e}"
            ))
        })?;
    }
    Ok(())
}

/// Check that paths exist for enabled primitives (impure function).
fn check_paths_exist<'a, T: PrimitiveConfig + 'a>(
    primitives: impl IntoIterator<Item = &'a T>,
) -> Result<(), ConfigError> {
    for primitive in primitives {
        if primitive.is_enabled() && !primitive.path().exists() {
            return Err(ConfigError::PathNotFound(primitive.path().to_path_buf()));
        }
    }
    Ok(())
}

/// Complete Emergent configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmergentConfig {
    /// Engine settings.
    #[serde(default)]
    pub engine: EngineConfig,

    /// Event store settings.
    #[serde(default)]
    pub event_store: EventStoreConfig,

    /// Source definitions.
    #[serde(default, rename = "sources")]
    pub sources: Vec<SourceConfig>,

    /// Handler definitions.
    #[serde(default, rename = "handlers")]
    pub handlers: Vec<HandlerConfig>,

    /// Sink definitions.
    #[serde(default, rename = "sinks")]
    pub sinks: Vec<SinkConfig>,
}

impl EmergentConfig {
    /// Load configuration from a file.
    ///
    /// Paths in primitive configurations are expanded to resolve `~` to the
    /// user's home directory before validation.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let mut config: EmergentConfig = toml::from_str(&content)?;
        config.expand_paths();
        config.resolve_path_commands();
        config.resolve_event_store();
        config.validate()?;
        Ok(config)
    }

    /// Load configuration from a TOML string.
    ///
    /// Paths in primitive configurations are expanded to resolve `~` to the
    /// user's home directory before validation.
    pub fn parse(content: &str) -> Result<Self, ConfigError> {
        let mut config: EmergentConfig = toml::from_str(content)?;
        config.expand_paths();
        config.resolve_path_commands();
        config.resolve_event_store();
        config.validate()?;
        Ok(config)
    }

    /// Expand tilde (`~`) in all primitive paths using the provided home directory (pure function).
    ///
    /// This creates a new configuration with all source, handler, and sink paths
    /// expanded. Useful for testing with a controlled home directory.
    #[must_use]
    pub fn with_expanded_paths(&self, home_dir: Option<&Path>) -> Self {
        let mut config = self.clone();
        for source in &mut config.sources {
            source.path = expand_tilde(&source.path, home_dir);
        }
        for handler in &mut config.handlers {
            handler.path = expand_tilde(&handler.path, home_dir);
        }
        for sink in &mut config.sinks {
            sink.path = expand_tilde(&sink.path, home_dir);
        }
        config
    }

    /// Expand tilde (`~`) in all primitive paths using the system home directory.
    ///
    /// Modifies the configuration in place, replacing `~` prefixes in source,
    /// handler, and sink paths with the user's actual home directory.
    pub fn expand_paths(&mut self) {
        let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
        let home_ref = home.as_deref();
        for source in &mut self.sources {
            source.path = expand_tilde(&source.path, home_ref);
        }
        for handler in &mut self.handlers {
            handler.path = expand_tilde(&handler.path, home_ref);
        }
        for sink in &mut self.sinks {
            sink.path = expand_tilde(&sink.path, home_ref);
        }
    }

    /// Resolve bare command names in all primitive paths via PATH lookup.
    ///
    /// For each source, handler, and sink, if the path is a bare command name
    /// (no `/` separator), attempts to resolve it to an absolute path using
    /// the system PATH. This allows configs to use `path = "uv"` instead of
    /// `path = "/usr/bin/uv"`.
    pub fn resolve_path_commands(&mut self) {
        for source in &mut self.sources {
            source.path = resolve_command_path(&source.path);
        }
        for handler in &mut self.handlers {
            handler.path = resolve_command_path(&handler.path);
        }
        for sink in &mut self.sinks {
            sink.path = resolve_command_path(&sink.path);
        }
    }

    /// Validate the configuration structure (pure function).
    ///
    /// Checks for duplicate names across all primitives without performing I/O.
    /// This is suitable for testing configuration validity without filesystem access.
    pub fn validate_unique_names(&self) -> Result<(), ConfigError> {
        let mut names = std::collections::HashSet::new();
        check_duplicate_names(&self.sources, &mut names)?;
        check_duplicate_names(&self.handlers, &mut names)?;
        check_duplicate_names(&self.sinks, &mut names)?;
        Ok(())
    }

    /// Validate every primitive's `restart` value (pure function).
    pub fn validate_restart_policies(&self) -> Result<(), ConfigError> {
        check_restart_policies(&self.sources)?;
        check_restart_policies(&self.handlers)?;
        check_restart_policies(&self.sinks)?;
        Ok(())
    }

    /// Validate that every primitive name can form a message type (pure function).
    ///
    /// Disabled primitives are checked too: the name is a structural property of
    /// the config, so enabling a primitive later must never be what breaks it.
    pub fn validate_names(&self) -> Result<(), ConfigError> {
        check_valid_names(&self.sources)?;
        check_valid_names(&self.handlers)?;
        check_valid_names(&self.sinks)?;
        Ok(())
    }

    /// Validate that all enabled primitive paths exist (impure function).
    ///
    /// Performs filesystem checks to verify executable paths exist.
    pub fn validate_paths(&self) -> Result<(), ConfigError> {
        check_paths_exist(&self.sources)?;
        check_paths_exist(&self.handlers)?;
        check_paths_exist(&self.sinks)?;
        Ok(())
    }

    /// Validate every configured subscription topic (pure function).
    ///
    /// A topic is either a literal message type or a prefix selector ending in
    /// a single `*`. A `*` anywhere else, such as `system.*.error`, can never
    /// match a published message type, so the engine refuses the topology
    /// rather than starting a primitive that would sit idle forever.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::ValidationError`] naming the primitive and the
    /// topic that cannot match.
    pub fn validate_subscription_topics(&self) -> Result<(), ConfigError> {
        for handler in &self.handlers {
            check_subscription_topics(&handler.name, "handler", &handler.subscribes)?;
        }
        for sink in &self.sinks {
            check_subscription_topics(&sink.name, "sink", &sink.subscribes)?;
        }
        Ok(())
    }

    /// Validate the configuration (combines structure and path validation).
    ///
    /// This is a convenience method that runs both pure validation (name rules,
    /// duplicate names, restart policies and subscription topics) and impure
    /// validation (path existence checks).
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_names()?;
        self.validate_unique_names()?;
        self.validate_restart_policies()?;
        self.validate_subscription_topics()?;
        self.validate_paths()?;
        Ok(())
    }

    /// Resolve event store paths to XDG data directory if set to "auto" (pure function).
    ///
    /// Replaces sentinel defaults with `{data_dir}/{engine_name}/logs` and
    /// `{data_dir}/{engine_name}/events.db`. Explicit paths are left unchanged.
    #[must_use]
    pub fn with_resolved_event_store(&self, data_dir: Option<&Path>) -> Self {
        let mut config = self.clone();
        config.resolve_event_store_paths(data_dir);
        config
    }

    /// Resolve event store paths in place.
    ///
    /// If `json_log_dir` or `sqlite_path` is the sentinel value "auto",
    /// replaces it with an XDG-compliant path under the data directory
    /// namespaced by engine name.
    pub fn resolve_event_store_paths(&mut self, data_dir: Option<&Path>) {
        if self.event_store.json_log_dir == Path::new(EVENT_STORE_SENTINEL_LOG_DIR) {
            if let Some(dir) = data_dir {
                self.event_store.json_log_dir = dir.join(&self.engine.name).join("logs");
            } else {
                self.event_store.json_log_dir = PathBuf::from("./logs");
            }
        }
        if self.event_store.sqlite_path == Path::new(EVENT_STORE_SENTINEL_SQLITE) {
            if let Some(dir) = data_dir {
                self.event_store.sqlite_path = dir.join(&self.engine.name).join("events.db");
            } else {
                self.event_store.sqlite_path = PathBuf::from("./events.db");
            }
        }
    }

    /// Resolve event store paths using system XDG directories.
    ///
    /// Uses `~/.local/share/emergent/{engine_name}/` as the base directory.
    pub fn resolve_event_store(&mut self) {
        let data_dir = directories::ProjectDirs::from("ai", "govcraft", "emergent")
            .map(|dirs| dirs.data_dir().to_path_buf());
        self.resolve_event_store_paths(data_dir.as_deref());
    }

    /// Resolve the socket path given an optional runtime directory (pure function).
    ///
    /// If `socket_path` is "auto", uses the provided runtime directory or falls back to `/tmp`.
    /// Otherwise, returns the configured socket path directly.
    ///
    /// This is useful for testing and deterministic path resolution.
    #[must_use]
    pub fn resolve_socket_path(&self, runtime_dir: Option<&Path>) -> PathBuf {
        if self.engine.socket_path == "auto" {
            if let Some(dir) = runtime_dir {
                dir.join(format!("{}.sock", self.engine.name))
            } else {
                PathBuf::from(format!("/tmp/{}.sock", self.engine.name))
            }
        } else {
            PathBuf::from(&self.engine.socket_path)
        }
    }

    /// Get the resolved socket path using system directories.
    ///
    /// Uses XDG runtime directory if available, otherwise falls back to `/tmp`.
    #[must_use]
    pub fn socket_path(&self) -> PathBuf {
        let runtime_dir = directories::BaseDirs::new()
            .and_then(|dirs| dirs.runtime_dir().map(|p| p.to_path_buf()));
        self.resolve_socket_path(runtime_dir.as_deref())
    }

    /// Get enabled sources.
    pub fn enabled_sources(&self) -> impl Iterator<Item = &SourceConfig> {
        self.sources.iter().filter(|s| s.enabled)
    }

    /// Get enabled handlers.
    pub fn enabled_handlers(&self) -> impl Iterator<Item = &HandlerConfig> {
        self.handlers.iter().filter(|h| h.enabled)
    }

    /// Get enabled sinks.
    pub fn enabled_sinks(&self) -> impl Iterator<Item = &SinkConfig> {
        self.sinks.iter().filter(|s| s.enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_wildcard_topic_is_accepted() -> Result<(), Box<dyn std::error::Error>> {
        let toml = r#"
[engine]
name = "test"

[[sinks]]
name = "watchdog"
path = "/bin/true"
subscribes = ["system.started.*", "tick.out", "*"]
"#;
        let config = EmergentConfig::parse(toml)?;
        config.validate_subscription_topics()?;
        Ok(())
    }

    #[test]
    fn a_mid_string_wildcard_topic_fails_the_config() {
        let toml = r#"
[engine]
name = "test"

[[sinks]]
name = "watchdog"
path = "/bin/true"
subscribes = ["system.*.error"]
"#;
        let Err(error) = EmergentConfig::parse(toml) else {
            panic!("a topic that can never match must not load");
        };
        let message = error.to_string();
        assert!(message.contains("watchdog"), "got: {message}");
        assert!(message.contains("system.*.error"), "got: {message}");
    }

    #[test]
    fn a_mid_string_wildcard_on_a_handler_fails_the_config() {
        let toml = r#"
[engine]
name = "test"

[[handlers]]
name = "router"
path = "/bin/true"
subscribes = ["*.tick"]
publishes = ["out"]
"#;
        let Err(error) = EmergentConfig::parse(toml) else {
            panic!("a topic that can never match must not load");
        };
        let message = error.to_string();
        assert!(message.contains("router"), "got: {message}");
        assert!(message.contains("*.tick"), "got: {message}");
    }

    #[test]
    fn test_parse_minimal_config() -> Result<(), Box<dyn std::error::Error>> {
        let toml = r#"
[engine]
name = "test"
"#;
        let config = EmergentConfig::parse(toml)?;
        assert_eq!(config.engine.name, "test");
        assert_eq!(config.engine.wire_format, None);
        Ok(())
    }

    #[test]
    fn test_parse_full_config() -> Result<(), Box<dyn std::error::Error>> {
        let toml = r#"
[engine]
name = "emergent"
socket_path = "auto"
wire_format = "messagepack"
api_port = 9000

[event_store]
json_log_dir = "./logs"
sqlite_path = "./events.db"
retention_days = 30

[[sources]]
name = "timer"
path = "/bin/true"
args = ["--interval", "5s"]
enabled = true
publishes = ["timer.tick"]

[[handlers]]
name = "filter"
path = "/bin/true"
enabled = true
subscribes = ["timer.tick"]
publishes = ["timer.filtered"]

[[sinks]]
name = "logger"
path = "/bin/true"
enabled = true
subscribes = ["timer.filtered"]
"#;

        let config = EmergentConfig::parse(toml)?;
        assert_eq!(config.engine.api_port, 9000);
        assert_eq!(config.sources.len(), 1);
        assert_eq!(config.handlers.len(), 1);
        assert_eq!(config.sinks.len(), 1);
        assert_eq!(config.sources[0].name, "timer");
        assert_eq!(config.handlers[0].subscribes, vec!["timer.tick"]);
        assert_eq!(config.sinks[0].subscribes, vec!["timer.filtered"]);
        Ok(())
    }

    #[test]
    fn test_wire_format_parsing() -> Result<(), Box<dyn std::error::Error>> {
        let json_config = r#"
[engine]
wire_format = "json"
"#;
        let config = EmergentConfig::parse(json_config)?;
        assert_eq!(config.engine.wire_format, Some(WireFormat::Json));

        let msgpack_config = r#"
[engine]
wire_format = "messagepack"
"#;
        let config = EmergentConfig::parse(msgpack_config)?;
        assert_eq!(config.engine.wire_format, Some(WireFormat::Messagepack));
        Ok(())
    }

    #[test]
    fn wire_format_warning_is_silent_when_the_key_is_absent() {
        assert_eq!(wire_format_warning(None), None);
    }

    #[test]
    fn wire_format_warning_names_the_setting_and_the_real_format() {
        let json = wire_format_warning(Some(WireFormat::Json)).unwrap_or_default();
        assert!(json.contains("wire_format = \"json\""), "got: {json}");
        assert!(json.contains("no effect"), "got: {json}");
        assert!(json.contains("MessagePack"), "got: {json}");

        let msgpack = wire_format_warning(Some(WireFormat::Messagepack)).unwrap_or_default();
        assert!(
            msgpack.contains("wire_format = \"messagepack\""),
            "got: {msgpack}"
        );
    }

    #[test]
    fn unknown_engine_key_is_a_load_error_naming_the_key() {
        let toml = r#"
[engine]
name = "test"
not_a_real_key = true
"#;
        let Err(error) = EmergentConfig::parse(toml) else {
            panic!("expected an unknown key to fail the load");
        };
        let message = error.to_string();
        assert!(message.contains("not_a_real_key"), "got: {message}");
    }

    #[test]
    fn unknown_event_store_key_is_a_load_error() {
        let toml = r#"
[event_store]
retension_days = 7
"#;
        let Err(error) = EmergentConfig::parse(toml) else {
            panic!("expected a misspelled retention key to fail the load");
        };
        let message = error.to_string();
        assert!(message.contains("retension_days"), "got: {message}");
        assert!(message.contains("retention_days"), "got: {message}");
    }

    #[test]
    fn restart_keys_load_under_the_strict_schema() -> Result<(), ConfigError> {
        let toml = r#"
[[handlers]]
name = "filter"
path = "/bin/true"
subscribes = ["timer.tick"]
restart = "on-failure"
restart_backoff_ms = 250
restart_max_retries = 7
"#;
        let config = EmergentConfig::parse(toml)?;
        let restart = &config.handlers[0].restart;
        assert_eq!(restart.restart, "on-failure");
        assert_eq!(restart.restart_backoff_ms, 250);
        assert_eq!(restart.restart_max_retries, 7);
        Ok(())
    }

    #[test]
    fn misspelled_restart_key_is_a_load_error() {
        for table in ["sources", "handlers", "sinks"] {
            let toml = format!(
                r#"
[[{table}]]
name = "p"
path = "/bin/true"
restart = "always"
restart_backof_ms = 250
"#
            );
            let Err(error) = EmergentConfig::parse(&toml) else {
                panic!("expected a misspelled restart key to fail the load in [[{table}]]");
            };
            let message = error.to_string();
            assert!(message.contains("restart_backof_ms"), "got: {message}");
        }
    }

    #[test]
    fn unknown_primitive_key_is_a_load_error() {
        let toml = r#"
[[handlers]]
name = "filter"
path = "/bin/true"
subscribe = ["timer.tick"]
"#;
        let Err(error) = EmergentConfig::parse(toml) else {
            panic!("expected a singular subscribe key to fail the load");
        };
        let message = error.to_string();
        assert!(message.contains("subscribe"), "got: {message}");
    }

    #[test]
    fn unknown_top_level_table_is_a_load_error() {
        let toml = r#"
[engines]
name = "test"
"#;
        let Err(error) = EmergentConfig::parse(toml) else {
            panic!("expected an unknown top-level table to fail the load");
        };
        let message = error.to_string();
        assert!(message.contains("engines"), "got: {message}");
    }

    #[test]
    fn test_validate_unique_names_success() {
        let config = EmergentConfig {
            sources: vec![SourceConfig {
                name: "source1".to_string(),
                path: PathBuf::from("/bin/true"),
                args: vec![],
                enabled: true,
                publishes: vec![],
                env: std::collections::HashMap::new(),
                restart: RestartConfig::default(),
            }],
            handlers: vec![HandlerConfig {
                name: "handler1".to_string(),
                path: PathBuf::from("/bin/true"),
                args: vec![],
                enabled: true,
                subscribes: vec![],
                publishes: vec![],
                env: std::collections::HashMap::new(),
                unwrap_stdout: false,
                restart: RestartConfig::default(),
            }],
            sinks: vec![SinkConfig {
                name: "sink1".to_string(),
                path: PathBuf::from("/bin/true"),
                args: vec![],
                enabled: true,
                subscribes: vec![],
                env: std::collections::HashMap::new(),
                unwrap_stdout: false,
                restart: RestartConfig::default(),
            }],
            ..Default::default()
        };

        // Pure validation should pass with unique names
        assert!(config.validate_unique_names().is_ok());
    }

    #[test]
    fn test_validate_unique_names_duplicate_source() {
        let config = EmergentConfig {
            sources: vec![
                SourceConfig {
                    name: "duplicate".to_string(),
                    path: PathBuf::from("/bin/true"),
                    args: vec![],
                    enabled: true,
                    publishes: vec![],
                    env: std::collections::HashMap::new(),
                    restart: RestartConfig::default(),
                },
                SourceConfig {
                    name: "duplicate".to_string(),
                    path: PathBuf::from("/bin/true"),
                    args: vec![],
                    enabled: true,
                    publishes: vec![],
                    env: std::collections::HashMap::new(),
                    restart: RestartConfig::default(),
                },
            ],
            ..Default::default()
        };

        let Err(err) = config.validate_unique_names() else {
            panic!("expected duplicate name validation to fail");
        };
        assert!(err.to_string().contains("Duplicate name: duplicate"));
    }

    #[test]
    fn test_validate_unique_names_cross_kind_duplicate() {
        let config = EmergentConfig {
            sources: vec![SourceConfig {
                name: "shared_name".to_string(),
                path: PathBuf::from("/bin/true"),
                args: vec![],
                enabled: true,
                publishes: vec![],
                env: std::collections::HashMap::new(),
                restart: RestartConfig::default(),
            }],
            sinks: vec![SinkConfig {
                name: "shared_name".to_string(),
                path: PathBuf::from("/bin/true"),
                args: vec![],
                enabled: true,
                subscribes: vec![],
                env: std::collections::HashMap::new(),
                unwrap_stdout: false,
                restart: RestartConfig::default(),
            }],
            ..Default::default()
        };

        // Cross-kind duplicates should also fail
        let result = config.validate_unique_names();
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_socket_path_explicit() {
        let config = EmergentConfig {
            engine: EngineConfig {
                name: "test-engine".to_string(),
                socket_path: "/custom/path.sock".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        // Explicit path ignores runtime_dir
        let path = config.resolve_socket_path(Some(Path::new("/run/user/1000")));
        assert_eq!(path, PathBuf::from("/custom/path.sock"));

        // Explicit path also works with None
        let path = config.resolve_socket_path(None);
        assert_eq!(path, PathBuf::from("/custom/path.sock"));
    }

    #[test]
    fn test_resolve_socket_path_auto_with_runtime_dir() {
        let config = EmergentConfig {
            engine: EngineConfig {
                name: "my-engine".to_string(),
                socket_path: "auto".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        // With runtime directory provided
        let path = config.resolve_socket_path(Some(Path::new("/run/user/1000")));
        assert_eq!(path, PathBuf::from("/run/user/1000/my-engine.sock"));
    }

    #[test]
    fn test_resolve_socket_path_auto_fallback() {
        let config = EmergentConfig {
            engine: EngineConfig {
                name: "fallback-engine".to_string(),
                socket_path: "auto".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        // Without runtime directory, falls back to /tmp
        let path = config.resolve_socket_path(None);
        assert_eq!(path, PathBuf::from("/tmp/fallback-engine.sock"));
    }

    #[test]
    fn test_primitive_config_trait() {
        let source = SourceConfig {
            name: "my-source".to_string(),
            path: PathBuf::from("/usr/bin/source"),
            args: vec![],
            enabled: true,
            publishes: vec![],
            env: std::collections::HashMap::new(),
            restart: RestartConfig::default(),
        };

        // Test trait implementation
        assert_eq!(source.name(), "my-source");
        assert_eq!(source.path(), Path::new("/usr/bin/source"));
        assert!(source.is_enabled());
    }

    #[test]
    fn test_expand_tilde_home_only() {
        let home = Path::new("/home/user");
        let result = expand_tilde(Path::new("~"), Some(home));
        assert_eq!(result, PathBuf::from("/home/user"));
    }

    #[test]
    fn test_expand_tilde_home_subpath() {
        let home = Path::new("/home/user");
        let result = expand_tilde(Path::new("~/bin/app"), Some(home));
        assert_eq!(result, PathBuf::from("/home/user/bin/app"));
    }

    #[test]
    fn test_expand_tilde_absolute_unchanged() {
        let home = Path::new("/home/user");
        let result = expand_tilde(Path::new("/absolute/path"), Some(home));
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_expand_tilde_relative_unchanged() {
        let home = Path::new("/home/user");
        let result = expand_tilde(Path::new("relative/path"), Some(home));
        assert_eq!(result, PathBuf::from("relative/path"));
    }

    #[test]
    fn test_expand_tilde_no_home_dir() {
        let result = expand_tilde(Path::new("~/bin/app"), None);
        assert_eq!(result, PathBuf::from("~/bin/app"));
    }

    #[test]
    fn test_expand_tilde_user_format_unchanged() {
        // ~username/path format is not expanded (would require passwd lookup)
        let home = Path::new("/home/user");
        let result = expand_tilde(Path::new("~other/bin/app"), Some(home));
        assert_eq!(result, PathBuf::from("~other/bin/app"));
    }

    #[test]
    fn test_expand_tilde_in_middle_unchanged() {
        let home = Path::new("/home/user");
        let result = expand_tilde(Path::new("foo/~/bar"), Some(home));
        assert_eq!(result, PathBuf::from("foo/~/bar"));
    }

    #[test]
    fn test_with_expanded_paths() {
        let home = Path::new("/home/testuser");
        let config = EmergentConfig {
            sources: vec![SourceConfig {
                name: "src1".to_string(),
                path: PathBuf::from("~/bin/source"),
                args: vec![],
                enabled: true,
                publishes: vec![],
                env: std::collections::HashMap::new(),
                restart: RestartConfig::default(),
            }],
            handlers: vec![HandlerConfig {
                name: "handler1".to_string(),
                path: PathBuf::from("~/.local/bin/handler"),
                args: vec![],
                enabled: true,
                subscribes: vec![],
                publishes: vec![],
                env: std::collections::HashMap::new(),
                unwrap_stdout: false,
                restart: RestartConfig::default(),
            }],
            sinks: vec![SinkConfig {
                name: "sink1".to_string(),
                path: PathBuf::from("/absolute/sink"),
                args: vec![],
                enabled: true,
                subscribes: vec![],
                env: std::collections::HashMap::new(),
                unwrap_stdout: false,
                restart: RestartConfig::default(),
            }],
            ..Default::default()
        };

        let expanded = config.with_expanded_paths(Some(home));

        assert_eq!(
            expanded.sources[0].path,
            PathBuf::from("/home/testuser/bin/source")
        );
        assert_eq!(
            expanded.handlers[0].path,
            PathBuf::from("/home/testuser/.local/bin/handler")
        );
        // Absolute path should remain unchanged
        assert_eq!(expanded.sinks[0].path, PathBuf::from("/absolute/sink"));
    }

    #[test]
    fn test_parse_expands_tilde_paths() {
        // This test verifies that parse() expands tildes
        // We can't easily verify the actual expansion without mocking,
        // but we can verify the path doesn't stay as ~/... when a home exists
        let toml = r#"
[engine]
name = "test"

[[sources]]
name = "src"
path = "/bin/true"
enabled = true
"#;
        // This should parse without error (path exists)
        let result = EmergentConfig::parse(toml);
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_command_path_absolute_unchanged() {
        let path = Path::new("/usr/bin/something");
        let result = resolve_command_path(path);
        assert_eq!(result, PathBuf::from("/usr/bin/something"));
    }

    #[test]
    fn test_resolve_command_path_relative_unchanged() {
        let path = Path::new("./target/release/timer");
        let result = resolve_command_path(path);
        assert_eq!(result, PathBuf::from("./target/release/timer"));
    }

    #[test]
    fn test_resolve_command_path_bare_resolves() {
        // "true" is a standard Unix command that should be on PATH
        let result = resolve_command_path(Path::new("true"));
        assert!(
            result.is_absolute(),
            "Expected absolute path, got: {result:?}"
        );
    }

    #[test]
    fn test_resolve_command_path_bare_not_found() {
        let path = Path::new("nonexistent_xyz_12345");
        let result = resolve_command_path(path);
        assert_eq!(result, PathBuf::from("nonexistent_xyz_12345"));
    }

    #[test]
    fn test_default_api_port() -> Result<(), Box<dyn std::error::Error>> {
        let toml = r#"
[engine]
name = "test"
"#;
        let config = EmergentConfig::parse(toml)?;
        assert_eq!(config.engine.api_port, 8891);
        assert!(config.engine.api_enabled());
        Ok(())
    }

    #[test]
    fn test_custom_api_port() -> Result<(), Box<dyn std::error::Error>> {
        let toml = r#"
[engine]
name = "test"
api_port = 9000
"#;
        let config = EmergentConfig::parse(toml)?;
        assert_eq!(config.engine.api_port, 9000);
        assert!(config.engine.api_enabled());
        Ok(())
    }

    #[test]
    fn test_api_port_disabled() -> Result<(), Box<dyn std::error::Error>> {
        let toml = r#"
[engine]
name = "test"
api_port = 0
"#;
        let config = EmergentConfig::parse(toml)?;
        assert_eq!(config.engine.api_port, 0);
        assert!(!config.engine.api_enabled());
        Ok(())
    }

    fn sink_named(name: &str) -> SinkConfig {
        SinkConfig {
            name: name.to_string(),
            path: PathBuf::from("/bin/true"),
            args: vec![],
            enabled: true,
            subscribes: vec![],
            env: std::collections::HashMap::new(),
            unwrap_stdout: false,
            restart: RestartConfig::default(),
        }
    }

    #[test]
    fn valid_primitive_names_are_accepted() {
        for name in [
            "timer",
            "my_handler",
            "email-handler",
            "filter123",
            &"a".repeat(PrimitiveName::MAX_LENGTH),
        ] {
            assert!(
                validate_primitive_name(name).is_ok(),
                "expected '{name}' to be accepted"
            );
        }
    }

    #[test]
    fn invalid_primitive_names_are_rejected() {
        for name in [
            "Bad Name",
            "Timer",
            "inValid",
            "1timer",
            "has.dot",
            "has space",
            "",
            &"a".repeat(PrimitiveName::MAX_LENGTH + 1),
        ] {
            assert!(
                validate_primitive_name(name).is_err(),
                "expected '{name}' to be rejected"
            );
        }
    }

    #[test]
    fn every_accepted_name_forms_a_valid_system_event_type() {
        for name in ["timer", "my_handler", "email-handler", "filter123"] {
            assert!(validate_primitive_name(name).is_ok());
            assert!(
                emergent_client::types::MessageType::new(format!("system.started.{name}")).is_ok(),
                "'{name}' passed config validation but cannot form a message type"
            );
        }
    }

    #[test]
    fn name_error_reports_the_primitive_and_the_rule() {
        let Err(err) = validate_primitive_name("Bad Name") else {
            panic!("expected 'Bad Name' to be rejected");
        };
        let message = err.to_string();
        assert!(message.contains("Bad Name"), "message was: {message}");
        assert!(
            message.contains("lowercase letter"),
            "message was: {message}"
        );
    }

    #[test]
    fn validate_names_checks_sources_handlers_and_sinks() {
        let mut config = EmergentConfig {
            sinks: vec![sink_named("good")],
            ..Default::default()
        };
        assert!(config.validate_names().is_ok());

        config.sinks.push(sink_named("Bad Name"));
        assert!(config.validate_names().is_err());
    }

    #[test]
    fn validate_names_checks_disabled_primitives_too() {
        let mut sink = sink_named("Bad Name");
        sink.enabled = false;
        let config = EmergentConfig {
            sinks: vec![sink],
            ..Default::default()
        };
        assert!(config.validate_names().is_err());
    }

    #[test]
    fn parse_rejects_the_reproduction_config_from_issue_42() {
        let toml = r#"
[engine]
name = "badname-test"

[[sinks]]
name = "Bad Name"
path = "/bin/true"
subscribes = ["tick.out"]
"#;
        let Err(err) = EmergentConfig::parse(toml) else {
            panic!("expected the invalid sink name to fail config parsing");
        };
        let message = err.to_string();
        assert!(message.contains("Bad Name"), "message was: {message}");
    }
}

#[cfg(test)]
mod restart_config_tests {
    use super::*;

    const BASE: &str = r#"
[[handlers]]
name = "h1"
path = "/bin/sh"
subscribes = ["a"]
publishes = ["b"]
"#;

    fn parse_no_io(content: &str) -> Result<EmergentConfig, ConfigError> {
        let config: EmergentConfig = toml::from_str(content)?;
        config.validate_unique_names()?;
        config.validate_restart_policies()?;
        Ok(config)
    }

    #[test]
    fn restart_defaults_to_never() -> Result<(), ConfigError> {
        let config = parse_no_io(BASE)?;
        let handler = config
            .handlers
            .first()
            .ok_or_else(|| ConfigError::ValidationError("expected one handler".to_string()))?;
        assert_eq!(handler.restart.restart, "never");
        assert_eq!(handler.restart.policy(), Some(RestartPolicy::Never));
        assert_eq!(handler.restart.limits(), RestartLimits::default());
        Ok(())
    }

    #[test]
    fn restart_keys_are_read_from_the_primitive_table() -> Result<(), ConfigError> {
        let content = format!(
            "{BASE}restart = \"on-failure\"\nrestart_backoff_ms = 100\nrestart_max_backoff_ms = 900\nrestart_max_retries = 2\nrestart_window_ms = 7000\n"
        );
        let config = parse_no_io(&content)?;
        let handler = config
            .handlers
            .first()
            .ok_or_else(|| ConfigError::ValidationError("expected one handler".to_string()))?;
        assert_eq!(handler.restart.policy(), Some(RestartPolicy::OnFailure));
        assert_eq!(
            handler.restart.limits(),
            RestartLimits {
                backoff_ms: 100,
                max_backoff_ms: 900,
                max_retries: 2,
                window_ms: 7000,
            }
        );
        Ok(())
    }

    #[test]
    fn unknown_restart_value_names_the_primitive_and_the_value() {
        let content = format!("{BASE}restart = \"sometimes\"\n");
        let Err(err) = parse_no_io(&content) else {
            panic!("expected a validation error");
        };
        let msg = err.to_string();
        assert!(
            msg.contains("h1"),
            "message should name the primitive: {msg}"
        );
        assert!(
            msg.contains("sometimes"),
            "message should name the bad value: {msg}"
        );
        assert!(
            msg.contains("on-failure"),
            "message should list valid values: {msg}"
        );
    }

    #[test]
    fn restart_applies_to_sources_and_sinks_too() -> Result<(), ConfigError> {
        let content = r#"
[[sources]]
name = "s1"
path = "/bin/sh"
publishes = ["a"]
restart = "always"

[[sinks]]
name = "k1"
path = "/bin/sh"
subscribes = ["a"]
restart = "on-failure"
restart_max_retries = 9
"#;
        let config = parse_no_io(content)?;
        let source = config
            .sources
            .first()
            .ok_or_else(|| ConfigError::ValidationError("expected a source".to_string()))?;
        let sink = config
            .sinks
            .first()
            .ok_or_else(|| ConfigError::ValidationError("expected a sink".to_string()))?;
        assert_eq!(source.restart.policy(), Some(RestartPolicy::Always));
        assert_eq!(sink.restart.policy(), Some(RestartPolicy::OnFailure));
        assert_eq!(sink.restart.limits().max_retries, 9);
        Ok(())
    }

    #[test]
    fn engine_shutdown_timings_default_to_the_historical_waits() {
        let engine = EngineConfig::default();
        assert_eq!(engine.shutdown_drain_ms, 500);
        assert_eq!(engine.shutdown_grace_ms, 2_000);
        assert_eq!(
            engine.shutdown_drain(),
            std::time::Duration::from_millis(500)
        );
        assert_eq!(engine.shutdown_grace(), std::time::Duration::from_secs(2));
    }

    #[test]
    fn engine_shutdown_timings_are_configurable() -> Result<(), ConfigError> {
        let config = parse_no_io("[engine]\nshutdown_drain_ms = 50\nshutdown_grace_ms = 250\n")?;
        assert_eq!(config.engine.shutdown_drain_ms, 50);
        assert_eq!(config.engine.shutdown_grace_ms, 250);
        Ok(())
    }
}
