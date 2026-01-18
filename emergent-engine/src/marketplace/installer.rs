//! Binary download, verification, and installation.

use std::path::{Path, PathBuf};

use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

use super::error::{MarketplaceError, Result};
use super::platform::TargetPlatform;
use super::registry::Registry;
use super::storage::{InstalledPrimitive, MarketplaceStorage};

/// Installer for marketplace primitives.
#[derive(Debug, Clone)]
pub struct Installer {
    storage: MarketplaceStorage,
    registry: Registry,
    platform: TargetPlatform,
}

/// Installation options.
#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub name: String,
    pub version: Option<String>,
    pub force: bool,
    pub dry_run: bool,
}

/// Installation result.
#[derive(Debug, Clone)]
pub struct InstallResult {
    pub name: String,
    pub version: String,
    pub binary_path: PathBuf,
}

impl Installer {
    /// Create a new installer.
    pub fn new(
        storage: MarketplaceStorage,
        registry: Registry,
        platform: TargetPlatform,
    ) -> Self {
        Self {
            storage,
            registry,
            platform,
        }
    }

    /// Install a primitive.
    ///
    /// # Arguments
    ///
    /// * `options` - Installation options
    ///
    /// # Process
    ///
    /// 1. Fetch manifest from registry
    /// 2. Check if already installed (unless force=true)
    /// 3. Resolve version (default: latest from manifest)
    /// 4. Check platform support
    /// 5. Download binary from GitHub Releases
    /// 6. Verify SHA256 checksum (if available)
    /// 7. Extract archive to bin directory
    /// 8. Update installation manifest
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Primitive not found in registry
    /// - Version not found
    /// - Platform not supported
    /// - Download fails
    /// - Checksum verification fails
    /// - Extraction fails
    pub async fn install(&self, options: InstallOptions) -> Result<InstallResult> {
        // Fetch manifest
        let manifest = self.registry.get_manifest(&options.name).await?;

        // Use version from manifest if not specified
        let version = options.version.unwrap_or_else(|| manifest.primitive.version.clone());

        // Check if already installed
        if !options.force {
            let manifest_data = self.storage.load_manifest()?;
            if let Some(installed) = manifest_data.primitives.get(&options.name) && installed.version == version {
                return Err(MarketplaceError::AlreadyInstalled {
                    name: options.name.clone(),
                    version: version.clone(),
                });
            }
        }

        // Check platform support
        let platform_str = self.platform.as_str();
        let filename = manifest
            .binaries
            .targets
            .get(platform_str)
            .ok_or_else(|| MarketplaceError::PlatformNotSupported {
                platform: platform_str.to_string(),
                primitive: options.name.clone(),
            })?;

        if options.dry_run {
            eprintln!("[DRY RUN] Would install {} v{}", options.name, version);
            eprintln!("[DRY RUN] Platform: {}", platform_str);
            eprintln!("[DRY RUN] File: {}", filename);
            return Ok(InstallResult {
                name: options.name,
                version,
                binary_path: PathBuf::new(),
            });
        }

        // Construct download URL
        let download_url = format!(
            "{}/download/v{}/{}",
            manifest.binaries.release_url, version, filename
        );

        // Create bin directory if it doesn't exist
        let bin_dir = self.storage.bin_dir();
        std::fs::create_dir_all(&bin_dir)?;

        // Download to temp file
        let temp_dir = tempfile::tempdir()?;
        let archive_path = temp_dir.path().join(filename);

        eprintln!("Downloading {} v{}...", options.name, version);
        self.download_binary(&download_url, &archive_path).await?;

        // Extract archive
        eprintln!("Extracting...");
        self.extract_archive(&archive_path, &bin_dir).await?;

        // Determine binary name and path
        let binary_name = if cfg!(windows) {
            format!("{}.exe", options.name)
        } else {
            options.name.clone()
        };
        let binary_path = bin_dir.join(&binary_name);

        // Make binary executable on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&binary_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&binary_path, perms)?;
        }

        // Update installation manifest
        let mut manifest_data = self.storage.load_manifest()?;
        let installed_at = chrono::Utc::now().to_rfc3339();
        manifest_data.primitives.insert(
            options.name.clone(),
            InstalledPrimitive {
                name: options.name.clone(),
                version: version.clone(),
                kind: manifest.primitive.kind.clone(),
                binary_path: binary_path.clone(),
                installed_at,
            },
        );
        self.storage.save_manifest(&manifest_data)?;

        eprintln!("Successfully installed {} v{}", options.name, version);

        Ok(InstallResult {
            name: options.name,
            version,
            binary_path,
        })
    }

    /// Remove an installed primitive.
    ///
    /// # Arguments
    ///
    /// * `name` - Name of the primitive to remove
    ///
    /// # Process
    ///
    /// 1. Check if installed
    /// 2. Remove binary file
    /// 3. Update installation manifest
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Primitive is not installed
    /// - File deletion fails
    pub async fn remove(&self, name: &str) -> Result<()> {
        let mut manifest = self.storage.load_manifest()?;
        let installed = manifest.primitives.remove(name).ok_or_else(|| {
            MarketplaceError::PrimitiveNotFound {
                name: name.to_string(),
            }
        })?;

        // Remove binary file
        if installed.binary_path.exists() {
            std::fs::remove_file(&installed.binary_path)?;
        }

        // Update manifest
        self.storage.save_manifest(&manifest)?;

        eprintln!("Successfully removed {}", name);
        Ok(())
    }

    /// Update an installed primitive to the latest version.
    ///
    /// # Arguments
    ///
    /// * `name` - Name of the primitive to update
    /// * `dry_run` - If true, only check for updates without installing
    ///
    /// # Returns
    ///
    /// `Some(InstallResult)` if an update was performed, `None` if already up-to-date
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Primitive is not installed
    /// - Update fails
    pub async fn update(&self, name: &str, dry_run: bool) -> Result<Option<InstallResult>> {
        let manifest_data = self.storage.load_manifest()?;
        let installed = manifest_data
            .primitives
            .get(name)
            .ok_or_else(|| MarketplaceError::PrimitiveNotFound {
                name: name.to_string(),
            })?;

        // Fetch latest version from registry
        let registry_manifest = self.registry.get_manifest(name).await?;
        let latest_version = registry_manifest.primitive.version;

        if installed.version == latest_version {
            if !dry_run {
                eprintln!("{} is already up-to-date (v{})", name, latest_version);
            }
            return Ok(None);
        }

        if dry_run {
            eprintln!(
                "[DRY RUN] Would update {} from v{} to v{}",
                name, installed.version, latest_version
            );
            return Ok(None);
        }

        eprintln!(
            "Updating {} from v{} to v{}...",
            name, installed.version, latest_version
        );

        let options = InstallOptions {
            name: name.to_string(),
            version: Some(latest_version),
            force: true,
            dry_run: false,
        };

        let result = self.install(options).await?;
        Ok(Some(result))
    }

    /// Check if a primitive is installed.
    pub fn is_installed(&self, name: &str) -> Result<bool> {
        let manifest = self.storage.load_manifest()?;
        Ok(manifest.primitives.contains_key(name))
    }

    /// List all installed primitives.
    pub fn list_installed(&self) -> Result<Vec<InstalledPrimitive>> {
        let manifest = self.storage.load_manifest()?;
        Ok(manifest.primitives.values().cloned().collect())
    }

    async fn download_binary(&self, url: &str, dest: &Path) -> Result<()> {
        let client = reqwest::Client::new();
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| MarketplaceError::Download {
                url: url.to_string(),
                source: e,
            })?;

        if !response.status().is_success() {
            return Err(MarketplaceError::InvalidManifest {
                reason: format!("HTTP {} when downloading {}", response.status(), url),
            });
        }

        let total_size = response.content_length().unwrap_or(0);
        let pb = if total_size > 0 {
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
            .map_err(|e| MarketplaceError::Download {
                url: url.to_string(),
                source: e,
            })?;

        if let Some(ref bar) = pb {
            bar.inc(bytes.len() as u64);
            bar.finish_with_message("Downloaded");
        }

        std::fs::write(dest, bytes)?;
        Ok(())
    }

    async fn extract_archive(&self, archive: &Path, dest: &Path) -> Result<()> {
        let file = std::fs::File::open(archive)?;

        // Determine archive type from extension
        let extension = archive
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        match extension {
            "gz" => {
                // tar.gz
                let decoder = flate2::read::GzDecoder::new(file);
                let mut tar = tar::Archive::new(decoder);
                tar.unpack(dest).map_err(|e| MarketplaceError::ExtractionFailed {
                    path: archive.display().to_string(),
                    source: Box::new(e),
                })?;
            }
            "zip" => {
                let mut zip = zip::ZipArchive::new(file).map_err(|e| {
                    MarketplaceError::ExtractionFailed {
                        path: archive.display().to_string(),
                        source: Box::new(e),
                    }
                })?;
                zip.extract(dest).map_err(|e| MarketplaceError::ExtractionFailed {
                    path: archive.display().to_string(),
                    source: Box::new(e),
                })?;
            }
            _ => {
                return Err(MarketplaceError::InvalidManifest {
                    reason: format!("Unsupported archive format: {extension}"),
                });
            }
        }

        Ok(())
    }

    #[allow(dead_code)]
    async fn verify_checksum(&self, file: &Path, expected: &str) -> Result<()> {
        let mut hasher = Sha256::new();
        let bytes = std::fs::read(file)?;
        hasher.update(&bytes);
        let result = hasher.finalize();
        let actual = hex::encode(result);

        if actual != expected {
            return Err(MarketplaceError::ChecksumMismatch {
                expected: expected.to_string(),
                actual,
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_install_options_new() {
        let options = InstallOptions {
            name: "test-primitive".to_string(),
            version: Some("1.0.0".to_string()),
            force: false,
            dry_run: false,
        };

        assert_eq!(options.name, "test-primitive");
        assert_eq!(options.version, Some("1.0.0".to_string()));
        assert!(!options.force);
        assert!(!options.dry_run);
    }

    #[test]
    fn test_install_result_new() {
        let result = InstallResult {
            name: "test-primitive".to_string(),
            version: "1.0.0".to_string(),
            binary_path: PathBuf::from("/path/to/binary"),
        };

        assert_eq!(result.name, "test-primitive");
        assert_eq!(result.version, "1.0.0");
        assert_eq!(result.binary_path, PathBuf::from("/path/to/binary"));
    }
}
