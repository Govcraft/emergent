//! Configuration loading and management for the Emergent Engine.
//!
//! The configuration is stored in TOML format and defines:
//! - Engine settings (name, socket path, wire format)
//! - Event store settings (log directory, SQLite path, retention)
//! - Sources, Handlers, and Sinks to manage

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

/// Wire format for IPC communication.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WireFormat {
    /// JSON format (human-readable, useful for debugging)
    Json,
    /// MessagePack format (binary, more efficient)
    #[default]
    Messagepack,
}

/// Engine configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Name of this engine instance.
    #[serde(default = "default_engine_name")]
    pub name: String,

    /// Socket path for IPC. Use "auto" for XDG-compliant default.
    #[serde(default = "default_socket_path")]
    pub socket_path: String,

    /// Wire format for IPC communication.
    #[serde(default)]
    pub wire_format: WireFormat,
}

fn default_engine_name() -> String {
    "emergent".to_string()
}

fn default_socket_path() -> String {
    "auto".to_string()
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            name: default_engine_name(),
            socket_path: default_socket_path(),
            wire_format: WireFormat::default(),
        }
    }
}

/// Event store configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventStoreConfig {
    /// Directory for JSON log files.
    #[serde(default = "default_json_log_dir")]
    pub json_log_dir: PathBuf,

    /// Path to SQLite database.
    #[serde(default = "default_sqlite_path")]
    pub sqlite_path: PathBuf,

    /// Retention period in days for old events.
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

fn default_json_log_dir() -> PathBuf {
    PathBuf::from("./logs")
}

fn default_sqlite_path() -> PathBuf {
    PathBuf::from("./events.db")
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

/// Configuration for a Source primitive.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Configuration for a Handler primitive.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Configuration for a Sink primitive.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

const fn default_enabled() -> bool {
    true
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
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: EmergentConfig = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Load configuration from a TOML string.
    pub fn parse(content: &str) -> Result<Self, ConfigError> {
        let config: EmergentConfig = toml::from_str(content)?;
        config.validate()?;
        Ok(config)
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

    /// Validate that all enabled primitive paths exist (impure function).
    ///
    /// Performs filesystem checks to verify executable paths exist.
    pub fn validate_paths(&self) -> Result<(), ConfigError> {
        check_paths_exist(&self.sources)?;
        check_paths_exist(&self.handlers)?;
        check_paths_exist(&self.sinks)?;
        Ok(())
    }

    /// Validate the configuration (combines structure and path validation).
    ///
    /// This is a convenience method that runs both pure validation (duplicate names)
    /// and impure validation (path existence checks).
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_unique_names()?;
        self.validate_paths()?;
        Ok(())
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
    fn test_parse_minimal_config() -> Result<(), Box<dyn std::error::Error>> {
        let toml = r#"
[engine]
name = "test"
"#;
        let config = EmergentConfig::parse(toml)?;
        assert_eq!(config.engine.name, "test");
        assert_eq!(config.engine.wire_format, WireFormat::Messagepack);
        Ok(())
    }

    #[test]
    fn test_parse_full_config() -> Result<(), Box<dyn std::error::Error>> {
        let toml = r#"
[engine]
name = "emergent"
socket_path = "auto"
wire_format = "messagepack"

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
        assert_eq!(config.engine.wire_format, WireFormat::Json);

        let msgpack_config = r#"
[engine]
wire_format = "messagepack"
"#;
        let config = EmergentConfig::parse(msgpack_config)?;
        assert_eq!(config.engine.wire_format, WireFormat::Messagepack);
        Ok(())
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
            }],
            handlers: vec![HandlerConfig {
                name: "handler1".to_string(),
                path: PathBuf::from("/bin/true"),
                args: vec![],
                enabled: true,
                subscribes: vec![],
                publishes: vec![],
                env: std::collections::HashMap::new(),
            }],
            sinks: vec![SinkConfig {
                name: "sink1".to_string(),
                path: PathBuf::from("/bin/true"),
                args: vec![],
                enabled: true,
                subscribes: vec![],
                env: std::collections::HashMap::new(),
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
                },
                SourceConfig {
                    name: "duplicate".to_string(),
                    path: PathBuf::from("/bin/true"),
                    args: vec![],
                    enabled: true,
                    publishes: vec![],
                    env: std::collections::HashMap::new(),
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
            }],
            sinks: vec![SinkConfig {
                name: "shared_name".to_string(),
                path: PathBuf::from("/bin/true"),
                args: vec![],
                enabled: true,
                subscribes: vec![],
                env: std::collections::HashMap::new(),
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
                wire_format: WireFormat::default(),
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
                wire_format: WireFormat::default(),
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
                wire_format: WireFormat::default(),
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
        };

        // Test trait implementation
        assert_eq!(source.name(), "my-source");
        assert_eq!(source.path(), Path::new("/usr/bin/source"));
        assert!(source.is_enabled());
    }
}
