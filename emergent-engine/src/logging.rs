//! Where the engine's own log lines go.
//!
//! By default a running engine logs to
//! `~/.local/share/emergent/<engine.name>/emergent.log`, which suits a desktop
//! where the terminal is closed. A container wants the opposite: its runtime
//! collects stdout, and a file inside the container's filesystem is invisible
//! to `docker logs` and lost with the container. `--log-stdout`, or
//! `EMERGENT_LOG_STDOUT=1` for an image that cannot change its command line,
//! sends the same lines to stdout instead.

/// The environment variable equivalent of `--log-stdout`.
pub const LOG_STDOUT_ENV: &str = "EMERGENT_LOG_STDOUT";

/// Where a running engine writes its log lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogDestination {
    /// `--verbose`: stdout, formatted for a terminal.
    Terminal,
    /// `--log-stdout` or `EMERGENT_LOG_STDOUT`: stdout, with colour only when
    /// stdout is a terminal, so a log collector never receives escape codes.
    Stdout,
    /// The default: the engine's `emergent.log` in its data directory.
    File,
}

/// Whether an environment flag's value turns it on (pure function).
///
/// `1`, `true`, `yes` and `on` in any case turn it on; anything else,
/// including an empty value, leaves it off, so `EMERGENT_LOG_STDOUT=0` means
/// what it says.
#[must_use]
pub fn env_flag_enabled(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Decide where the engine logs (pure function).
///
/// `--verbose` keeps its meaning and wins; otherwise `--log-stdout` or a
/// truthy `EMERGENT_LOG_STDOUT` select stdout, and with neither the engine
/// logs to its file exactly as before.
#[must_use]
pub fn log_destination(verbose: bool, log_stdout: bool, env_value: Option<&str>) -> LogDestination {
    if verbose {
        LogDestination::Terminal
    } else if log_stdout || env_flag_enabled(env_value) {
        LogDestination::Stdout
    } else {
        LogDestination::File
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_unchanged() {
        assert_eq!(log_destination(false, false, None), LogDestination::File);
    }

    #[test]
    fn verbose_still_logs_to_the_terminal() {
        assert_eq!(log_destination(true, false, None), LogDestination::Terminal);
        assert_eq!(
            log_destination(true, true, Some("1")),
            LogDestination::Terminal
        );
    }

    #[test]
    fn the_flag_or_the_variable_selects_stdout() {
        assert_eq!(log_destination(false, true, None), LogDestination::Stdout);
        assert_eq!(
            log_destination(false, false, Some("1")),
            LogDestination::Stdout
        );
        assert_eq!(
            log_destination(false, false, Some("TRUE")),
            LogDestination::Stdout
        );
    }

    #[test]
    fn a_false_or_empty_variable_is_off() {
        for value in ["0", "false", "no", "off", "", "  "] {
            assert_eq!(
                log_destination(false, false, Some(value)),
                LogDestination::File,
                "{value:?}"
            );
        }
    }
}
