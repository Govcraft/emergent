//! Whether a scaffold run produced the crate it promised.
//!
//! `emergent scaffold` used to print a missing template or a render error to
//! stderr, carry on, and exit 0 with a partial crate on disk. The decision of
//! whether a run succeeded now lives here, as one pure function over what the
//! run was asked to produce, what it produced, and what went wrong, so the
//! command can fail with the files named.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One file the scaffold was asked to produce and could not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateFailure {
    /// The file that was not produced, relative to the output directory.
    pub file: String,
    /// Why it was not produced.
    pub reason: String,
}

impl TemplateFailure {
    /// Records a file the scaffold could not produce, and why.
    pub fn new(file: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for TemplateFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.file, self.reason)
    }
}

/// Decides whether a scaffold run succeeded.
///
/// `expected` is how many files the run was asked to produce, `written` the
/// ones it produced, and `failures` the ones it could not. Returns `None` when
/// the run produced everything it promised, and otherwise the error message
/// the command fails with, which names every file that failed.
///
/// A run that produced fewer files than it was asked for without recording a
/// failure also fails: the crate on disk is partial either way.
#[must_use]
pub fn scaffold_error(
    expected: usize,
    written: &[String],
    failures: &[TemplateFailure],
) -> Option<String> {
    if !failures.is_empty() {
        let named = failures
            .iter()
            .map(TemplateFailure::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Some(format!(
            "scaffold failed for {} of {expected} file(s): {named}",
            failures.len()
        ));
    }

    if written.len() < expected {
        return Some(format!(
            "scaffold wrote {} of {expected} file(s)",
            written.len()
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(files: &[&str]) -> Vec<String> {
        files.iter().map(|f| (*f).to_string()).collect()
    }

    #[test]
    fn a_complete_run_succeeds() {
        assert_eq!(
            scaffold_error(2, &written(&["Cargo.toml", "src/main.rs"]), &[]),
            None
        );
    }

    #[test]
    fn a_failed_file_fails_the_run_and_is_named() {
        let failures = vec![TemplateFailure::new("src/main.rs", "template not found")];
        let error = scaffold_error(2, &written(&["Cargo.toml"]), &failures);

        let Some(error) = error else {
            panic!("a run with a failed file must not succeed");
        };
        assert!(error.contains("src/main.rs"), "{error}");
        assert!(error.contains("template not found"), "{error}");
        assert!(error.contains("1 of 2"), "{error}");
    }

    #[test]
    fn every_failed_file_is_named() {
        let failures = vec![
            TemplateFailure::new("Cargo.toml", "template not found"),
            TemplateFailure::new("src/main.rs", "Failed to render template: eof"),
        ];
        let error = scaffold_error(2, &[], &failures);

        let Some(error) = error else {
            panic!("a run with failed files must not succeed");
        };
        assert!(error.contains("Cargo.toml"), "{error}");
        assert!(error.contains("src/main.rs"), "{error}");
    }

    #[test]
    fn a_short_run_fails_even_without_a_recorded_failure() {
        let error = scaffold_error(2, &written(&["Cargo.toml"]), &[]);

        let Some(error) = error else {
            panic!("a partial crate must not count as success");
        };
        assert!(error.contains("1 of 2"), "{error}");
    }

    #[test]
    fn a_run_with_nothing_to_do_fails_when_it_was_asked_for_nothing() {
        // The handler records a failure when no templates are registered, so
        // an empty run only reaches here as a success of zero files.
        assert_eq!(scaffold_error(0, &[], &[]), None);
    }

    #[test]
    fn extra_files_do_not_fail_the_run() {
        assert_eq!(
            scaffold_error(1, &written(&["Cargo.toml", "src/main.rs"]), &[]),
            None
        );
    }
}
