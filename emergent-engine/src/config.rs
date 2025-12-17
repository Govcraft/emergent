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

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate source paths exist
        for source in &self.sources {
            if source.enabled && !source.path.exists() {
                return Err(ConfigError::PathNotFound(source.path.clone()));
            }
        }

        // Validate handler paths exist
        for handler in &self.handlers {
            if handler.enabled && !handler.path.exists() {
                return Err(ConfigError::PathNotFound(handler.path.clone()));
            }
        }

        // Validate sink paths exist
        for sink in &self.sinks {
            if sink.enabled && !sink.path.exists() {
                return Err(ConfigError::PathNotFound(sink.path.clone()));
            }
        }

        // Check for duplicate names
        let mut names = std::collections::HashSet::new();
        for source in &self.sources {
            if !names.insert(&source.name) {
                return Err(ConfigError::ValidationError(format!(
                    "Duplicate name: {}",
                    source.name
                )));
            }
        }
        for handler in &self.handlers {
            if !names.insert(&handler.name) {
                return Err(ConfigError::ValidationError(format!(
                    "Duplicate name: {}",
                    handler.name
                )));
            }
        }
        for sink in &self.sinks {
            if !names.insert(&sink.name) {
                return Err(ConfigError::ValidationError(format!(
                    "Duplicate name: {}",
                    sink.name
                )));
            }
        }

        Ok(())
    }

    /// Get the resolved socket path.
    pub fn socket_path(&self) -> PathBuf {
        if self.engine.socket_path == "auto" {
            // Use XDG runtime directory or fallback to /tmp
            if let Some(runtime_dir) = directories::BaseDirs::new()
                .and_then(|dirs| dirs.runtime_dir().map(|p| p.to_path_buf()))
            {
                runtime_dir.join(format!("{}.sock", self.engine.name))
            } else {
                PathBuf::from(format!("/tmp/{}.sock", self.engine.name))
            }
        } else {
            PathBuf::from(&self.engine.socket_path)
        }
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
    fn test_parse_minimal_config() {
        let toml = r#"
[engine]
name = "test"
"#;
        let config = EmergentConfig::parse(toml).unwrap();
        assert_eq!(config.engine.name, "test");
        assert_eq!(config.engine.wire_format, WireFormat::Messagepack);
    }

    #[test]
    fn test_parse_full_config() {
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

        let config = EmergentConfig::parse(toml).unwrap();
        assert_eq!(config.sources.len(), 1);
        assert_eq!(config.handlers.len(), 1);
        assert_eq!(config.sinks.len(), 1);
        assert_eq!(config.sources[0].name, "timer");
        assert_eq!(config.handlers[0].subscribes, vec!["timer.tick"]);
        assert_eq!(config.sinks[0].subscribes, vec!["timer.filtered"]);
    }

    #[test]
    fn test_wire_format_parsing() {
        let json_config = r#"
[engine]
wire_format = "json"
"#;
        let config = EmergentConfig::parse(json_config).unwrap();
        assert_eq!(config.engine.wire_format, WireFormat::Json);

        let msgpack_config = r#"
[engine]
wire_format = "messagepack"
"#;
        let config = EmergentConfig::parse(msgpack_config).unwrap();
        assert_eq!(config.engine.wire_format, WireFormat::Messagepack);
    }
}
