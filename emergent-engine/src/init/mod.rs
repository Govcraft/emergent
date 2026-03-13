//! Init command - Generate a starter emergent.toml configuration file.
//!
//! Creates a well-commented configuration file with sensible defaults,
//! giving new users a working starting point for their event-driven workflows.
//!
//! # Usage
//!
//! ```bash
//! # Create default config
//! emergent init
//!
//! # Custom engine name
//! emergent init --name my-pipeline
//!
//! # Preview without writing
//! emergent init --dry-run
//! ```

pub mod cli;
pub mod template;

pub use cli::InitArgs;

use std::io::IsTerminal;

/// Execute the init command with the given arguments.
///
/// Generates and writes a starter `emergent.toml` configuration file.
///
/// # Errors
///
/// Returns an error if:
/// - The output file already exists and `--force` is not set (non-TTY)
/// - The user declines the overwrite prompt (TTY)
/// - Template rendering fails
/// - File writing fails
pub async fn execute(args: InitArgs) -> anyhow::Result<()> {
    // Check if output file already exists
    if args.output.exists() && !args.force && !args.dry_run {
        if std::io::stdout().is_terminal() {
            let prompt = format!("{} already exists. Overwrite?", args.output.display());
            let confirmed = dialoguer::Confirm::new()
                .with_prompt(prompt)
                .default(false)
                .interact()
                .map_err(|e| anyhow::anyhow!("Prompt error: {e}"))?;
            if !confirmed {
                anyhow::bail!("Aborted");
            }
        } else {
            anyhow::bail!(
                "{} already exists. Use --force to overwrite.",
                args.output.display()
            );
        }
    }

    // Resolve engine name: explicit flag > interactive prompt > default
    let name = if let Some(name) = args.name {
        name
    } else if std::io::stdin().is_terminal() {
        dialoguer::Input::<String>::new()
            .with_prompt("Engine name")
            .default("emergent".to_string())
            .interact_text()
            .map_err(|e| anyhow::anyhow!("Prompt error: {e}"))?
    } else {
        "emergent".to_string()
    };

    // Render the template
    let content = template::render_config_template(&name)
        .map_err(|e| anyhow::anyhow!("Template error: {e}"))?;

    // Dry-run: print to stdout and return
    if args.dry_run {
        print!("{content}");
        return Ok(());
    }

    // Write the file
    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.output, &content)?;

    eprintln!("Created {}", args.output.display());
    eprintln!();
    eprintln!("Next steps:");
    eprintln!("  1. Add primitives to your config:");
    eprintln!("     emergent scaffold");
    eprintln!("  2. Start the engine:");
    eprintln!("     emergent --config {}", args.output.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_execute_creates_file() -> anyhow::Result<()> {
        let dir = TempDir::new()?;
        let output = dir.path().join("emergent.toml");

        let args = InitArgs {
            name: Some("test-engine".to_string()),
            output: output.clone(),
            force: false,
            dry_run: false,
        };

        execute(args).await?;
        assert!(output.exists(), "Config file should be created");

        let content = std::fs::read_to_string(&output)?;
        assert!(
            content.contains("name = \"test-engine\""),
            "File should contain the engine name"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_dry_run_no_file() -> anyhow::Result<()> {
        let dir = TempDir::new()?;
        let output = dir.path().join("emergent.toml");

        let args = InitArgs {
            name: Some("emergent".to_string()),
            output: output.clone(),
            force: false,
            dry_run: true,
        };

        execute(args).await?;
        assert!(!output.exists(), "File should not be created in dry-run");
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_refuses_overwrite() -> anyhow::Result<()> {
        let dir = TempDir::new()?;
        let output = dir.path().join("emergent.toml");

        // Create an existing file
        std::fs::write(&output, "existing content")?;

        let args = InitArgs {
            name: None,
            output: output.clone(),
            force: false,
            dry_run: false,
        };

        // In non-TTY (test environment), should error
        let Err(err) = execute(args).await else {
            panic!("Should refuse to overwrite without --force");
        };
        let err = format!("{err}");
        assert!(
            err.contains("already exists"),
            "Error should mention file exists, got: {err}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_force_overwrites() -> anyhow::Result<()> {
        let dir = TempDir::new()?;
        let output = dir.path().join("emergent.toml");

        // Create an existing file
        std::fs::write(&output, "old content")?;

        let args = InitArgs {
            name: Some("new-engine".to_string()),
            output: output.clone(),
            force: true,
            dry_run: false,
        };

        execute(args).await?;
        let content = std::fs::read_to_string(&output)?;
        assert!(
            content.contains("name = \"new-engine\""),
            "File should contain the new engine name"
        );
        assert!(
            !content.contains("old content"),
            "Old content should be replaced"
        );
        Ok(())
    }
}
