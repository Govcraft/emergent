//! CLI commands and output formatting for the marketplace.

use clap::{Args, Subcommand};
use dialoguer::Confirm;
use std::io::IsTerminal;

use super::error::Result;
use super::installer::{InstallOptions, Installer};
use super::platform::TargetPlatform;
use super::registry::Registry;
use super::storage::MarketplaceStorage;

/// Marketplace subcommand arguments.
#[derive(Args, Debug, Clone)]
pub struct MarketplaceArgs {
    #[command(subcommand)]
    pub command: MarketplaceCommand,
}

/// Marketplace subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum MarketplaceCommand {
    /// List available or installed primitives
    List {
        /// Filter by primitive kind
        #[arg(short, long, value_name = "KIND")]
        kind: Option<String>,

        /// Show only installed primitives
        #[arg(short, long)]
        installed: bool,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Columns to display (comma-separated)
        #[arg(short, long, value_name = "COLS")]
        columns: Option<String>,
    },

    /// Install a primitive
    Install {
        /// Name of the primitive
        name: String,

        /// Version to install (default: latest)
        #[arg(short, long, value_name = "VERSION")]
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

    /// Remove an installed primitive
    Remove {
        /// Name of the primitive
        name: String,

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
            name,
            version,
            force,
            dry_run,
            yes,
        } => {
            handle_install(&installer, name, version, force, dry_run, yes).await?;
        }
        MarketplaceCommand::Remove { name, yes } => {
            handle_remove(&installer, name, yes).await?;
        }
        MarketplaceCommand::Update { name, dry_run } => {
            handle_update(&installer, name, dry_run).await?;
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
    name: String,
    version: Option<String>,
    force: bool,
    dry_run: bool,
    yes: bool,
) -> Result<()> {
    if !yes && !dry_run && is_tty() {
        let version_str = version
            .as_ref()
            .map(|v| format!(" (v{v})"))
            .unwrap_or_else(|| " (latest)".to_string());
        let prompt = format!("Install {name}{version_str}?");
        if !confirm(&prompt)? {
            eprintln!("Installation cancelled");
            return Ok(());
        }
    }

    let options = InstallOptions {
        name,
        version,
        force,
        dry_run,
    };

    installer.install(options).await?;
    Ok(())
}

async fn handle_remove(installer: &Installer, name: String, yes: bool) -> Result<()> {
    if !yes && is_tty() {
        let prompt = format!("Remove {name}?");
        if !confirm(&prompt)? {
            eprintln!("Removal cancelled");
            return Ok(());
        }
    }

    installer.remove(&name).await?;
    Ok(())
}

async fn handle_update(
    installer: &Installer,
    name: Option<String>,
    dry_run: bool,
) -> Result<()> {
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
        eprintln!("Name: {}", manifest.primitive.name);
        eprintln!("Version: {}", manifest.primitive.version);
        eprintln!("Kind: {}", manifest.primitive.kind);
        eprintln!();
        if !manifest.messages.publishes.is_empty() {
            eprintln!("Publishes:");
            for msg in &manifest.messages.publishes {
                eprintln!("  - {msg}");
            }
        }
        if !manifest.messages.subscribes.is_empty() {
            eprintln!("Subscribes:");
            for msg in &manifest.messages.subscribes {
                eprintln!("  - {msg}");
            }
        }
        if !manifest.args.is_empty() {
            eprintln!();
            eprintln!("Arguments:");
            for arg in &manifest.args {
                let required = if arg.required { " (required)" } else { "" };
                eprintln!("  --{}{}", arg.long, required);
                if !arg.env.is_empty() {
                    eprintln!("    env: {}", arg.env);
                }
            }
        }
        eprintln!();
        eprintln!("Platforms:");
        for platform in manifest.binaries.targets.keys() {
            eprintln!("  - {platform}");
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
}
