//! Message types for the scaffold workflow.
//!
//! The scaffold feature uses an internal event-driven architecture:
//! - `ScaffoldRequest` - CLI source emits this with user input
//! - `TemplateRendered` - Handler emits for each rendered file
//! - `ScaffoldComplete` - Final message indicating completion

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Language for code generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Rust,
    TypeScript,
    Python,
}

impl Language {
    /// Returns the language directory name.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Python => "python",
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for Language {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "rust" | "rs" => Ok(Self::Rust),
            "typescript" | "ts" => Ok(Self::TypeScript),
            "python" | "py" => Ok(Self::Python),
            _ => Err(format!("Unknown language: {s}. Valid options: rust, typescript, python")),
        }
    }
}

/// Type of primitive to scaffold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrimitiveType {
    Source,
    Handler,
    Sink,
}

impl PrimitiveType {
    /// Returns the primitive type as a string.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Handler => "handler",
            Self::Sink => "sink",
        }
    }
}

impl std::fmt::Display for PrimitiveType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for PrimitiveType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "source" => Ok(Self::Source),
            "handler" => Ok(Self::Handler),
            "sink" => Ok(Self::Sink),
            _ => Err(format!("Unknown primitive type: {s}. Valid options: source, handler, sink")),
        }
    }
}

/// Request to scaffold a new primitive.
///
/// Emitted by the CLI source actor after collecting user input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldRequest {
    /// Target language for code generation.
    pub language: Language,
    /// Name of the primitive (snake_case).
    pub name: String,
    /// Type of primitive to create.
    pub primitive_type: PrimitiveType,
    /// Message types this primitive subscribes to (handlers and sinks only).
    pub subscribes: Vec<String>,
    /// Message types this primitive publishes (sources and handlers only).
    pub publishes: Vec<String>,
    /// Description for the primitive.
    pub description: String,
    /// Output directory for generated files.
    pub output_dir: PathBuf,
    /// Whether this is a dry run (preview only).
    pub dry_run: bool,
    /// Whether to output JSON (for scripting).
    pub json_output: bool,
}

/// A single rendered template file.
///
/// Emitted by the template handler for each file to be written.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateRendered {
    /// Relative path within the output directory.
    pub file_path: String,
    /// Rendered file content.
    pub content: String,
    /// Index of this file (0-based).
    pub file_index: usize,
    /// Total number of files to generate.
    pub total_files: usize,
}

/// Scaffold completion result.
///
/// Emitted by the file writer sink after all files are written.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldComplete {
    /// List of files that were written (or would be written in dry-run).
    pub files_written: Vec<String>,
    /// Output directory where files were written.
    pub output_dir: PathBuf,
    /// Whether the scaffold operation succeeded.
    pub success: bool,
    /// Error message if the operation failed.
    pub error: Option<String>,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

/// Template context for rendering.
///
/// Contains all variables available to templates.
#[derive(Debug, Clone, Serialize)]
pub struct TemplateContext {
    /// Package name (snake_case).
    pub name: String,
    /// Snake case name.
    pub name_snake: String,
    /// PascalCase name.
    pub name_pascal: String,
    /// Primitive type string.
    pub primitive_type: String,
    /// Subscribe list.
    pub subscribes: Vec<String>,
    /// Publish list.
    pub publishes: Vec<String>,
    /// Description for doc comments.
    pub description: String,
}
