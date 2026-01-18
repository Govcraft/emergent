//! CLI argument parsing and interactive wizard for scaffold command.
//!
//! Supports both flag-based invocation (for scripting) and interactive wizard
//! mode (for ease of use).

use std::path::PathBuf;

use clap::Args;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use heck::{ToSnakeCase, ToUpperCamelCase};

use crate::scaffold::messages::{Language, PrimitiveType, ScaffoldRequest, TemplateContext};

/// Scaffold subcommand arguments.
#[derive(Args, Debug, Clone)]
pub struct ScaffoldArgs {
    /// Language for code generation.
    #[arg(short, long, value_name = "LANG")]
    pub language: Option<String>,

    /// Type of primitive to scaffold.
    #[arg(short = 't', long, value_name = "TYPE")]
    pub primitive_type: Option<String>,

    /// Name of the primitive (snake_case).
    #[arg(short, long, value_name = "NAME")]
    pub name: Option<String>,

    /// Message types to subscribe to (comma-separated).
    /// Only for handlers and sinks.
    #[arg(short = 'S', long, value_name = "TYPES")]
    pub subscribes: Option<String>,

    /// Message types to publish (comma-separated).
    /// Only for sources and handlers.
    #[arg(short, long, value_name = "TYPES")]
    pub publishes: Option<String>,

    /// Output directory (default: ./<name>).
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<PathBuf>,

    /// Description for the primitive.
    #[arg(short, long, value_name = "DESC")]
    pub description: Option<String>,

    /// Preview files without writing (dry run).
    #[arg(long)]
    pub dry_run: bool,

    /// Output JSON for scripting.
    #[arg(long)]
    pub json: bool,
}

/// Error type for scaffold operations.
#[derive(Debug)]
pub enum ScaffoldError {
    /// User cancelled the operation.
    Cancelled,
    /// Invalid input provided.
    InvalidInput(String),
    /// IO error during prompting.
    IoError(std::io::Error),
    /// Dialoguer error during prompting.
    DialoguerError(String),
    /// Template rendering error.
    TemplateError(String),
    /// File write error.
    WriteError(String),
}

impl std::fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "Operation cancelled by user"),
            Self::InvalidInput(msg) => write!(f, "Invalid input: {msg}"),
            Self::IoError(e) => write!(f, "IO error: {e}"),
            Self::DialoguerError(msg) => write!(f, "Prompt error: {msg}"),
            Self::TemplateError(msg) => write!(f, "Template error: {msg}"),
            Self::WriteError(msg) => write!(f, "Write error: {msg}"),
        }
    }
}

impl std::error::Error for ScaffoldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IoError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ScaffoldError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
    }
}

impl From<dialoguer::Error> for ScaffoldError {
    fn from(e: dialoguer::Error) -> Self {
        Self::DialoguerError(e.to_string())
    }
}

/// Parse comma-separated string into a vector of strings.
fn parse_comma_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Validate that a name is valid snake_case.
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Name cannot be empty".to_string());
    }

    // Check if it's valid snake_case
    let snake = name.to_snake_case();
    if snake != name {
        return Err(format!("Name should be snake_case: suggested '{snake}'"));
    }

    // Check for invalid characters
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("Name can only contain alphanumeric characters and underscores".to_string());
    }

    // Check that it doesn't start with a number
    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return Err("Name cannot start with a number".to_string());
    }

    Ok(())
}

/// Run the interactive wizard to collect scaffold parameters.
///
/// Returns a `ScaffoldRequest` with all necessary information.
pub fn run_wizard(args: &ScaffoldArgs) -> Result<ScaffoldRequest, ScaffoldError> {
    let theme = ColorfulTheme::default();

    // Language selection
    let language = if let Some(ref lang_str) = args.language {
        lang_str.parse::<Language>().map_err(ScaffoldError::InvalidInput)?
    } else {
        let languages = ["Rust", "TypeScript (Deno)", "Python"];
        let selection = Select::with_theme(&theme)
            .with_prompt("Select language")
            .items(&languages)
            .default(0)
            .interact()?;

        match selection {
            0 => Language::Rust,
            1 => Language::TypeScript,
            2 => Language::Python,
            _ => Language::Rust,
        }
    };

    // Primitive type selection
    let primitive_type = if let Some(ref type_str) = args.primitive_type {
        type_str.parse::<PrimitiveType>().map_err(ScaffoldError::InvalidInput)?
    } else {
        let types = ["Source (emits events)", "Handler (transforms events)", "Sink (consumes events)"];
        let selection = Select::with_theme(&theme)
            .with_prompt("Select primitive type")
            .items(&types)
            .default(0)
            .interact()?;

        match selection {
            0 => PrimitiveType::Source,
            1 => PrimitiveType::Handler,
            2 => PrimitiveType::Sink,
            _ => PrimitiveType::Source,
        }
    };

    // Name
    let name = if let Some(ref name_str) = args.name {
        validate_name(name_str).map_err(ScaffoldError::InvalidInput)?;
        name_str.clone()
    } else {
        let default_name = format!("my_{}", primitive_type.as_str());
        Input::with_theme(&theme)
            .with_prompt("Primitive name (snake_case)")
            .default(default_name)
            .validate_with(|input: &String| validate_name(input))
            .interact_text()?
    };

    // Description
    let description = if let Some(ref desc) = args.description {
        desc.clone()
    } else {
        let default_desc = format!("A {} that {}",
            primitive_type.as_str(),
            match primitive_type {
                PrimitiveType::Source => "emits events",
                PrimitiveType::Handler => "transforms events",
                PrimitiveType::Sink => "consumes events",
            }
        );
        Input::with_theme(&theme)
            .with_prompt("Description")
            .default(default_desc)
            .interact_text()?
    };

    // Subscribes (for handlers and sinks)
    let subscribes = if matches!(primitive_type, PrimitiveType::Handler | PrimitiveType::Sink) {
        if let Some(ref subs) = args.subscribes {
            parse_comma_list(subs)
        } else {
            let input: String = Input::with_theme(&theme)
                .with_prompt("Message types to subscribe (comma-separated)")
                .default("timer.tick".to_string())
                .interact_text()?;
            parse_comma_list(&input)
        }
    } else {
        Vec::new()
    };

    // Publishes (for sources and handlers)
    let publishes = if matches!(primitive_type, PrimitiveType::Source | PrimitiveType::Handler) {
        if let Some(ref pubs) = args.publishes {
            parse_comma_list(pubs)
        } else {
            let default_pub = format!("{}.output", name);
            let input: String = Input::with_theme(&theme)
                .with_prompt("Message types to publish (comma-separated)")
                .default(default_pub)
                .interact_text()?;
            parse_comma_list(&input)
        }
    } else {
        Vec::new()
    };

    // Output directory
    let output_dir = if let Some(ref output) = args.output {
        output.clone()
    } else {
        let default_output = PathBuf::from(format!("./{name}"));
        Input::with_theme(&theme)
            .with_prompt("Output directory")
            .default(default_output.display().to_string())
            .interact_text()
            .map(PathBuf::from)?
    };

    Ok(ScaffoldRequest {
        language,
        name,
        primitive_type,
        subscribes,
        publishes,
        description,
        output_dir,
        dry_run: args.dry_run,
        json_output: args.json,
    })
}

/// Check if all required arguments are provided (non-interactive mode).
pub fn has_required_args(args: &ScaffoldArgs) -> bool {
    args.primitive_type.is_some() && args.name.is_some()
}

/// Build a scaffold request from command-line arguments only (no prompting).
pub fn build_from_args(args: &ScaffoldArgs) -> Result<ScaffoldRequest, ScaffoldError> {
    let language = args.language
        .as_ref()
        .map(|s| s.parse::<Language>())
        .transpose()
        .map_err(ScaffoldError::InvalidInput)?
        .unwrap_or_default();

    let primitive_type = args.primitive_type
        .as_ref()
        .ok_or_else(|| ScaffoldError::InvalidInput("--type is required".to_string()))?
        .parse::<PrimitiveType>()
        .map_err(ScaffoldError::InvalidInput)?;

    let name = args.name
        .as_ref()
        .ok_or_else(|| ScaffoldError::InvalidInput("--name is required".to_string()))?
        .clone();

    validate_name(&name).map_err(ScaffoldError::InvalidInput)?;

    let subscribes = args.subscribes
        .as_ref()
        .map(|s| parse_comma_list(s))
        .unwrap_or_default();

    let publishes = args.publishes
        .as_ref()
        .map(|s| parse_comma_list(s))
        .unwrap_or_default();

    let description = args.description
        .clone()
        .unwrap_or_else(|| format!("A {} primitive", primitive_type.as_str()));

    let output_dir = args.output
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("./{name}")));

    Ok(ScaffoldRequest {
        language,
        name,
        primitive_type,
        subscribes,
        publishes,
        description,
        output_dir,
        dry_run: args.dry_run,
        json_output: args.json,
    })
}

/// Build template context from a scaffold request.
pub fn build_template_context(request: &ScaffoldRequest) -> TemplateContext {
    let name_snake = request.name.to_snake_case();
    let name_pascal = request.name.to_upper_camel_case();

    TemplateContext {
        name: request.name.clone(),
        name_snake,
        name_pascal,
        primitive_type: request.primitive_type.as_str().to_string(),
        subscribes: request.subscribes.clone(),
        publishes: request.publishes.clone(),
        description: request.description.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_name_valid() {
        assert!(validate_name("my_source").is_ok());
        assert!(validate_name("timer_handler").is_ok());
        assert!(validate_name("console_sink").is_ok());
    }

    #[test]
    fn test_validate_name_invalid() {
        assert!(validate_name("").is_err());
        assert!(validate_name("MySource").is_err()); // not snake_case
        assert!(validate_name("my-source").is_err()); // has hyphen
        assert!(validate_name("1source").is_err()); // starts with number
    }

    #[test]
    fn test_parse_comma_list() {
        assert_eq!(
            parse_comma_list("timer.tick, filter.processed"),
            vec!["timer.tick", "filter.processed"]
        );
        assert_eq!(
            parse_comma_list("single"),
            vec!["single"]
        );
        assert!(parse_comma_list("").is_empty());
    }

    #[test]
    fn test_build_template_context() {
        let request = ScaffoldRequest {
            language: Language::Rust,
            name: "my_handler".to_string(),
            primitive_type: PrimitiveType::Handler,
            subscribes: vec!["timer.tick".to_string()],
            publishes: vec!["timer.filtered".to_string()],
            description: "A test handler".to_string(),
            output_dir: PathBuf::from("./my_handler"),
            dry_run: false,
            json_output: false,
        };

        let context = build_template_context(&request);
        assert_eq!(context.name, "my_handler");
        assert_eq!(context.name_snake, "my_handler");
        assert_eq!(context.name_pascal, "MyHandler");
        assert_eq!(context.primitive_type, "handler");
    }
}
