//! CLI argument parsing for the init command.

use std::path::PathBuf;

use clap::Args;

/// Arguments for the `emergent init` command.
///
/// Generates a well-commented starter `emergent.toml` configuration file
/// with sensible defaults.
#[derive(Args, Debug, Clone)]
#[command(after_long_help = "\
Examples:
  # Create a default emergent.toml in the current directory
  emergent init

  # Initialize with a custom engine name
  emergent init --name my-pipeline

  # Write to a custom path
  emergent init --output ./config/emergent.toml

  # Preview the generated config without writing
  emergent init --dry-run

  # Pipe the generated config to a file
  emergent init --dry-run > custom.toml

  # Overwrite an existing config file
  emergent init --force
")]
pub struct InitArgs {
    /// Engine instance name
    #[arg(short, long)]
    pub name: Option<String>,

    /// Output file path
    #[arg(short, long, default_value = "./emergent.toml")]
    pub output: PathBuf,

    /// Overwrite existing file without prompting
    #[arg(long)]
    pub force: bool,

    /// Preview the generated config without writing to disk
    #[arg(long)]
    pub dry_run: bool,
}
