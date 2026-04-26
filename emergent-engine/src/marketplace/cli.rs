//! CLI commands and output formatting for the marketplace.

use clap::{Args, Subcommand};
use dialoguer::Confirm;
use std::io::IsTerminal;

use super::error::{MarketplaceError, Result};
use super::installer::{InstallOptions, Installer};
use super::platform::TargetPlatform;
use super::registry::Registry;
use super::storage::MarketplaceStorage;

/// Marketplace subcommand arguments.
#[derive(Args, Debug, Clone)]
#[command(after_long_help = "\
Examples:
  # List all available primitives
  emergent marketplace list

  # List only sources
  emergent marketplace list --kind source

  # Search for a primitive
  emergent marketplace search slack

  # Install a primitive
  emergent marketplace install http-source

  # Install multiple primitives at once
  emergent marketplace install exec-source exec-handler exec-sink

  # Install a specific version (single primitive only)
  emergent marketplace install http-source --version 0.5.0

  # Show details about a primitive
  emergent marketplace info http-source

  # Update all installed primitives
  emergent marketplace update

  # Remove one or more primitives
  emergent marketplace remove http-source
  emergent marketplace remove exec-source exec-handler exec-sink
")]
pub struct MarketplaceArgs {
    #[command(subcommand)]
    pub command: MarketplaceCommand,
}

/// Marketplace subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum MarketplaceCommand {
    /// List available or installed primitives
    List {
        /// Filter by primitive kind [possible values: source, handler, sink]
        #[arg(short, long, value_name = "KIND")]
        kind: Option<String>,

        /// Show only installed primitives
        #[arg(short, long)]
        installed: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Columns to display (comma-separated)
        #[arg(long, value_name = "COLS")]
        columns: Option<String>,
    },

    /// Install one or more primitives
    Install {
        /// Names of primitives to install
        #[arg(value_name = "NAME", num_args = 1.., required = true)]
        names: Vec<String>,

        /// Version to install (single primitive only; default: latest)
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,

        /// Force reinstall if already installed
        #[arg(short, long)]
        force: bool,

        /// Preview without installing
        #[arg(long)]
        dry_run: bool,

        /// Skip confirmation prompts
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Remove one or more installed primitives
    Remove {
        /// Names of primitives to remove
        #[arg(value_name = "NAME", num_args = 1.., required = true)]
        names: Vec<String>,

        /// Skip confirmation prompts
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Update installed primitive(s)
    Update {
        /// Name of the primitive (default: all)
        name: Option<String>,

        /// Preview without updating
        #[arg(long)]
        dry_run: bool,
    },

    /// Show detailed information about a primitive
    Info {
        /// Name of the primitive
        name: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Search for primitives
    Search {
        /// Search query
        query: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

/// Execute the marketplace command.
pub async fn execute(args: MarketplaceArgs) -> Result<()> {
    let storage = MarketplaceStorage::new()?;
    let config = storage.load_config()?;
    let registry = Registry::new(storage.cache_dir().to_path_buf(), config.registry_url);
    let platform = TargetPlatform::detect();
    let installer = Installer::new(storage, registry.clone(), platform.clone());

    match args.command {
        MarketplaceCommand::List {
            kind,
            installed,
            json,
            columns,
        } => {
            handle_list(&installer, &registry, kind, installed, json, columns).await?;
        }
        MarketplaceCommand::Install {
            names,
            version,
            force,
            dry_run,
            yes,
        } => {
            handle_install(&installer, names, version, force, dry_run, yes).await?;
        }
        MarketplaceCommand::Remove { names, yes } => {
            handle_remove(&installer, names, yes).await?;
        }
        MarketplaceCommand::Update { name, dry_run } => {
            handle_update(&installer, &registry, name, dry_run).await?;
        }
        MarketplaceCommand::Info { name, json } => {
            handle_info(&registry, name, json).await?;
        }
        MarketplaceCommand::Search { query, json } => {
            handle_search(&registry, query, json).await?;
        }
    }

    Ok(())
}

async fn handle_list(
    installer: &Installer,
    registry: &Registry,
    kind: Option<String>,
    installed: bool,
    json: bool,
    _columns: Option<String>,
) -> Result<()> {
    if installed {
        let primitives = installer.list_installed()?;
        if json {
            let json_str = format_json(&primitives)?;
            println!("{json_str}");
        } else {
            let headers = &["NAME", "VERSION", "KIND", "INSTALLED"];
            let rows: Vec<Vec<String>> = primitives
                .iter()
                .map(|p| {
                    vec![
                        p.name.clone(),
                        p.version.clone(),
                        p.kind.clone(),
                        p.installed_at.clone(),
                    ]
                })
                .collect();
            format_table(headers, rows);
        }
    } else {
        let index = registry.fetch_index().await?;
        let filtered = if let Some(k) = kind {
            registry.filter_by_kind(&index, &k)
        } else {
            index.primitives.iter().collect()
        };

        if json {
            let json_str = format_json(&filtered)?;
            println!("{json_str}");
        } else {
            let headers = &["NAME", "VERSION", "KIND", "DESCRIPTION"];
            let rows: Vec<Vec<String>> = filtered
                .iter()
                .map(|p| {
                    vec![
                        p.name.clone(),
                        p.version.clone(),
                        p.kind.clone(),
                        p.description.clone(),
                    ]
                })
                .collect();
            format_table(headers, rows);
        }
    }

    Ok(())
}

async fn handle_install(
    installer: &Installer,
    names: Vec<String>,
    version: Option<String>,
    force: bool,
    dry_run: bool,
    yes: bool,
) -> Result<()> {
    if names.is_empty() {
        return Err(MarketplaceError::InvalidManifest {
            reason: "at least one primitive name is required".to_string(),
        });
    }

    if names.len() > 1 && version.is_some() {
        return Err(MarketplaceError::InvalidManifest {
            reason: "--version can only be used when installing a single primitive".to_string(),
        });
    }

    if !yes && !dry_run && is_tty() {
        let version_str = version
            .as_ref()
            .map(|v| format!(" (v{v})"))
            .unwrap_or_else(|| " (latest)".to_string());
        let prompt = format!("Install {}{}?", names.join(", "), version_str);
        if !confirm(&prompt)? {
            eprintln!("Installation cancelled");
            return Ok(());
        }
    }

    let total = names.len();
    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for name in names {
        let options = InstallOptions {
            name: name.clone(),
            version: version.clone(),
            force,
            dry_run,
        };

        match installer.install(options).await {
            Ok(_) => succeeded += 1,
            Err(e) => {
                eprintln!("✗ {name}: {e}");
                failed += 1;
            }
        }
    }

    if total > 1 {
        eprintln!("Installed {succeeded} of {total} primitives");
    }

    if failed > 0 {
        return Err(MarketplaceError::BatchFailed { succeeded, failed });
    }
    Ok(())
}

async fn handle_remove(installer: &Installer, names: Vec<String>, yes: bool) -> Result<()> {
    if names.is_empty() {
        return Err(MarketplaceError::InvalidManifest {
            reason: "at least one primitive name is required".to_string(),
        });
    }

    if !yes && is_tty() {
        let prompt = format!("Remove {}?", names.join(", "));
        if !confirm(&prompt)? {
            eprintln!("Removal cancelled");
            return Ok(());
        }
    }

    let total = names.len();
    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for name in &names {
        match installer.remove(name).await {
            Ok(_) => succeeded += 1,
            Err(e) => {
                eprintln!("✗ {name}: {e}");
                failed += 1;
            }
        }
    }

    if total > 1 {
        eprintln!("Removed {succeeded} of {total} primitives");
    }

    if failed > 0 {
        return Err(MarketplaceError::BatchFailed { succeeded, failed });
    }
    Ok(())
}

async fn handle_update(
    installer: &Installer,
    registry: &Registry,
    name: Option<String>,
    dry_run: bool,
) -> Result<()> {
    // Refresh the registry once before checking versions
    let _ = registry.fetch_index().await;

    if let Some(name) = name {
        installer.update(&name, dry_run).await?;
    } else {
        let installed = installer.list_installed()?;
        for primitive in installed {
            installer.update(&primitive.name, dry_run).await?;
        }
    }

    Ok(())
}

async fn handle_info(registry: &Registry, name: String, json: bool) -> Result<()> {
    let manifest = registry.get_manifest(&name).await?;

    if json {
        let json_str = format_json(&manifest)?;
        println!("{json_str}");
    } else {
        println!("Name: {}", manifest.primitive.name);
        println!("Version: {}", manifest.primitive.version);
        println!("Kind: {}", manifest.primitive.kind);
        println!();
        if !manifest.messages.publishes.is_empty() {
            println!("Publishes:");
            for msg in &manifest.messages.publishes {
                println!("  - {msg}");
            }
        }
        if !manifest.messages.subscribes.is_empty() {
            println!("Subscribes:");
            for msg in &manifest.messages.subscribes {
                println!("  - {msg}");
            }
        }
        if !manifest.args.is_empty() {
            println!();
            println!("Arguments:");
            for arg in &manifest.args {
                let required = if arg.required { " (required)" } else { "" };
                println!("  --{}{}", arg.long, required);
                if !arg.env.is_empty() {
                    println!("    env: {}", arg.env);
                }
            }
        }
        println!();
        println!("Platforms:");
        for platform in manifest.binaries.targets.keys() {
            println!("  - {platform}");
        }
    }

    Ok(())
}

async fn handle_search(registry: &Registry, query: String, json: bool) -> Result<()> {
    let index = registry.fetch_index().await?;
    let results = registry.search(&index, &query);

    if json {
        let json_str = format_json(&results)?;
        println!("{json_str}");
    } else {
        let headers = &["NAME", "VERSION", "KIND", "DESCRIPTION"];
        let rows: Vec<Vec<String>> = results
            .iter()
            .map(|p| {
                vec![
                    p.name.clone(),
                    p.version.clone(),
                    p.kind.clone(),
                    p.description.clone(),
                ]
            })
            .collect();
        format_table(headers, rows);
    }

    Ok(())
}

/// Check if stdout is a TTY.
fn is_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// Prompt user for confirmation.
fn confirm(prompt: &str) -> Result<bool> {
    let result = Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .interact()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(result)
}

/// Format and print a table to stdout.
fn format_table(headers: &[&str], rows: Vec<Vec<String>>) {
    if rows.is_empty() {
        eprintln!("No items found");
        return;
    }

    // Calculate column widths
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    // Print header
    let header_line: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| format!("{:width$}", h, width = widths[i]))
        .collect();
    println!("{}", header_line.join("  "));

    // Print separator
    let separator: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    println!("{}", separator.join("  "));

    // Print rows
    for row in rows {
        let formatted_row: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let width = if i < widths.len() { widths[i] } else { 0 };
                format!("{:width$}", cell, width = width)
            })
            .collect();
        println!("{}", formatted_row.join("  "));
    }
}

/// Format data as JSON.
fn format_json<T: serde::Serialize>(data: &T) -> Result<String> {
    let json = serde_json::to_string_pretty(data)?;
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_json() {
        #[derive(serde::Serialize)]
        struct TestData {
            name: String,
            value: i32,
        }

        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        if let Ok(json) = format_json(&data) {
            assert!(json.contains("\"name\""));
            assert!(json.contains("\"test\""));
            assert!(json.contains("\"value\""));
            assert!(json.contains("42"));
        } else {
            panic!("Should serialize to JSON");
        }
    }

    #[test]
    fn test_format_table_empty() {
        // This test verifies format_table doesn't panic with empty rows
        let headers = &["NAME", "VALUE"];
        let rows = Vec::new();
        format_table(headers, rows);
        // Should print "No items found" to stderr
    }

    #[test]
    fn test_format_table() {
        let headers = &["NAME", "VALUE"];
        let rows = vec![
            vec!["test1".to_string(), "value1".to_string()],
            vec!["test2".to_string(), "value2".to_string()],
        ];
        format_table(headers, rows);
        // Visual test - should print formatted table to stdout
    }

    use clap::Parser;

    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(subcommand)]
        command: MarketplaceCommand,
    }

    fn parse(args: &[&str]) -> clap::error::Result<MarketplaceCommand> {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).map(|c| c.command)
    }

    #[test]
    fn test_install_parses_single_name() {
        let Ok(MarketplaceCommand::Install { names, .. }) = parse(&["install", "exec-source"])
        else {
            panic!("expected Install with single name");
        };
        assert_eq!(names, vec!["exec-source".to_string()]);
    }

    #[test]
    fn test_install_parses_multiple_names() {
        let Ok(MarketplaceCommand::Install { names, .. }) =
            parse(&["install", "exec-source", "exec-handler", "exec-sink"])
        else {
            panic!("expected Install with multiple names");
        };
        assert_eq!(
            names,
            vec![
                "exec-source".to_string(),
                "exec-handler".to_string(),
                "exec-sink".to_string(),
            ]
        );
    }

    #[test]
    fn test_install_requires_at_least_one_name() {
        assert!(parse(&["install"]).is_err());
    }

    #[test]
    fn test_remove_parses_multiple_names() {
        let Ok(MarketplaceCommand::Remove { names, yes }) =
            parse(&["remove", "a", "b", "c", "-y"])
        else {
            panic!("expected Remove with multiple names");
        };
        assert_eq!(
            names,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(yes);
    }

    #[tokio::test]
    async fn test_handle_install_rejects_version_with_multiple_names() {
        // Build an installer; we never actually call it because validation
        // fails before any network/disk work happens.
        let Ok(storage) = MarketplaceStorage::new() else {
            panic!("failed to construct MarketplaceStorage");
        };
        let registry = Registry::new(
            storage.cache_dir().to_path_buf(),
            "https://example.invalid".to_string(),
        );
        let installer = Installer::new(storage, registry, TargetPlatform::detect());

        let result = handle_install(
            &installer,
            vec!["a".to_string(), "b".to_string()],
            Some("1.0.0".to_string()),
            false,
            false,
            true,
        )
        .await;

        match result {
            Err(MarketplaceError::InvalidManifest { reason }) => {
                assert!(
                    reason.contains("--version"),
                    "expected --version in error, got: {reason}"
                );
            }
            other => panic!("expected InvalidManifest error, got {other:?}"),
        }
    }
}
