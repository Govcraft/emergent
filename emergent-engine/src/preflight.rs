//! The checks the engine runs before it spawns anything, as one call.
//!
//! Startup and `emergent validate` both go through [`preflight`], so a config
//! that validates is one the engine will start, and a rule added here is
//! enforced in both places at once. Startup refuses on the first error;
//! `validate` reports every one of them.
//!
//! The checks are [`EmergentConfig::issues`] (names, duplicates, restart
//! policies, subscription topics, `api_allowed_hosts`, primitive paths), the
//! IPC connection-capacity check against the limit acton-reactive resolves
//! ([`effective_max_connections`]), and the warnings startup logs.

use crate::config::{
    ConfigError, ConfigIssue, EmergentConfig, IssueCode, PathCheck, check_connection_capacity,
    wire_format_warning,
};
use acton_reactive::ipc::IpcConfig;
use serde::Serialize;
use std::path::Path;

/// What [`preflight`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Findings {
    /// Problems that stop the engine from starting.
    pub errors: Vec<ConfigIssue>,
    /// Problems startup logs and carries on past.
    pub warnings: Vec<ConfigIssue>,
}

impl Findings {
    /// Whether the engine would start with this configuration.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// The first error as the [`ConfigError`] startup refuses with.
    ///
    /// # Errors
    ///
    /// Returns the first error, when there is one.
    pub fn into_result(self) -> Result<Vec<ConfigIssue>, ConfigError> {
        match self.errors.into_iter().next() {
            Some(issue) => Err(issue.into_error()),
            None => Ok(self.warnings),
        }
    }
}

/// Run every pre-spawn check against a loaded configuration (impure only when
/// `paths` is [`PathCheck::Check`], which reads the filesystem).
///
/// `max_connections` is the effective IPC connection limit, normally
/// [`effective_max_connections`]; it is a parameter so the decision can be
/// tested without an `ipc.toml`.
#[must_use]
pub fn preflight(config: &EmergentConfig, max_connections: usize, paths: PathCheck) -> Findings {
    let mut errors = config.issues(paths);
    if let Err(e) = check_connection_capacity(config.enabled_primitive_count(), max_connections) {
        let message = match e {
            ConfigError::ValidationError(message) => message,
            other => other.to_string(),
        };
        errors.push(
            ConfigIssue::new(IssueCode::ConnectionCapacity, message).at("engine.max_connections"),
        );
    }

    let warnings = wire_format_warning(config.engine.wire_format)
        .map(|message| {
            ConfigIssue::new(IssueCode::WireFormatIgnored, message).at("engine.wire_format")
        })
        .into_iter()
        .collect();

    Findings { errors, warnings }
}

/// The IPC configuration the engine listens with.
///
/// [`IpcConfig::load`] resolves acton-reactive's own defaults and
/// `$XDG_CONFIG_HOME/acton/ipc.toml` first, then
/// [`apply_engine_ipc_overrides`] applies the engine-owned settings.
#[must_use]
pub fn ipc_config(socket_path: Option<&Path>, max_connections: Option<usize>) -> IpcConfig {
    apply_engine_ipc_overrides(IpcConfig::load(), socket_path, max_connections)
}

/// Apply engine-owned settings without changing admission or subscription
/// deadlines.
///
/// Idle reads are disabled for long-lived publish-only primitives; the
/// independent policy admission and subscription deadlines remain unchanged.
/// The socket path, when given, always comes from `[engine]`.
/// `max_connections` overrides the resolved limit only when
/// `[engine].max_connections` is set, so leaving the key out keeps whatever
/// acton resolved.
#[must_use]
pub fn apply_engine_ipc_overrides(
    mut ipc_config: IpcConfig,
    socket_path: Option<&Path>,
    max_connections: Option<usize>,
) -> IpcConfig {
    // A source can legitimately wait indefinitely between external events.
    ipc_config.timeouts.read = 0;
    if let Some(path) = socket_path {
        ipc_config.socket.path = Some(path.to_path_buf());
    }
    if let Some(limit) = max_connections {
        ipc_config.limits.max_connections = limit;
    }
    ipc_config
}

/// The connection limit the engine would run under with this configuration.
#[must_use]
pub fn effective_max_connections(config: &EmergentConfig) -> usize {
    ipc_config(None, config.engine.max_connections)
        .limits
        .max_connections
}

/// Convert a read or parse failure into the issue `validate` reports.
///
/// A parse error keeps toml's own sentence and adds the line and column; an
/// unknown key gets its own code and the configuration path it was found at,
/// such as `sinks[3].publishes`, because that is the mistake a schema change
/// makes most often and a control plane wants to point at it.
#[must_use]
pub fn load_issue(error: &ConfigError, content: Option<&str>) -> ConfigIssue {
    match error {
        ConfigError::ParseError(e) => {
            let message = e.message().trim().to_string();
            let unknown_key = unknown_field_name(&message);
            let code = if unknown_key.is_some() {
                IssueCode::UnknownField
            } else {
                IssueCode::ParseError
            };
            let located = content.zip(e.span()).map(|(text, span)| {
                let (line, column) = line_and_column(text, span.start);
                (line, column, table_path_at(text, span.start))
            });
            match located {
                Some((line, column, table)) => {
                    let issue =
                        ConfigIssue::new(code, format!("{message} (line {line}, column {column})"));
                    match (unknown_key, table) {
                        (Some(key), Some(table)) => issue.at(format!("{table}.{key}")),
                        (Some(key), None) => issue.at(key),
                        (None, Some(table)) => issue.at(table),
                        (None, None) => issue,
                    }
                }
                None => ConfigIssue::new(code, message),
            }
        }
        ConfigError::ReadError(e) => ConfigIssue::new(IssueCode::ReadError, e.to_string()),
        ConfigError::ValidationError(message) => {
            ConfigIssue::new(IssueCode::ParseError, message.clone())
        }
        ConfigError::PathNotFound(path) => ConfigIssue::new(
            IssueCode::PathNotFound,
            format!("Path does not exist: {}", path.display()),
        ),
    }
}

/// The key named by serde's "unknown field `x`, expected ..." (pure function).
fn unknown_field_name(message: &str) -> Option<&str> {
    let rest = message.strip_prefix("unknown field `")?;
    rest.split_once('`').map(|(key, _)| key)
}

/// One-based line and column of a byte offset (pure function).
fn line_and_column(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit_once('\n')
        .map_or(before, |(_, tail)| tail)
        .chars()
        .count()
        + 1;
    (line, column)
}

/// The table a byte offset sits in, spelled as a configuration path (pure function).
///
/// `[[sinks]]` is an array of tables, so the third one is `sinks[2]`; a
/// sub-table header such as `[sinks.restart]` belongs to the latest element,
/// giving `sinks[2].restart`. Returns `None` before any header. This reads
/// header lines only, which is how every shipped config and every
/// `emergent init` config is written; inline tables are reported at the table
/// that holds them.
fn table_path_at(text: &str, offset: usize) -> Option<String> {
    // The line holding the offset counts: toml points an error inside an
    // array-of-tables element at that element's `[[header]]`.
    let line_end = text
        .get(offset..)
        .and_then(|rest| rest.find('\n'))
        .map_or(text.len(), |n| offset + n);
    let before = text.get(..line_end).unwrap_or(text);
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut current: Option<String> = None;

    for line in before.lines() {
        let line = line.trim();
        if let Some(name) = line
            .strip_prefix("[[")
            .and_then(|rest| rest.split_once("]]"))
            .map(|(name, _)| name.trim())
        {
            let count = counts.entry(name.to_string()).or_insert(0);
            current = Some(format!("{name}[{count}]"));
            *count += 1;
        } else if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.split_once(']'))
            .map(|(name, _)| name.trim())
        {
            let (head, tail) = name
                .split_once('.')
                .map_or((name, None), |(h, t)| (h, Some(t)));
            let head = counts.get(head).map_or_else(
                || head.to_string(),
                |n| format!("{head}[{}]", n.saturating_sub(1)),
            );
            current = Some(tail.map_or_else(|| head.clone(), |tail| format!("{head}.{tail}")));
        }
    }
    current
}

/// The result `emergent validate --json` prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationReport {
    /// Whether the engine would start with this configuration.
    pub ok: bool,
    /// The version of the engine that ran the checks.
    pub engine_version: String,
    /// Problems that stop the engine from starting.
    pub errors: Vec<ConfigIssue>,
    /// Problems startup logs and carries on past.
    pub warnings: Vec<ConfigIssue>,
}

impl ValidationReport {
    /// A report of what [`preflight`] found.
    #[must_use]
    pub fn from_findings(findings: Findings) -> Self {
        Self {
            ok: findings.is_ok(),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            errors: findings.errors,
            warnings: findings.warnings,
        }
    }

    /// A report of a configuration that could not be loaded at all.
    #[must_use]
    pub fn failed(issue: ConfigIssue) -> Self {
        Self::from_findings(Findings {
            errors: vec![issue],
            warnings: Vec::new(),
        })
    }

    /// The process exit code for this report: 0 when ok, 1 otherwise.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        if self.ok { 0 } else { 1 }
    }

    /// The report as an operator reads it.
    #[must_use]
    pub fn to_human(&self, config_path: &Path) -> String {
        let mut out = String::new();
        for issue in &self.errors {
            out.push_str(&format_issue("error", issue));
        }
        for issue in &self.warnings {
            out.push_str(&format_issue("warning", issue));
        }
        if self.ok {
            out.push_str(&format!(
                "{} is valid for emergent {}",
                config_path.display(),
                self.engine_version
            ));
        } else {
            out.push_str(&format!(
                "{} is invalid: {} error(s)",
                config_path.display(),
                self.errors.len()
            ));
        }
        if !self.warnings.is_empty() {
            out.push_str(&format!(", {} warning(s)", self.warnings.len()));
        }
        out.push('\n');
        out
    }
}

/// One issue as a line of human output (pure function).
fn format_issue(severity: &str, issue: &ConfigIssue) -> String {
    match &issue.path {
        Some(path) => format!("{severity}[{}] at {path}: {}\n", issue.code, issue.message),
        None => format!("{severity}[{}]: {}\n", issue.code, issue.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RESERVED_IPC_CONNECTIONS;

    const THREE_SINKS: &str = r#"
[engine]
name = "t"

[[sinks]]
name = "a"
path = "/bin/sh"
subscribes = ["x"]

[[sinks]]
name = "b"
path = "/bin/sh"
subscribes = ["x"]

[[sinks]]
name = "c"
path = "/bin/sh"
subscribes = ["x"]
"#;

    fn parsed(content: &str) -> Result<EmergentConfig, ConfigError> {
        EmergentConfig::parse_unchecked(content)
    }

    #[test]
    fn a_valid_config_has_no_findings() -> Result<(), ConfigError> {
        let findings = preflight(&parsed(THREE_SINKS)?, 100, PathCheck::Check);
        assert!(findings.is_ok());
        assert!(findings.warnings.is_empty());
        Ok(())
    }

    #[test]
    fn capacity_is_checked_against_the_limit_given() -> Result<(), ConfigError> {
        let config = parsed(THREE_SINKS)?;
        let enough = 3 + RESERVED_IPC_CONNECTIONS;
        assert!(preflight(&config, enough, PathCheck::Check).is_ok());

        let findings = preflight(&config, enough - 1, PathCheck::Check);
        assert_eq!(findings.errors.len(), 1);
        assert_eq!(findings.errors[0].code, IssueCode::ConnectionCapacity);
        assert_eq!(
            findings.errors[0].path.as_deref(),
            Some("engine.max_connections")
        );
        assert!(
            findings.errors[0]
                .message
                .contains("[engine].max_connections")
        );
        Ok(())
    }

    #[test]
    fn every_error_is_reported_not_only_the_first() -> Result<(), ConfigError> {
        let content = r#"
[engine]
api_allowed_hosts = ["http://example.com"]

[[handlers]]
name = "Bad"
path = "/nonexistent/emergent-preflight-a"
subscribes = ["a.*.b"]
restart = "sometimes"

[[sinks]]
name = "dup"
path = "/nonexistent/emergent-preflight-b"
subscribes = ["x"]

[[sinks]]
name = "dup"
path = "/bin/sh"
subscribes = ["x"]
"#;
        let findings = preflight(&parsed(content)?, 100, PathCheck::Check);
        let codes: Vec<_> = findings
            .errors
            .iter()
            .map(|i| (i.code, i.path.clone().unwrap_or_default()))
            .collect();
        assert_eq!(
            codes,
            vec![
                (IssueCode::InvalidName, "handlers[0].name".to_string()),
                (IssueCode::DuplicateName, "sinks[1].name".to_string()),
                (
                    IssueCode::InvalidRestartPolicy,
                    "handlers[0].restart".to_string()
                ),
                (
                    IssueCode::InvalidSubscriptionTopic,
                    "handlers[0].subscribes[0]".to_string()
                ),
                (
                    IssueCode::InvalidApiAllowedHost,
                    "engine.api_allowed_hosts".to_string()
                ),
                (IssueCode::PathNotFound, "handlers[0].path".to_string()),
                (IssueCode::PathNotFound, "sinks[0].path".to_string()),
            ]
        );
        Ok(())
    }

    #[test]
    fn the_first_error_is_the_one_startup_refuses_with() -> Result<(), ConfigError> {
        let content = r#"
[[sinks]]
name = "a"
path = "/nonexistent/emergent-preflight-c"
subscribes = ["x"]
"#;
        let config = parsed(content)?;
        let from_preflight = preflight(&config, 100, PathCheck::Check)
            .into_result()
            .err()
            .map(|e| e.to_string());
        let from_validate = config.validate().err().map(|e| e.to_string());
        assert_eq!(from_preflight, from_validate);
        assert_eq!(
            from_validate.as_deref(),
            Some("Path does not exist: /nonexistent/emergent-preflight-c")
        );
        Ok(())
    }

    #[test]
    fn skipping_the_path_check_skips_only_that_check() -> Result<(), ConfigError> {
        let content = r#"
[[sinks]]
name = "a"
path = "/nonexistent/emergent-preflight-d"
subscribes = ["a.*.b"]
"#;
        let findings = preflight(&parsed(content)?, 100, PathCheck::Skip);
        let codes: Vec<_> = findings.errors.iter().map(|i| i.code).collect();
        assert_eq!(codes, vec![IssueCode::InvalidSubscriptionTopic]);
        Ok(())
    }

    #[test]
    fn a_disabled_primitive_needs_no_path_and_no_connection() -> Result<(), ConfigError> {
        let content = r#"
[[sinks]]
name = "a"
path = "/nonexistent/emergent-preflight-e"
enabled = false
subscribes = ["x"]
"#;
        let findings = preflight(
            &parsed(content)?,
            RESERVED_IPC_CONNECTIONS,
            PathCheck::Check,
        );
        assert!(findings.is_ok());
        Ok(())
    }

    #[test]
    fn wire_format_is_a_warning_not_an_error() -> Result<(), ConfigError> {
        let content = "[engine]\nwire_format = \"json\"\n";
        let findings = preflight(&parsed(content)?, 100, PathCheck::Check);
        assert!(findings.is_ok());
        assert_eq!(findings.warnings.len(), 1);
        assert_eq!(findings.warnings[0].code, IssueCode::WireFormatIgnored);
        Ok(())
    }

    #[test]
    fn an_unknown_key_is_reported_with_its_table_and_line() {
        let content = "[engine]\nname = \"t\"\n\n[[sinks]]\nname = \"a\"\npath = \"/bin/sh\"\n\n[[sinks]]\nname = \"b\"\npath = \"/bin/sh\"\npublishes = [\"x\"]\n";
        let Err(error) = parsed(content) else {
            panic!("a sink with publishes must not parse");
        };
        let issue = load_issue(&error, Some(content));
        assert_eq!(issue.code, IssueCode::UnknownField);
        assert_eq!(issue.path.as_deref(), Some("sinks[1].publishes"));
        assert!(issue.message.starts_with("unknown field `publishes`"));
        // toml points at the `[[sinks]]` header of the element holding the key.
        assert!(issue.message.ends_with("(line 8, column 1)"));
    }

    #[test]
    fn a_syntax_error_is_a_parse_error() {
        let content = "[engine\nname = 1\n";
        let Err(error) = parsed(content) else {
            panic!("an unclosed header must not parse");
        };
        let issue = load_issue(&error, Some(content));
        assert_eq!(issue.code, IssueCode::ParseError);
        assert!(issue.message.contains("(line 1, column"));
    }

    #[test]
    fn table_paths_follow_arrays_and_sub_tables() {
        let text =
            "top = 1\n[engine]\nname = \"t\"\n[[sinks]]\n[[sinks]]\n[sinks.restart]\nx = 1\n";
        let at = |needle: &str| text.find(needle).unwrap_or(0);
        assert_eq!(table_path_at(text, 0), None);
        assert_eq!(table_path_at(text, at("name")), Some("engine".to_string()));
        assert_eq!(
            table_path_at(text, at("[[sinks]]")),
            Some("sinks[0]".to_string())
        );
        assert_eq!(
            table_path_at(text, at("x = 1")),
            Some("sinks[1].restart".to_string())
        );
    }

    #[test]
    fn line_and_column_are_one_based() {
        assert_eq!(line_and_column("abc", 0), (1, 1));
        assert_eq!(line_and_column("ab\ncd", 4), (2, 2));
    }

    #[test]
    fn the_report_serializes_to_the_documented_shape() -> Result<(), serde_json::Error> {
        let report = ValidationReport::failed(
            ConfigIssue::new(IssueCode::UnknownField, "unknown field `publishes`")
                .at("sinks[0].publishes"),
        );
        let json = serde_json::to_value(&report)?;
        assert_eq!(json["ok"], false);
        assert_eq!(json["engine_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["errors"][0]["code"], "unknown_field");
        assert_eq!(json["errors"][0]["path"], "sinks[0].publishes");
        assert_eq!(json["warnings"], serde_json::json!([]));
        assert_eq!(report.exit_code(), 1);

        let ok = ValidationReport::from_findings(Findings::default());
        let json = serde_json::to_value(&ok)?;
        assert_eq!(json["ok"], true);
        assert_eq!(json["errors"], serde_json::json!([]));
        assert_eq!(ok.exit_code(), 0);
        Ok(())
    }

    #[test]
    fn an_issue_without_a_location_omits_path() -> Result<(), serde_json::Error> {
        let json = serde_json::to_value(ConfigIssue::new(IssueCode::ReadError, "gone"))?;
        assert!(json.get("path").is_none());
        Ok(())
    }
}
