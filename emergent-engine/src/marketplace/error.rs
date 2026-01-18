//! Error types for marketplace operations.

use std::fmt;

/// Errors that can occur during marketplace operations.
#[derive(Debug)]
pub enum MarketplaceError {
    /// Error accessing XDG directories
    XdgDirectories { message: String },

    /// Error performing I/O operations
    Io { source: std::io::Error },

    /// Error downloading files
    Download {
        url: String,
        source: reqwest::Error,
    },

    /// Error parsing TOML manifests
    TomlParse {
        path: String,
        source: toml::de::Error,
    },

    /// Error serializing TOML
    TomlSerialize {
        path: String,
        source: toml::ser::Error,
    },

    /// Error parsing JSON
    JsonParse { source: serde_json::Error },

    /// Primitive not found in registry
    PrimitiveNotFound { name: String },

    /// Version not found for primitive
    VersionNotFound { name: String, version: String },

    /// Platform not supported
    PlatformNotSupported { platform: String, primitive: String },

    /// Checksum verification failed
    ChecksumMismatch { expected: String, actual: String },

    /// Archive extraction failed
    ExtractionFailed {
        path: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Primitive already installed
    AlreadyInstalled { name: String, version: String },

    /// Invalid manifest format
    InvalidManifest { reason: String },

    /// Git operation failed
    GitError { message: String },
}

impl fmt::Display for MarketplaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::XdgDirectories { message } => write!(f, "XDG directory error: {message}"),
            Self::Io { source } => write!(f, "I/O error: {source}"),
            Self::Download { url, source } => write!(f, "Failed to download {url}: {source}"),
            Self::TomlParse { path, source } => {
                write!(f, "Failed to parse TOML at {path}: {source}")
            }
            Self::TomlSerialize { path, source } => {
                write!(f, "Failed to serialize TOML at {path}: {source}")
            }
            Self::JsonParse { source } => write!(f, "Failed to parse JSON: {source}"),
            Self::PrimitiveNotFound { name } => {
                write!(f, "Primitive '{name}' not found in registry")
            }
            Self::VersionNotFound { name, version } => {
                write!(f, "Version '{version}' not found for primitive '{name}'")
            }
            Self::PlatformNotSupported { platform, primitive } => {
                write!(
                    f,
                    "Platform '{platform}' not supported for primitive '{primitive}'"
                )
            }
            Self::ChecksumMismatch { expected, actual } => {
                write!(f, "Checksum mismatch: expected {expected}, got {actual}")
            }
            Self::ExtractionFailed { path, source } => {
                write!(f, "Failed to extract archive at {path}: {source}")
            }
            Self::AlreadyInstalled { name, version } => {
                write!(f, "Primitive '{name}' version '{version}' is already installed")
            }
            Self::InvalidManifest { reason } => write!(f, "Invalid manifest: {reason}"),
            Self::GitError { message } => write!(f, "Git error: {message}"),
        }
    }
}

impl std::error::Error for MarketplaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source } => Some(source),
            Self::Download { source, .. } => Some(source),
            Self::TomlParse { source, .. } => Some(source),
            Self::TomlSerialize { source, .. } => Some(source),
            Self::JsonParse { source } => Some(source),
            Self::ExtractionFailed { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<std::io::Error> for MarketplaceError {
    fn from(source: std::io::Error) -> Self {
        Self::Io { source }
    }
}

impl From<toml::de::Error> for MarketplaceError {
    fn from(source: toml::de::Error) -> Self {
        Self::TomlParse {
            path: "<unknown>".to_string(),
            source,
        }
    }
}

impl From<serde_json::Error> for MarketplaceError {
    fn from(source: serde_json::Error) -> Self {
        Self::JsonParse { source }
    }
}

impl From<reqwest::Error> for MarketplaceError {
    fn from(source: reqwest::Error) -> Self {
        Self::Download {
            url: "<unknown>".to_string(),
            source,
        }
    }
}

/// Result type for marketplace operations.
pub type Result<T> = std::result::Result<T, MarketplaceError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = MarketplaceError::PrimitiveNotFound {
            name: "test".to_string(),
        };
        assert_eq!(err.to_string(), "Primitive 'test' not found in registry");

        let err = MarketplaceError::VersionNotFound {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Version '1.0.0' not found for primitive 'test'"
        );

        let err = MarketplaceError::PlatformNotSupported {
            platform: "x86_64-unknown-linux-gnu".to_string(),
            primitive: "test".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Platform 'x86_64-unknown-linux-gnu' not supported for primitive 'test'"
        );
    }

    #[test]
    fn test_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: MarketplaceError = io_err.into();
        assert!(matches!(err, MarketplaceError::Io { .. }));
    }

    #[test]
    fn test_error_from_toml() {
        let toml_str = "invalid toml [[[";
        if let Err(toml_err) = toml::from_str::<toml::Value>(toml_str) {
            let err: MarketplaceError = toml_err.into();
            assert!(matches!(err, MarketplaceError::TomlParse { .. }));
        } else {
            panic!("Should fail to parse invalid TOML");
        }
    }

    #[test]
    fn test_error_from_json() {
        if let Err(json_err) = serde_json::from_str::<serde_json::Value>("invalid json {{{") {
            let err: MarketplaceError = json_err.into();
            assert!(matches!(err, MarketplaceError::JsonParse { .. }));
        } else {
            panic!("Should fail to parse invalid JSON");
        }
    }
}
