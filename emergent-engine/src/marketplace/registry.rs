//! Registry fetching and manifest parsing.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::error::{MarketplaceError, Result};

/// Registry metadata from the root index.toml.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RegistryMetadata {
    pub registry: RegistryInfo,
    #[serde(default)]
    pub primitives: Vec<PrimitiveEntry>,
}

/// Registry information.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RegistryInfo {
    pub name: String,
    pub version: String,
}

/// Entry in the registry index.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PrimitiveEntry {
    pub name: String,
    pub version: String,
    pub kind: String,
    pub description: String,
    #[serde(default)]
    pub publishes: Vec<String>,
    #[serde(default)]
    pub subscribes: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Full manifest for a specific primitive.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PrimitiveManifest {
    pub primitive: PrimitiveInfo,
    pub messages: MessageInfo,
    #[serde(default)]
    pub args: Vec<ArgumentInfo>,
    pub binaries: BinaryInfo,
}

/// Primitive information.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PrimitiveInfo {
    pub name: String,
    pub version: String,
    pub kind: String,
}

/// Message type information.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct MessageInfo {
    #[serde(default)]
    pub publishes: Vec<String>,
    #[serde(default)]
    pub subscribes: Vec<String>,
}

/// Argument information for CLI.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ArgumentInfo {
    pub name: String,
    pub long: String,
    #[serde(default)]
    pub env: String,
    #[serde(default)]
    pub required: bool,
}

/// Binary information and download URLs.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BinaryInfo {
    pub release_url: String,
    pub targets: HashMap<String, String>,
}

/// Registry handle for fetching manifests.
#[derive(Debug, Clone)]
pub struct Registry {
    cache_dir: PathBuf,
    registry_url: String,
}

impl Registry {
    /// Create a new registry handle.
    ///
    /// # Arguments
    ///
    /// * `cache_dir` - Directory to cache the registry
    /// * `registry_url` - Git URL of the registry repository
    pub fn new(cache_dir: PathBuf, registry_url: String) -> Self {
        Self {
            cache_dir,
            registry_url,
        }
    }

    /// Fetch or refresh the registry index.
    ///
    /// This clones or updates a local cache of the registry git repository,
    /// then parses the index.toml file.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Git operations fail
    /// - The index.toml file is missing or invalid
    pub async fn fetch_index(&self) -> Result<RegistryMetadata> {
        let repo_dir = self.cache_dir.join("repo");

        // Clone or update the registry
        if repo_dir.exists() {
            // Pull latest changes
            self.git_pull(&repo_dir)?;
        } else {
            // Clone repository
            self.git_clone(&repo_dir)?;
        }

        // Read and parse index.toml
        let index_path = repo_dir.join("index.toml");
        let content = std::fs::read_to_string(&index_path)?;
        let metadata: RegistryMetadata = toml::from_str(&content).map_err(|e| {
            MarketplaceError::TomlParse {
                path: index_path.display().to_string(),
                source: e,
            }
        })?;

        Ok(metadata)
    }

    /// Get manifest for a specific primitive.
    ///
    /// # Arguments
    ///
    /// * `name` - The primitive name
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The primitive is not found in the index
    /// - The manifest file is missing or invalid
    pub async fn get_manifest(&self, name: &str) -> Result<PrimitiveManifest> {
        let repo_dir = self.cache_dir.join("repo");
        let manifest_path = repo_dir.join("primitives").join(name).join("manifest.toml");

        if !manifest_path.exists() {
            return Err(MarketplaceError::PrimitiveNotFound {
                name: name.to_string(),
            });
        }

        let content = std::fs::read_to_string(&manifest_path)?;
        let manifest: PrimitiveManifest = toml::from_str(&content).map_err(|e| {
            MarketplaceError::TomlParse {
                path: manifest_path.display().to_string(),
                source: e,
            }
        })?;

        Ok(manifest)
    }

    /// Search primitives by query.
    ///
    /// Searches in name, description, and tags.
    ///
    /// # Arguments
    ///
    /// * `index` - The registry index
    /// * `query` - Search query string
    ///
    /// # Returns
    ///
    /// Vec of matching primitive entries
    pub fn search<'a>(&self, index: &'a RegistryMetadata, query: &str) -> Vec<&'a PrimitiveEntry> {
        let query_lower = query.to_lowercase();
        index
            .primitives
            .iter()
            .filter(|p| {
                p.name.to_lowercase().contains(&query_lower)
                    || p.description.to_lowercase().contains(&query_lower)
                    || p.tags.iter().any(|t| t.to_lowercase().contains(&query_lower))
            })
            .collect()
    }

    /// Filter primitives by kind.
    ///
    /// # Arguments
    ///
    /// * `index` - The registry index
    /// * `kind` - Primitive kind ("source", "handler", or "sink")
    ///
    /// # Returns
    ///
    /// Vec of matching primitive entries
    pub fn filter_by_kind<'a>(
        &self,
        index: &'a RegistryMetadata,
        kind: &str,
    ) -> Vec<&'a PrimitiveEntry> {
        index
            .primitives
            .iter()
            .filter(|p| p.kind.eq_ignore_ascii_case(kind))
            .collect()
    }

    fn git_clone(&self, dest: &Path) -> Result<()> {
        let output = Command::new("git")
            .arg("clone")
            .arg(&self.registry_url)
            .arg(dest)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(MarketplaceError::GitError {
                message: format!("Failed to clone registry: {stderr}"),
            });
        }

        Ok(())
    }

    fn git_pull(&self, repo_dir: &Path) -> Result<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_dir)
            .arg("pull")
            .arg("--ff-only")
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(MarketplaceError::GitError {
                message: format!("Failed to pull registry updates: {stderr}"),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_registry_metadata() {
        let toml = r#"
[registry]
name = "emergent-official"
version = "1.0.0"

[[primitives]]
name = "timer"
version = "0.1.0"
kind = "source"
description = "Timer source"
publishes = ["timer.tick"]
tags = ["time"]
        "#;

        if let Ok(metadata) = toml::from_str::<RegistryMetadata>(toml) {
            assert_eq!(metadata.registry.name, "emergent-official");
            assert_eq!(metadata.registry.version, "1.0.0");
            assert_eq!(metadata.primitives.len(), 1);
            assert_eq!(metadata.primitives[0].name, "timer");
        } else {
            panic!("Should parse valid TOML");
        }
    }

    #[test]
    fn test_parse_primitive_manifest() {
        let toml = r#"
[primitive]
name = "slack-source"
version = "0.1.0"
kind = "source"

[messages]
publishes = ["slack.message", "slack.reaction"]

[[args]]
name = "token"
long = "token"
env = "SLACK_TOKEN"
required = true

[binaries]
release_url = "https://github.com/govcraft/emergent-primitives/releases"

[binaries.targets]
x86_64-unknown-linux-gnu = "slack-source-0.1.0-x86_64-unknown-linux-gnu.tar.gz"
        "#;

        if let Ok(manifest) = toml::from_str::<PrimitiveManifest>(toml) {
            assert_eq!(manifest.primitive.name, "slack-source");
            assert_eq!(manifest.primitive.version, "0.1.0");
            assert_eq!(manifest.messages.publishes.len(), 2);
            assert_eq!(manifest.args.len(), 1);
            assert!(manifest.args[0].required);
        } else {
            panic!("Should parse valid TOML");
        }
    }

    #[test]
    fn test_search_by_name() {
        let index = create_test_index();
        let registry = Registry::new(PathBuf::from("/tmp"), String::new());

        let results = registry.search(&index, "timer");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "timer");
    }

    #[test]
    fn test_search_by_description() {
        let index = create_test_index();
        let registry = Registry::new(PathBuf::from("/tmp"), String::new());

        let results = registry.search(&index, "slack");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "slack-source");
    }

    #[test]
    fn test_search_by_tag() {
        let index = create_test_index();
        let registry = Registry::new(PathBuf::from("/tmp"), String::new());

        let results = registry.search(&index, "time");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "timer");
    }

    #[test]
    fn test_filter_by_kind() {
        let index = create_test_index();
        let registry = Registry::new(PathBuf::from("/tmp"), String::new());

        let results = registry.filter_by_kind(&index, "source");
        assert_eq!(results.len(), 2);

        let results = registry.filter_by_kind(&index, "handler");
        assert_eq!(results.len(), 1);

        let results = registry.filter_by_kind(&index, "sink");
        assert!(results.is_empty());
    }

    fn create_test_index() -> RegistryMetadata {
        RegistryMetadata {
            registry: RegistryInfo {
                name: "test-registry".to_string(),
                version: "1.0.0".to_string(),
            },
            primitives: vec![
                PrimitiveEntry {
                    name: "timer".to_string(),
                    version: "0.1.0".to_string(),
                    kind: "source".to_string(),
                    description: "Timer source".to_string(),
                    publishes: vec!["timer.tick".to_string()],
                    subscribes: vec![],
                    tags: vec!["time".to_string()],
                },
                PrimitiveEntry {
                    name: "slack-source".to_string(),
                    version: "0.1.0".to_string(),
                    kind: "source".to_string(),
                    description: "Monitor Slack channels".to_string(),
                    publishes: vec!["slack.message".to_string()],
                    subscribes: vec![],
                    tags: vec!["slack".to_string(), "chat".to_string()],
                },
                PrimitiveEntry {
                    name: "filter".to_string(),
                    version: "0.1.0".to_string(),
                    kind: "handler".to_string(),
                    description: "Filter events".to_string(),
                    publishes: vec!["filter.processed".to_string()],
                    subscribes: vec!["timer.tick".to_string()],
                    tags: vec![],
                },
            ],
        }
    }
}
