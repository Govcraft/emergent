//! Registry fetching and manifest parsing.
//!
//! The registry is two files served over HTTPS: `index.toml`, which lists
//! every primitive, and `manifests.toml`, which bundles each primitive's full
//! manifest. The emergent-primitives release publishes both as release assets,
//! so `registry_url` is a base URL and anyone can host a registry by serving
//! those two files.
//!
//! Engine 0.10.10 and earlier cloned a git repository into the cache instead,
//! which required git on the PATH and could only ever describe one version: a
//! pinned install got the current manifest's filenames. Every fetch here is
//! scoped to a release, so a pinned install reads that release's manifest.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::error::{MarketplaceError, Result};

/// The index asset: what `list` and `search` read.
pub const INDEX_FILE: &str = "index.toml";

/// The manifest bundle asset: what `info` and `install` read.
pub const MANIFESTS_FILE: &str = "manifests.toml";

/// The checksum asset a release publishes beside its archives.
pub const CHECKSUMS_FILE: &str = "checksums.txt";

/// Registry metadata from the release's index.toml.
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
    /// The release this index was published from.
    pub version: String,
    #[serde(default)]
    pub description: String,
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

/// Every manifest a release publishes, keyed by primitive name.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ManifestBundle {
    pub version: String,
    #[serde(default)]
    pub manifests: BTreeMap<String, PrimitiveManifest>,
}

/// Full manifest for a specific primitive.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PrimitiveManifest {
    pub primitive: PrimitiveInfo,
    #[serde(default)]
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
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// The runtime a primitive needs when it is not a native binary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
}

/// Message type information.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
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
    pub short: Option<String>,
    #[serde(default)]
    pub env: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: String,
}

/// Binary information and download URLs.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct BinaryInfo {
    pub release_url: String,
    pub targets: HashMap<String, String>,
    /// Lowercase hex SHA-256 of each target's archive, keyed by target triple.
    ///
    /// An emergent-primitives release leaves this out and publishes
    /// `checksums.txt` instead. A manifest served from somewhere else may
    /// still carry it, and it is used when the release has no checksum file.
    /// An empty string means no checksum.
    #[serde(default)]
    pub checksums: HashMap<String, String>,
}

impl BinaryInfo {
    /// The checksum the manifest itself publishes for a target, or `None` when
    /// it has no entry or only an empty placeholder.
    #[must_use]
    pub fn checksum_for(&self, target: &str) -> Option<&str> {
        self.checksums
            .get(target)
            .map(|sum| sum.trim())
            .filter(|sum| !sum.is_empty())
    }
}

/// What a fetch should do about the disk cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePlan {
    /// A pinned release never changes, so a cached copy is the answer.
    UseCache,
    /// Ask the network, and fall back to the cache if it cannot be reached.
    Fetch,
}

/// Registry handle for fetching the index and manifests.
#[derive(Debug, Clone)]
pub struct Registry {
    cache_dir: PathBuf,
    base_url: String,
    /// Bodies already fetched in this process, keyed by URL, so updating every
    /// installed primitive fetches the bundle once.
    fetched: Arc<Mutex<HashMap<String, String>>>,
}

impl Registry {
    /// Create a new registry handle.
    ///
    /// # Arguments
    ///
    /// * `cache_dir` - Directory to cache fetched assets in
    /// * `base_url` - Base URL serving index.toml and manifests.toml
    pub fn new(cache_dir: PathBuf, base_url: String) -> Self {
        Self {
            cache_dir,
            base_url,
            fetched: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Fetch the index of the latest release.
    ///
    /// # Errors
    ///
    /// Returns an error if the index cannot be fetched and no cached copy
    /// exists, or if it is not valid TOML.
    pub async fn fetch_index(&self) -> Result<RegistryMetadata> {
        self.fetch_index_at(None).await
    }

    /// Fetch the index of one release, or of the latest release for `None`.
    ///
    /// # Errors
    ///
    /// Returns an error if the index cannot be fetched and no cached copy
    /// exists, or if it is not valid TOML.
    pub async fn fetch_index_at(&self, version: Option<&str>) -> Result<RegistryMetadata> {
        let body = self.asset(version, INDEX_FILE).await?;
        parse_toml(&body, INDEX_FILE)
    }

    /// Get the manifest for one primitive, from a release or from the latest.
    ///
    /// # Errors
    ///
    /// Returns an error if the bundle cannot be fetched, is not valid TOML, or
    /// names no such primitive.
    pub async fn get_manifest(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<PrimitiveManifest> {
        let body = self.asset(version, MANIFESTS_FILE).await?;
        let bundle: ManifestBundle = parse_toml(&body, MANIFESTS_FILE)?;
        bundle
            .manifests
            .get(name)
            .cloned()
            .ok_or_else(|| MarketplaceError::PrimitiveNotFound {
                name: name.to_string(),
            })
    }

    /// The checksums a release publishes for its archives, keyed by filename.
    ///
    /// Returns `None` when the release publishes no checksum file, which keeps
    /// an install possible against a host that does not publish one.
    pub async fn release_checksums(
        &self,
        release_url: &str,
        version: &str,
    ) -> Option<BTreeMap<String, String>> {
        let url = asset_url(release_url, Some(version), CHECKSUMS_FILE);
        match self.http_get(&url, CHECKSUMS_FILE).await {
            Ok(body) => Some(parse_checksums(&body)),
            Err(e) => {
                eprintln!("Warning: {e}");
                None
            }
        }
    }

    /// Search primitives by query.
    ///
    /// Searches in name, description, and tags.
    pub fn search<'a>(&self, index: &'a RegistryMetadata, query: &str) -> Vec<&'a PrimitiveEntry> {
        let query_lower = query.to_lowercase();
        index
            .primitives
            .iter()
            .filter(|p| {
                p.name.to_lowercase().contains(&query_lower)
                    || p.description.to_lowercase().contains(&query_lower)
                    || p.tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&query_lower))
            })
            .collect()
    }

    /// Filter primitives by kind.
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

    /// One registry asset, from the process cache, the disk cache or the network.
    async fn asset(&self, version: Option<&str>, file: &str) -> Result<String> {
        if let Some(version) = version
            && !is_safe_version(version)
        {
            return Err(MarketplaceError::InvalidManifest {
                reason: format!("'{version}' is not a version number"),
            });
        }

        let url = asset_url(&self.base_url, version, file);
        if let Some(body) = self.remembered(&url) {
            return Ok(body);
        }

        self.drop_legacy_clone();
        let cached = self.cache_path(version, file);
        if cache_plan(version.is_some(), cached.is_file()) == CachePlan::UseCache
            && let Ok(body) = std::fs::read_to_string(&cached)
        {
            self.remember(&url, &body);
            return Ok(body);
        }

        match self.http_get(&url, file).await {
            Ok(body) => {
                write_cache(&cached, &body);
                self.remember(&url, &body);
                Ok(body)
            }
            Err(e) => {
                // A host that answered and refused is an answer: only an
                // unreachable network falls back to a cached copy.
                let unreachable = matches!(e, MarketplaceError::Download { .. });
                match (unreachable, std::fs::read_to_string(&cached)) {
                    (true, Ok(body)) => {
                        eprintln!("{}", offline_note(file, &cached_age(&cached)));
                        self.remember(&url, &body);
                        Ok(body)
                    }
                    _ => Err(e),
                }
            }
        }
    }

    /// GET a URL, turning a refusal into an error that says what to do.
    async fn http_get(&self, url: &str, file: &str) -> Result<String> {
        let response = reqwest::Client::new().get(url).send().await.map_err(|e| {
            MarketplaceError::Download {
                url: url.to_string(),
                source: e,
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(MarketplaceError::RegistryUnavailable {
                file: file.to_string(),
                url: url.to_string(),
                status: status.as_u16(),
            });
        }

        response
            .text()
            .await
            .map_err(|e| MarketplaceError::Download {
                url: url.to_string(),
                source: e,
            })
    }

    fn remembered(&self, url: &str) -> Option<String> {
        self.fetched.lock().ok()?.get(url).cloned()
    }

    fn remember(&self, url: &str, body: &str) {
        if let Ok(mut fetched) = self.fetched.lock() {
            fetched.insert(url.to_string(), body.to_string());
        }
    }

    fn cache_path(&self, version: Option<&str>, file: &str) -> PathBuf {
        self.cache_dir.join(cache_scope(version)).join(file)
    }

    /// Remove the git clone engine 0.10.10 and earlier kept here.
    ///
    /// The engine created it, nothing reads it now, and leaving it costs the
    /// user a stale copy of a repository they never asked for.
    fn drop_legacy_clone(&self) {
        let clone = self.cache_dir.join("repo");
        if clone.join(".git").exists() && std::fs::remove_dir_all(&clone).is_ok() {
            eprintln!(
                "Removed the old git registry cache at {}; the registry is fetched over HTTPS now.",
                clone.display()
            );
        }
    }
}

/// Parse TOML, naming the asset that failed rather than a path on disk.
fn parse_toml<T: serde::de::DeserializeOwned>(body: &str, file: &str) -> Result<T> {
    toml::from_str(body).map_err(|e| MarketplaceError::TomlParse {
        path: file.to_string(),
        source: e,
    })
}

/// Write a fetched asset to the cache, leaving any older copy alone on failure.
fn write_cache(path: &Path, body: &str) {
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let temp = path.with_extension("part");
    if std::fs::write(&temp, body).is_ok() && std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

/// How long ago a cached file was written.
fn cached_age(path: &Path) -> String {
    let age = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .unwrap_or_default();
    describe_age(age)
}

/// The URL an asset lives at.
///
/// A base ending in `/releases` is a GitHub releases page, where the latest
/// release is `latest/download/<file>` and a pinned one is
/// `download/v<version>/<file>`. Both are plain redirects, so no token and no
/// API rate limit is involved. Any other base is a static host, where the
/// current files sit at the base and a pinned release under `v<version>/`.
#[must_use]
pub fn asset_url(base: &str, version: Option<&str>, file: &str) -> String {
    let base = base.trim_end_matches('/');
    match (base.ends_with("/releases"), version) {
        (true, None) => format!("{base}/latest/download/{file}"),
        (true, Some(version)) => format!("{base}/download/v{version}/{file}"),
        (false, None) => format!("{base}/{file}"),
        (false, Some(version)) => format!("{base}/v{version}/{file}"),
    }
}

/// The cache directory one release's assets live in.
#[must_use]
pub fn cache_scope(version: Option<&str>) -> String {
    match version {
        Some(version) => format!("v{version}"),
        None => "latest".to_string(),
    }
}

/// Whether a version is a version rather than a path or a URL fragment.
///
/// A version reaches both a URL and a cache path, so `../..` is refused here
/// rather than in either of them.
#[must_use]
pub fn is_safe_version(version: &str) -> bool {
    !version.is_empty()
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
        && !version.contains("..")
}

/// Whether a fetch may answer from the cache without asking the network.
///
/// A pinned release is immutable, so a cached copy of it is the answer. The
/// latest release changes, so it is always asked for.
#[must_use]
pub fn cache_plan(pinned: bool, cached: bool) -> CachePlan {
    if pinned && cached {
        CachePlan::UseCache
    } else {
        CachePlan::Fetch
    }
}

/// The line printed when the network is unreachable and the cache answers.
#[must_use]
pub fn offline_note(file: &str, age: &str) -> String {
    format!("Offline: using the cached {file} from {age}.")
}

/// A duration as the note spells it.
#[must_use]
pub fn describe_age(age: Duration) -> String {
    let seconds = age.as_secs();
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => plural(seconds / 60, "minute"),
        3600..=86_399 => plural(seconds / 3600, "hour"),
        _ => plural(seconds / 86_400, "day"),
    }
}

fn plural(count: u64, unit: &str) -> String {
    if count == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{count} {unit}s ago")
    }
}

/// Parse a `sha256sum` file into checksums keyed by filename.
///
/// Each line is `<hex>  <filename>`. GNU coreutils writes `*<filename>` for a
/// file read in binary mode, and blank lines and `#` comments are ignored.
#[must_use]
pub fn parse_checksums(text: &str) -> BTreeMap<String, String> {
    let mut checksums = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((hex, name)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let name = name.trim_start().trim_start_matches('*').trim();
        if hex.is_empty() || name.is_empty() {
            continue;
        }
        checksums.insert(name.to_string(), hex.to_ascii_lowercase());
    }
    checksums
}

/// What to tell a user whose registry host refused to serve an asset.
#[must_use]
pub fn unavailable_message(file: &str, url: &str, status: u16) -> String {
    let mut message =
        format!("Could not read {file} from the registry: {url} returned HTTP {status}.");
    if status == 404 {
        message.push_str(&format!(
            " That release publishes no {file}. Releases before primitives 0.11.0 predate it, \
             so install from a later version, or point registry_url in marketplace.toml at a \
             host that serves {INDEX_FILE} and {MANIFESTS_FILE}."
        ));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    const GITHUB: &str = "https://github.com/Govcraft/emergent-primitives/releases";

    #[test]
    fn a_github_base_resolves_to_release_assets() {
        assert_eq!(
            asset_url(GITHUB, None, INDEX_FILE),
            "https://github.com/Govcraft/emergent-primitives/releases/latest/download/index.toml"
        );
        assert_eq!(
            asset_url(GITHUB, Some("0.11.0"), MANIFESTS_FILE),
            "https://github.com/Govcraft/emergent-primitives/releases/download/v0.11.0/manifests.toml"
        );
    }

    #[test]
    fn a_trailing_slash_does_not_double_up() {
        assert_eq!(
            asset_url("https://example.invalid/releases/", None, INDEX_FILE),
            "https://example.invalid/releases/latest/download/index.toml"
        );
    }

    #[test]
    fn any_other_base_is_a_plain_static_host() {
        assert_eq!(
            asset_url("https://example.invalid/registry", None, INDEX_FILE),
            "https://example.invalid/registry/index.toml"
        );
        assert_eq!(
            asset_url(
                "https://example.invalid/registry",
                Some("0.9.0"),
                INDEX_FILE
            ),
            "https://example.invalid/registry/v0.9.0/index.toml"
        );
    }

    #[test]
    fn a_release_scopes_its_own_cache_directory() {
        assert_eq!(cache_scope(None), "latest");
        assert_eq!(cache_scope(Some("0.12.0")), "v0.12.0");
    }

    #[test]
    fn a_version_that_is_a_path_is_refused() {
        for version in ["0.12.0", "1.0.0-rc.1", "0.12.0+build.3"] {
            assert!(is_safe_version(version), "{version} should be a version");
        }
        for version in [
            "",
            "../../etc",
            "0.1/../..",
            "a/b",
            "0.1.0 x",
            "..",
            "v?x=1",
        ] {
            assert!(!is_safe_version(version), "{version} should be refused");
        }
    }

    #[test]
    fn only_a_pinned_release_answers_from_the_cache_alone() {
        for (pinned, cached, expected) in [
            (true, true, CachePlan::UseCache),
            (true, false, CachePlan::Fetch),
            (false, true, CachePlan::Fetch),
            (false, false, CachePlan::Fetch),
        ] {
            assert_eq!(cache_plan(pinned, cached), expected, "{pinned} {cached}");
        }
    }

    #[test]
    fn an_age_reads_the_way_a_person_says_it() {
        for (seconds, expected) in [
            (0, "just now"),
            (59, "just now"),
            (60, "1 minute ago"),
            (3599, "59 minutes ago"),
            (3600, "1 hour ago"),
            (86_399, "23 hours ago"),
            (86_400, "1 day ago"),
            (172_800, "2 days ago"),
        ] {
            assert_eq!(
                describe_age(Duration::from_secs(seconds)),
                expected,
                "{seconds}s"
            );
        }
    }

    #[test]
    fn checksums_parse_in_both_sha256sum_layouts() {
        let text = "\
# generated by the release workflow

ABC123  exec-source-0.12.0-x86_64-unknown-linux-gnu.tar.gz
def456 *exec-sink-0.12.0-aarch64-apple-darwin.tar.gz

nonsense
";
        let checksums = parse_checksums(text);
        assert_eq!(checksums.len(), 2);
        assert_eq!(
            checksums.get("exec-source-0.12.0-x86_64-unknown-linux-gnu.tar.gz"),
            Some(&"abc123".to_string())
        );
        assert_eq!(
            checksums.get("exec-sink-0.12.0-aarch64-apple-darwin.tar.gz"),
            Some(&"def456".to_string())
        );
    }

    #[test]
    fn a_missing_index_says_what_was_fetched_and_what_to_do() {
        let url = asset_url(GITHUB, Some("0.5.0"), INDEX_FILE);
        let message = unavailable_message(INDEX_FILE, &url, 404);
        assert!(message.contains(&url), "{message}");
        assert!(message.contains("HTTP 404"), "{message}");
        assert!(message.contains("registry_url"), "{message}");
    }

    #[test]
    fn another_refusal_names_the_status_without_guessing_at_a_cause() {
        let message = unavailable_message(INDEX_FILE, "https://example.invalid/index.toml", 503);
        assert!(message.contains("HTTP 503"), "{message}");
        assert!(!message.contains("predate"), "{message}");
    }

    #[test]
    fn the_offline_note_names_the_file_and_its_age() {
        let note = offline_note(INDEX_FILE, "5 minutes ago");
        assert!(note.contains("index.toml"), "{note}");
        assert!(note.contains("5 minutes ago"), "{note}");
    }

    #[test]
    fn test_parse_registry_metadata() {
        let toml = r#"
[registry]
name = "emergent-primitives"
version = "0.12.0"

[[primitives]]
name = "timer"
version = "0.12.0"
kind = "source"
description = "Timer source"
publishes = ["timer.tick"]
tags = ["time"]
        "#;

        if let Ok(metadata) = toml::from_str::<RegistryMetadata>(toml) {
            assert_eq!(metadata.registry.name, "emergent-primitives");
            assert_eq!(metadata.registry.version, "0.12.0");
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
description = "Monitor Slack channels"

[messages]
publishes = ["slack.message", "slack.reaction"]

[[args]]
name = "token"
long = "token"
env = "SLACK_TOKEN"
required = true
description = "API token"

[binaries]
release_url = "https://github.com/Govcraft/emergent-primitives/releases"

[binaries.targets]
x86_64-unknown-linux-gnu = "slack-source-0.1.0-x86_64-unknown-linux-gnu.tar.gz"
        "#;

        if let Ok(manifest) = toml::from_str::<PrimitiveManifest>(toml) {
            assert_eq!(manifest.primitive.name, "slack-source");
            assert_eq!(manifest.primitive.version, "0.1.0");
            assert_eq!(manifest.primitive.description, "Monitor Slack channels");
            assert_eq!(manifest.messages.publishes.len(), 2);
            assert_eq!(manifest.args.len(), 1);
            assert!(manifest.args[0].required);
        } else {
            panic!("Should parse valid TOML");
        }
    }

    #[test]
    fn a_bundle_holds_every_manifest_the_release_published() {
        let toml = r#"
version = "0.12.0"

[manifests.exec-source.primitive]
name = "exec-source"
version = "0.12.0"
kind = "source"
description = "Execute shell commands"

[manifests.exec-source.messages]
publishes = ["exec.output"]

[manifests.exec-source.binaries]
release_url = "https://example.invalid/releases"

[manifests.exec-source.binaries.targets]
x86_64-unknown-linux-gnu = "exec-source-0.12.0-x86_64-unknown-linux-gnu.tar.gz"
        "#;

        let Ok(bundle) = toml::from_str::<ManifestBundle>(toml) else {
            panic!("Should parse valid TOML");
        };
        assert_eq!(bundle.version, "0.12.0");
        let Some(manifest) = bundle.manifests.get("exec-source") else {
            panic!("bundle should hold exec-source");
        };
        assert_eq!(manifest.primitive.kind, "source");
    }

    fn manifest_with_binaries(
        binaries: &str,
    ) -> std::result::Result<PrimitiveManifest, toml::de::Error> {
        toml::from_str(&format!(
            r#"
[primitive]
name = "slack-source"
version = "0.1.0"
kind = "source"

[messages]
publishes = ["slack.message"]

[binaries]
release_url = "https://example.invalid/releases"
{binaries}
[binaries.targets]
x86_64-unknown-linux-gnu = "slack-source.tar.gz"
"#
        ))
    }

    #[test]
    fn a_manifest_without_a_checksums_table_publishes_none()
    -> std::result::Result<(), toml::de::Error> {
        let manifest = manifest_with_binaries("")?;
        assert_eq!(
            manifest.binaries.checksum_for("x86_64-unknown-linux-gnu"),
            None
        );
        Ok(())
    }

    #[test]
    fn an_empty_checksum_placeholder_counts_as_unpublished()
    -> std::result::Result<(), toml::de::Error> {
        let manifest = manifest_with_binaries(
            "\n[binaries.checksums]\nx86_64-unknown-linux-gnu = \"\"\naarch64-apple-darwin = \"  \"\n",
        )?;
        assert_eq!(
            manifest.binaries.checksum_for("x86_64-unknown-linux-gnu"),
            None
        );
        assert_eq!(manifest.binaries.checksum_for("aarch64-apple-darwin"), None);
        Ok(())
    }

    #[test]
    fn a_published_checksum_is_returned_for_its_target_only()
    -> std::result::Result<(), toml::de::Error> {
        let manifest = manifest_with_binaries(
            "\n[binaries.checksums]\nx86_64-unknown-linux-gnu = \" abc123 \"\n",
        )?;
        assert_eq!(
            manifest.binaries.checksum_for("x86_64-unknown-linux-gnu"),
            Some("abc123")
        );
        assert_eq!(manifest.binaries.checksum_for("aarch64-apple-darwin"), None);
        Ok(())
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
                description: String::new(),
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
