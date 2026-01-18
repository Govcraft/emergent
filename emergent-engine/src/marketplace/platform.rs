//! Target platform detection for binary downloads.

use std::fmt;

/// Target platform triple identifier.
///
/// Represents a Rust target triple like "x86_64-unknown-linux-gnu" or "aarch64-apple-darwin".
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TargetPlatform(String);

impl TargetPlatform {
    /// Detect the current platform.
    ///
    /// Returns the Rust target triple for the current platform based on compile-time cfg attributes.
    ///
    /// # Examples
    ///
    /// ```
    /// # use emergent_engine::marketplace::platform::TargetPlatform;
    /// let platform = TargetPlatform::detect();
    /// assert!(!platform.as_str().is_empty());
    /// ```
    pub fn detect() -> Self {
        let triple = format!(
            "{}-{}-{}",
            Self::detect_arch(),
            Self::detect_vendor(),
            Self::detect_os()
        );
        Self(triple)
    }

    /// Get the platform as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn detect_arch() -> &'static str {
        #[cfg(target_arch = "x86_64")]
        return "x86_64";
        #[cfg(target_arch = "aarch64")]
        return "aarch64";
        #[cfg(target_arch = "arm")]
        return "arm";
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "arm")))]
        return "unknown";
    }

    fn detect_vendor() -> &'static str {
        #[cfg(target_vendor = "apple")]
        return "apple";
        #[cfg(target_vendor = "pc")]
        return "unknown";
        #[cfg(not(any(target_vendor = "apple", target_vendor = "pc")))]
        return "unknown";
    }

    fn detect_os() -> &'static str {
        #[cfg(target_os = "linux")]
        return "linux-gnu";
        #[cfg(target_os = "macos")]
        return "darwin";
        #[cfg(target_os = "windows")]
        return "windows-msvc";
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        return "unknown";
    }
}

impl fmt::Display for TargetPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for TargetPlatform {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TargetPlatform {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detection() {
        let platform = TargetPlatform::detect();
        assert!(!platform.as_str().is_empty());
        assert!(platform.as_str().contains('-'));

        // Should have three parts separated by hyphens
        let parts: Vec<&str> = platform.as_str().split('-').collect();
        assert!(parts.len() >= 3, "Platform triple should have at least 3 parts");
    }

    #[test]
    fn test_platform_display() {
        let platform = TargetPlatform::detect();
        let display_str = format!("{platform}");
        assert_eq!(display_str, platform.as_str());
    }

    #[test]
    fn test_platform_from_string() {
        let platform: TargetPlatform = "x86_64-unknown-linux-gnu".into();
        assert_eq!(platform.as_str(), "x86_64-unknown-linux-gnu");

        let platform: TargetPlatform = "x86_64-unknown-linux-gnu".to_string().into();
        assert_eq!(platform.as_str(), "x86_64-unknown-linux-gnu");
    }

    #[test]
    fn test_platform_equality() {
        let p1 = TargetPlatform::from("x86_64-unknown-linux-gnu");
        let p2 = TargetPlatform::from("x86_64-unknown-linux-gnu");
        let p3 = TargetPlatform::from("aarch64-apple-darwin");

        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }

    #[test]
    fn test_platform_clone() {
        let p1 = TargetPlatform::detect();
        let p2 = p1.clone();
        assert_eq!(p1, p2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_linux_platform() {
        let platform = TargetPlatform::detect();
        assert!(platform.as_str().contains("linux"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_platform() {
        let platform = TargetPlatform::detect();
        assert!(platform.as_str().contains("darwin"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_windows_platform() {
        let platform = TargetPlatform::detect();
        assert!(platform.as_str().contains("windows"));
    }
}
