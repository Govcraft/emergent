//! Binary download, verification, and installation.

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

use super::error::{MarketplaceError, Result};
use super::platform::TargetPlatform;
use super::registry::{self, Registry};
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
    pub fn new(storage: MarketplaceStorage, registry: Registry, platform: TargetPlatform) -> Self {
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
    /// 1. Fetch the manifest of the release being installed
    /// 2. Check if already installed (unless force=true)
    /// 3. Resolve version (default: the latest release)
    /// 4. Check platform support
    /// 5. Download binary from that release
    /// 6. Verify SHA256 against that release's checksums.txt (if published)
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
        // Fetch the manifest of the release being installed, so a pinned
        // version is described by its own release rather than by the latest.
        let manifest = self
            .registry
            .get_manifest(&options.name, options.version.as_deref())
            .await?;

        // Use the version the manifest came from if none was pinned
        let version = options
            .version
            .unwrap_or_else(|| manifest.primitive.version.clone());

        // Check if already installed
        if !options.force {
            let manifest_data = self.storage.load_manifest()?;
            if let Some(installed) = manifest_data.primitives.get(&options.name)
                && installed.version == version
            {
                return Err(MarketplaceError::AlreadyInstalled {
                    name: options.name.clone(),
                    version: version.clone(),
                });
            }
        }

        // Check platform support
        let platform_str = self.platform.as_str();
        let filename = manifest.binaries.targets.get(platform_str).ok_or_else(|| {
            MarketplaceError::PlatformNotSupported {
                platform: platform_str.to_string(),
                primitive: options.name.clone(),
            }
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
        let download_url =
            registry::asset_url(&manifest.binaries.release_url, Some(&version), filename);

        // Create bin directory if it doesn't exist
        let bin_dir = self.storage.bin_dir();
        std::fs::create_dir_all(&bin_dir)?;

        // Download to temp file
        let temp_dir = tempfile::tempdir()?;
        let archive_path = temp_dir.path().join(filename);

        eprintln!("Downloading {} v{}...", options.name, version);
        self.download_binary(&download_url, &archive_path).await?;

        // Verify before anything from the archive reaches the bin directory,
        // against the checksums published by the same release as the archive.
        let published = self
            .registry
            .release_checksums(&manifest.binaries.release_url, &version)
            .await;
        let expected = expected_checksum(
            published.as_ref(),
            filename,
            manifest.binaries.checksum_for(platform_str),
        );
        if verify_archive(&archive_path, expected)? {
            eprintln!("Checksum verified.");
        } else {
            eprintln!(
                "Warning: release v{version} publishes no checksum for {}; installing unverified.",
                filename
            );
        }

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
        let installed = manifest_data.primitives.get(name).ok_or_else(|| {
            MarketplaceError::PrimitiveNotFound {
                name: name.to_string(),
            }
        })?;

        // Fetch latest version from registry
        let registry_manifest = self.registry.get_manifest(name, None).await?;
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
        let extension = archive.extension().and_then(|s| s.to_str()).unwrap_or("");

        match extension {
            "gz" => {
                // tar.gz
                let decoder = flate2::read::GzDecoder::new(file);
                let mut tar = tar::Archive::new(decoder);
                tar.unpack(dest)
                    .map_err(|e| MarketplaceError::ExtractionFailed {
                        path: archive.display().to_string(),
                        source: Box::new(e),
                    })?;
            }
            "zip" => {
                let mut zip =
                    zip::ZipArchive::new(file).map_err(|e| MarketplaceError::ExtractionFailed {
                        path: archive.display().to_string(),
                        source: Box::new(e),
                    })?;
                zip.extract(dest)
                    .map_err(|e| MarketplaceError::ExtractionFailed {
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
}

/// The checksum to verify an archive against.
///
/// The release's own checksums.txt is the authority, because it ships with the
/// archive it describes. A manifest served from elsewhere may publish one
/// instead, and neither means the install proceeds unverified.
fn expected_checksum<'a>(
    published: Option<&'a BTreeMap<String, String>>,
    filename: &str,
    from_manifest: Option<&'a str>,
) -> Option<&'a str> {
    published
        .and_then(|sums| sums.get(filename))
        .map(String::as_str)
        .or(from_manifest)
}

/// Lowercase hex SHA-256 of a byte slice.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Compare a computed checksum against the published one.
///
/// Hex case and surrounding whitespace in the published value do not matter.
fn check_checksum(actual: &str, expected: &str) -> Result<()> {
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(MarketplaceError::ChecksumMismatch {
            expected: expected.trim().to_string(),
            actual: actual.to_string(),
        })
    }
}

/// Verify a downloaded archive against the manifest's checksum, if it has one.
///
/// Returns `Ok(true)` when the archive was verified and `Ok(false)` when the
/// manifest publishes no checksum for it, so the caller can say so.
fn verify_archive(archive: &Path, expected: Option<&str>) -> Result<bool> {
    let Some(expected) = expected else {
        return Ok(false);
    };
    let bytes = std::fs::read(archive)?;
    check_checksum(&sha256_hex(&bytes), expected)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHA-256 of the ASCII string "abc", from FIPS 180-2.
    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn sha256_hex_matches_the_published_test_vector() {
        assert_eq!(sha256_hex(b"abc"), ABC_SHA256);
    }

    #[test]
    fn checksum_comparison_ignores_hex_case_and_padding() {
        let published = format!("  {}\n", ABC_SHA256.to_uppercase());
        assert!(check_checksum(ABC_SHA256, &published).is_ok());
    }

    #[test]
    fn a_different_checksum_is_a_mismatch_naming_both_values() {
        let expected = "0".repeat(64);
        match check_checksum(ABC_SHA256, &expected) {
            Err(MarketplaceError::ChecksumMismatch {
                expected: e,
                actual: a,
            }) => {
                assert_eq!(e, expected);
                assert_eq!(a, ABC_SHA256);
            }
            other => panic!("expected a checksum mismatch, got {other:?}"),
        }
    }

    #[test]
    fn an_archive_is_verified_only_when_a_checksum_is_published()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let archive = dir.path().join("primitive.tar.gz");
        std::fs::write(&archive, b"abc")?;

        assert!(!verify_archive(&archive, None)?);
        assert!(verify_archive(&archive, Some(ABC_SHA256))?);
        Ok(())
    }

    #[test]
    fn a_tampered_archive_fails_verification() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let archive = dir.path().join("primitive.tar.gz");
        std::fs::write(&archive, b"abd")?;

        assert!(matches!(
            verify_archive(&archive, Some(ABC_SHA256)),
            Err(MarketplaceError::ChecksumMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn the_release_checksum_outranks_the_manifest_one() {
        let published = BTreeMap::from([("primitive.tar.gz".to_string(), ABC_SHA256.to_string())]);
        assert_eq!(
            expected_checksum(Some(&published), "primitive.tar.gz", Some("stale")),
            Some(ABC_SHA256)
        );
    }

    #[test]
    fn a_manifest_checksum_answers_when_the_release_publishes_none() {
        let published = BTreeMap::from([("other.tar.gz".to_string(), ABC_SHA256.to_string())]);
        assert_eq!(
            expected_checksum(Some(&published), "primitive.tar.gz", Some("abc123")),
            Some("abc123")
        );
        assert_eq!(
            expected_checksum(None, "primitive.tar.gz", Some("abc123")),
            Some("abc123")
        );
    }

    #[test]
    fn nothing_published_anywhere_leaves_the_archive_unverified() {
        assert_eq!(expected_checksum(None, "primitive.tar.gz", None), None);
        let empty = BTreeMap::new();
        assert_eq!(
            expected_checksum(Some(&empty), "primitive.tar.gz", None),
            None
        );
    }

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
