//! Self-update command for the Emergent engine.
//!
//! Downloads the latest release binary from GitHub and replaces the current
//! executable. Uses the same download/extract infrastructure as the marketplace.

use anyhow::{Context, Result, bail};
use clap::Args;
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use std::io::IsTerminal;
use std::path::Path;

use crate::marketplace::platform::TargetPlatform;

/// GitHub repository for release downloads.
const GITHUB_REPO: &str = "Govcraft/emergent";

/// Update subcommand arguments.
#[derive(Args, Debug, Clone)]
#[command(after_long_help = "\
Examples:
  # Check for and install the latest version
  emergent update

  # Preview without installing
  emergent update --dry-run

  # Force reinstall even if already on latest
  emergent update --force
")]
pub struct UpdateArgs {
    /// Force update even if already on latest version.
    #[arg(short, long)]
    pub force: bool,

    /// Preview without installing.
    #[arg(long)]
    pub dry_run: bool,
}

/// GitHub release API response (minimal fields).
#[derive(Debug, serde::Deserialize)]
struct GitHubRelease {
    tag_name: String,
}

/// Fetch the latest release version from GitHub.
async fn fetch_latest_version() -> Result<String> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let client = reqwest::Client::builder()
        .user_agent("emergent-updater")
        .build()
        .context("Failed to create HTTP client")?;

    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to fetch latest release from GitHub")?;

    if !response.status().is_success() {
        bail!("GitHub API returned HTTP {}: {}", response.status(), url);
    }

    let release: GitHubRelease = response
        .json()
        .await
        .context("Failed to parse GitHub release response")?;

    // Strip "v" prefix from tag name (e.g., "v0.10.1" → "0.10.1")
    let version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name)
        .to_string();

    Ok(version)
}

/// Download a file with a progress bar.
async fn download(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("emergent-updater")
        .build()
        .context("Failed to create HTTP client")?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to download {url}"))?;

    if !response.status().is_success() {
        bail!("HTTP {} when downloading {}", response.status(), url);
    }

    let total_size = response.content_length().unwrap_or(0);
    let pb = if total_size > 0 && std::io::stderr().is_terminal() {
        let bar = ProgressBar::new(total_size);
        bar.set_style(
            ProgressStyle::default_bar()
                .template("{bar:40.cyan/blue} {bytes}/{total_bytes} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=>-"),
        );
        Some(bar)
    } else {
        None
    };

    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("Failed to read response from {url}"))?;

    if let Some(ref bar) = pb {
        bar.inc(bytes.len() as u64);
        bar.finish_with_message("Downloaded");
    }

    std::fs::write(dest, bytes).context("Failed to write downloaded file")?;
    Ok(())
}

/// Extract a tar.gz archive to a destination directory.
fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)
        .with_context(|| format!("Failed to open archive: {}", archive.display()))?;
    let decoder = GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    tar.unpack(dest)
        .with_context(|| format!("Failed to extract archive: {}", archive.display()))?;
    Ok(())
}

/// Execute the update command.
pub async fn execute(args: UpdateArgs) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    eprintln!("Current version: {current_version}");

    // Fetch latest release version
    eprint!("Checking for updates... ");
    let latest_version = fetch_latest_version().await?;
    eprintln!("latest is {latest_version}");

    // Compare versions
    if current_version == latest_version && !args.force {
        eprintln!("Already up to date.");
        return Ok(());
    }

    if current_version == latest_version && args.force {
        eprintln!("Already on latest, but --force specified.");
    }

    // Detect platform
    let platform = TargetPlatform::detect();
    let archive_name = format!("emergent-{latest_version}-{platform}.tar.gz");
    let download_url = format!(
        "https://github.com/{GITHUB_REPO}/releases/download/v{latest_version}/{archive_name}"
    );

    // Get current binary path
    let current_exe =
        std::env::current_exe().context("Failed to determine current executable path")?;

    if args.dry_run {
        eprintln!("Would update: {current_version} → {latest_version}");
        eprintln!("Platform: {platform}");
        eprintln!("Download: {download_url}");
        eprintln!("Binary: {}", current_exe.display());
        return Ok(());
    }

    eprintln!("Updating {current_version} → {latest_version}...");

    // Download to temp file
    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let archive_path = temp_dir.path().join(&archive_name);

    eprintln!("Downloading {archive_name}...");
    download(&download_url, &archive_path).await?;

    // Extract
    eprintln!("Extracting...");
    extract_tar_gz(&archive_path, temp_dir.path())?;

    // Find the extracted binary
    let new_binary = temp_dir.path().join("emergent");
    if !new_binary.exists() {
        bail!(
            "Expected binary 'emergent' not found in archive. Contents: {:?}",
            std::fs::read_dir(temp_dir.path())?
                .filter_map(|e| e.ok().map(|e| e.file_name()))
                .collect::<Vec<_>>()
        );
    }

    // Set executable permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&new_binary, perms)
            .context("Failed to set executable permissions")?;
    }

    // Replace current binary: backup → copy → remove backup
    let backup_path = current_exe.with_extension("bak");

    // Backup current binary
    std::fs::rename(&current_exe, &backup_path).with_context(|| {
        format!(
            "Failed to backup current binary to {}",
            backup_path.display()
        )
    })?;

    // Copy new binary into place
    if let Err(e) = std::fs::copy(&new_binary, &current_exe) {
        // Restore from backup on failure
        let _ = std::fs::rename(&backup_path, &current_exe);
        return Err(e).context("Failed to install new binary (restored backup)");
    }

    // Set permissions on the installed binary
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        let _ = std::fs::set_permissions(&current_exe, perms);
    }

    // Remove backup
    let _ = std::fs::remove_file(&backup_path);

    eprintln!("Successfully updated emergent to {latest_version}");
    Ok(())
}
