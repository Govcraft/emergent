//! The SDK requirement a scaffolded primitive declares.
//!
//! A generated primitive depends on the Emergent SDK, and that dependency has
//! to name a version the SDK actually published. Hard-coding it in each
//! template lets it rot: the Rust templates asked for `emergent-client = "0.3"`
//! long after the SDK reached 0.13, so every scaffolded crate failed to build.
//!
//! [`SDK_VERSION`] is therefore taken from the `emergent-client` crate the
//! engine itself links against, and the templates render their requirement
//! from it. The engine crate's own `CARGO_PKG_VERSION` is a different number
//! and would be the wrong source. The Python and TypeScript SDKs ship in
//! lockstep with the Rust one, which `tests/scaffold_sdk_version.rs` checks.

use crate::scaffold::messages::Language;

/// The Emergent SDK version this engine was built against.
pub const SDK_VERSION: &str = emergent_client::VERSION;

/// Splits a plain `MAJOR.MINOR.PATCH` release into its `MAJOR.MINOR` prefix.
///
/// Returns `None` for anything else, including a pre-release such as
/// `0.14.0-rc.1`, whose compatible range cannot be written by truncation.
fn release_major_minor(version: &str) -> Option<&str> {
    let mut parts = version.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next()?;

    if parts.next().is_some() {
        return None;
    }

    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !(numeric(major) && numeric(minor) && numeric(patch)) {
        return None;
    }

    Some(&version[..major.len() + 1 + minor.len()])
}

/// Renders the Cargo requirement for an SDK version.
///
/// A release becomes its `MAJOR.MINOR` prefix, which Cargo reads as the
/// compatible range for that release: `0.13.1` gives `0.13`, accepting every
/// `0.13.x`. A pre-release is used verbatim, because Cargo only matches a
/// pre-release against a requirement that names one.
#[must_use]
pub fn cargo_requirement(version: &str) -> String {
    release_major_minor(version).unwrap_or(version).to_string()
}

/// Renders the PEP 440 requirement clause for an SDK version.
///
/// A release becomes `~=MAJOR.MINOR.PATCH`, the compatible-release clause that
/// accepts later patches of the same minor. A pre-release is pinned exactly,
/// since PEP 440 spells pre-releases differently from semver.
#[must_use]
pub fn python_requirement(version: &str) -> String {
    if release_major_minor(version).is_some() {
        format!("~={version}")
    } else {
        format!("=={version}")
    }
}

/// Renders the JSR requirement for an SDK version.
///
/// A release becomes `^MAJOR.MINOR.PATCH`, so a generated primitive keeps
/// resolving within the SDK line it was generated for instead of silently
/// following the next breaking release. A pre-release is pinned exactly.
#[must_use]
pub fn jsr_requirement(version: &str) -> String {
    if release_major_minor(version).is_some() {
        format!("^{version}")
    } else {
        version.to_string()
    }
}

/// Renders the SDK requirement a template of `language` should declare.
#[must_use]
pub fn sdk_requirement(language: Language, version: &str) -> String {
    match language {
        Language::Rust => cargo_requirement(version),
        Language::Python => python_requirement(version),
        Language::TypeScript => jsr_requirement(version),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_requirement_keeps_the_compatible_prefix_of_a_release() {
        assert_eq!(cargo_requirement("0.13.1"), "0.13");
        assert_eq!(cargo_requirement("0.13.0"), "0.13");
        assert_eq!(cargo_requirement("1.2.3"), "1.2");
        assert_eq!(cargo_requirement("10.20.30"), "10.20");
    }

    #[test]
    fn cargo_requirement_keeps_a_pre_release_whole() {
        assert_eq!(cargo_requirement("0.14.0-rc.1"), "0.14.0-rc.1");
        assert_eq!(cargo_requirement("1.0.0-alpha"), "1.0.0-alpha");
    }

    #[test]
    fn cargo_requirement_leaves_an_unrecognised_version_alone() {
        assert_eq!(cargo_requirement("0.13"), "0.13");
        assert_eq!(cargo_requirement("nightly"), "nightly");
        assert_eq!(cargo_requirement("0.13.1.2"), "0.13.1.2");
        assert_eq!(cargo_requirement("0.x.1"), "0.x.1");
    }

    #[test]
    fn python_requirement_uses_the_compatible_release_clause() {
        assert_eq!(python_requirement("0.13.1"), "~=0.13.1");
        assert_eq!(python_requirement("1.2.3"), "~=1.2.3");
        assert_eq!(python_requirement("0.14.0-rc.1"), "==0.14.0-rc.1");
    }

    #[test]
    fn jsr_requirement_uses_a_caret_range() {
        assert_eq!(jsr_requirement("0.13.1"), "^0.13.1");
        assert_eq!(jsr_requirement("1.2.3"), "^1.2.3");
        assert_eq!(jsr_requirement("0.14.0-rc.1"), "0.14.0-rc.1");
    }

    #[test]
    fn sdk_requirement_picks_the_syntax_of_the_language() {
        assert_eq!(sdk_requirement(Language::Rust, "0.13.1"), "0.13");
        assert_eq!(sdk_requirement(Language::Python, "0.13.1"), "~=0.13.1");
        assert_eq!(sdk_requirement(Language::TypeScript, "0.13.1"), "^0.13.1");
    }

    #[test]
    fn the_linked_sdk_version_is_a_release() {
        assert!(
            release_major_minor(SDK_VERSION).is_some(),
            "the linked SDK version {SDK_VERSION} is not a plain release, so the \
             rendered requirements fall back to an exact pin"
        );
    }
}
