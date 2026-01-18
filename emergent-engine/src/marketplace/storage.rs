//! XDG-compliant storage management for marketplace data.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::error::{MarketplaceError, Result};

/// Manages XDG-compliant storage for marketplace data.
///
/// Uses the following directories:
/// - Config: `~/.config/emergent/marketplace.toml`
/// - Cache: `~/.cache/emergent/registry/`
/// - Data: `~/.local/share/emergent/primitives/`
#[derive(Debug, Clone)]
pub struct MarketplaceStorage {
    config_dir: PathBuf,
    cache_dir: PathBuf,
    data_dir: PathBuf,
}

/// Configuration for the marketplace.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct MarketplaceConfig {
    /// URL of the registry repository
    pub registry_url: String,
}

impl Default for MarketplaceConfig {
    fn default() -> Self {
        Self {
            registry_url: "https://github.com/govcraft/emergent-primitives".to_string(),
        }
    }
}

/// Metadata about an installed primitive.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct InstalledPrimitive {
    pub name: String,
    pub version: String,
    pub kind: String,
    pub binary_path: PathBuf,
    pub installed_at: String,
}

/// Installation manifest tracking all installed primitives.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct InstallationManifest {
    #[serde(default)]
    pub primitives: HashMap<String, InstalledPrimitive>,
}

impl MarketplaceStorage {
    /// Create a new storage handle, initializing directories if needed.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - XDG directories cannot be determined
    /// - Directory creation fails
    pub fn new() -> Result<Self> {
        let proj_dirs = directories::ProjectDirs::from("ai", "govcraft", "emergent")
            .ok_or_else(|| MarketplaceError::XdgDirectories {
                message: "Could not determine XDG directories".to_string(),
            })?;

        let config_dir = proj_dirs.config_dir().to_path_buf();
        let cache_dir = proj_dirs.cache_dir().join("registry");
        let data_dir = proj_dirs.data_dir().join("primitives");

        // Create directories if they don't exist
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&cache_dir)?;
        std::fs::create_dir_all(&data_dir)?;

        Ok(Self {
            config_dir,
            cache_dir,
            data_dir,
        })
    }

    /// Get the config directory path.
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// Get the cache directory path.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Get the data directory path.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Get the path to the marketplace config file.
    fn config_path(&self) -> PathBuf {
        self.config_dir.join("marketplace.toml")
    }

    /// Get the path to the installation manifest file.
    fn manifest_path(&self) -> PathBuf {
        self.data_dir.join(".installed.toml")
    }

    /// Load marketplace configuration.
    ///
    /// If the config file doesn't exist, returns default configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the config file exists but cannot be read or parsed.
    pub fn load_config(&self) -> Result<MarketplaceConfig> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(MarketplaceConfig::default());
        }

        let content = std::fs::read_to_string(&path)?;
        let config: MarketplaceConfig = toml::from_str(&content).map_err(|e| {
            MarketplaceError::TomlParse {
                path: path.display().to_string(),
                source: e,
            }
        })?;
        Ok(config)
    }

    /// Save marketplace configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - The configuration to save
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails.
    pub fn save_config(&self, config: &MarketplaceConfig) -> Result<()> {
        let path = self.config_path();
        let content = toml::to_string_pretty(config).map_err(|e| {
            MarketplaceError::TomlSerialize {
                path: path.display().to_string(),
                source: e,
            }
        })?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Load installation manifest.
    ///
    /// If the manifest doesn't exist, returns an empty manifest.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest exists but cannot be read or parsed.
    pub fn load_manifest(&self) -> Result<InstallationManifest> {
        let path = self.manifest_path();
        if !path.exists() {
            return Ok(InstallationManifest::default());
        }

        let content = std::fs::read_to_string(&path)?;
        let manifest: InstallationManifest =
            toml::from_str(&content).map_err(|e| MarketplaceError::TomlParse {
                path: path.display().to_string(),
                source: e,
            })?;
        Ok(manifest)
    }

    /// Save installation manifest.
    ///
    /// # Arguments
    ///
    /// * `manifest` - The manifest to save
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails.
    pub fn save_manifest(&self, manifest: &InstallationManifest) -> Result<()> {
        let path = self.manifest_path();
        let content = toml::to_string_pretty(manifest).map_err(|e| {
            MarketplaceError::TomlSerialize {
                path: path.display().to_string(),
                source: e,
            }
        })?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Get the binary installation directory.
    pub fn bin_dir(&self) -> PathBuf {
        self.data_dir.join("bin")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_temp_storage() -> (TempDir, MarketplaceStorage) {
        let temp_dir = match TempDir::new() {
            Ok(dir) => dir,
            Err(e) => panic!("Failed to create temp dir: {e}"),
        };

        let storage = MarketplaceStorage {
            config_dir: temp_dir.path().join("config"),
            cache_dir: temp_dir.path().join("cache"),
            data_dir: temp_dir.path().join("data"),
        };

        if let Err(e) = std::fs::create_dir_all(&storage.config_dir) {
            panic!("Failed to create config dir: {e}");
        }
        if let Err(e) = std::fs::create_dir_all(&storage.cache_dir) {
            panic!("Failed to create cache dir: {e}");
        }
        if let Err(e) = std::fs::create_dir_all(&storage.data_dir) {
            panic!("Failed to create data dir: {e}");
        }

        (temp_dir, storage)
    }

    #[test]
    fn test_storage_new() {
        // This test will use actual XDG directories, so we just verify it doesn't panic
        let result = MarketplaceStorage::new();
        assert!(result.is_ok());

        if let Ok(storage) = result {
            assert!(storage.config_dir().exists());
            assert!(storage.cache_dir().exists());
            assert!(storage.data_dir().exists());
        } else {
            panic!("Should create storage");
        }
    }

    #[test]
    fn test_load_default_config() {
        let (_temp, storage) = create_temp_storage();

        if let Ok(config) = storage.load_config() {
            assert_eq!(config, MarketplaceConfig::default());
        } else {
            panic!("Should load default config");
        }
    }

    #[test]
    fn test_save_and_load_config() {
        let (_temp, storage) = create_temp_storage();

        let config = MarketplaceConfig {
            registry_url: "https://example.com/registry".to_string(),
        };

        if let Err(e) = storage.save_config(&config) {
            panic!("Failed to save config: {e}");
        }

        if let Ok(loaded) = storage.load_config() {
            assert_eq!(loaded, config);
        } else {
            panic!("Failed to load config");
        }
    }

    #[test]
    fn test_load_empty_manifest() {
        let (_temp, storage) = create_temp_storage();

        if let Ok(manifest) = storage.load_manifest() {
            assert_eq!(manifest, InstallationManifest::default());
            assert!(manifest.primitives.is_empty());
        } else {
            panic!("Should load manifest");
        }
    }

    #[test]
    fn test_save_and_load_manifest() {
        let (_temp, storage) = create_temp_storage();

        let mut manifest = InstallationManifest::default();
        manifest.primitives.insert(
            "test-primitive".to_string(),
            InstalledPrimitive {
                name: "test-primitive".to_string(),
                version: "1.0.0".to_string(),
                kind: "source".to_string(),
                binary_path: PathBuf::from("/path/to/binary"),
                installed_at: "2024-01-01T00:00:00Z".to_string(),
            },
        );

        if let Err(e) = storage.save_manifest(&manifest) {
            panic!("Failed to save manifest: {e}");
        }

        if let Ok(loaded) = storage.load_manifest() {
            assert_eq!(loaded, manifest);
            assert_eq!(loaded.primitives.len(), 1);
        } else {
            panic!("Failed to load manifest");
        }
    }

    #[test]
    fn test_bin_dir() {
        let (_temp, storage) = create_temp_storage();
        let bin_dir = storage.bin_dir();
        assert_eq!(bin_dir, storage.data_dir().join("bin"));
    }

    #[test]
    fn test_config_dir_getters() {
        let (_temp, storage) = create_temp_storage();
        assert_eq!(storage.config_dir(), storage.config_dir);
        assert_eq!(storage.cache_dir(), storage.cache_dir);
        assert_eq!(storage.data_dir(), storage.data_dir);
    }
}
