//! Event retention: what `[event_store].retention_days` actually removes.
//!
//! Retention is enforced in two places, the SQLite store and the JSON log
//! directory, and both decisions are pure:
//!
//! - [`sqlite_cutoff_ms`] turns "now" and a window into the timestamp below
//!   which rows are deleted.
//! - [`expired_log_files`] turns today's UTC date and the same window into the
//!   `events-YYYY-MM-DD.jsonl` file names that are wholly outside it.
//!
//! A window of `0` days disables pruning entirely: nothing is deleted and the
//! stores keep every event. Both pure functions report that as `None`.
//!
//! [`prune`] is the only impure part. It applies those decisions and reports
//! what it removed so the caller can log it.

use crate::event_store::SqliteEventStore;
use chrono::{DateTime, Days, NaiveDate, Utc};
use std::path::{Path, PathBuf};

/// Milliseconds in a day.
const MILLIS_PER_DAY: u64 = 24 * 60 * 60 * 1000;

/// Prefix of a rotated JSON event log file.
const LOG_FILE_PREFIX: &str = "events-";

/// Suffix of a rotated JSON event log file.
const LOG_FILE_SUFFIX: &str = ".jsonl";

/// What a single prune pass removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneReport {
    /// Rows deleted from the SQLite event store.
    pub events_deleted: u64,

    /// JSON log files that were deleted.
    pub files_deleted: Vec<PathBuf>,

    /// Failures that did not stop the pass, rendered for logging.
    pub errors: Vec<String>,
}

impl PruneReport {
    /// Returns whether the pass removed nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events_deleted == 0 && self.files_deleted.is_empty()
    }
}

/// Returns whether a retention window prunes anything (pure function).
///
/// `0` days means "keep everything", so pruning is disabled.
#[must_use]
pub const fn is_pruning_enabled(retention_days: u32) -> bool {
    retention_days != 0
}

/// Compute the SQLite cutoff timestamp for a retention window (pure function).
///
/// Events with `timestamp_ms` strictly below the returned value are outside the
/// window. Returns `None` when pruning is disabled, and saturates at `0` rather
/// than wrapping when the window is longer than the clock.
///
/// # Examples
///
/// ```
/// use emergent_engine::retention::sqlite_cutoff_ms;
///
/// let day = 24 * 60 * 60 * 1000;
/// assert_eq!(sqlite_cutoff_ms(10 * day, 3), Some(7 * day));
/// assert_eq!(sqlite_cutoff_ms(10 * day, 0), None);
/// ```
#[must_use]
pub const fn sqlite_cutoff_ms(now_ms: u64, retention_days: u32) -> Option<u64> {
    if !is_pruning_enabled(retention_days) {
        return None;
    }
    let window_ms = (retention_days as u64).saturating_mul(MILLIS_PER_DAY);
    Some(now_ms.saturating_sub(window_ms))
}

/// Compute the oldest log date still inside the retention window (pure function).
///
/// A file dated before the returned date holds only events the window has
/// dropped. Returns `None` when pruning is disabled or the subtraction would
/// leave the representable range.
///
/// # Examples
///
/// ```
/// use chrono::NaiveDate;
/// use emergent_engine::retention::oldest_retained_log_date;
///
/// let today = NaiveDate::from_ymd_opt(2026, 9, 19).unwrap_or_default();
/// let kept = NaiveDate::from_ymd_opt(2026, 9, 12).unwrap_or_default();
/// assert_eq!(oldest_retained_log_date(today, 7), Some(kept));
/// assert_eq!(oldest_retained_log_date(today, 0), None);
/// ```
#[must_use]
pub fn oldest_retained_log_date(today: NaiveDate, retention_days: u32) -> Option<NaiveDate> {
    if !is_pruning_enabled(retention_days) {
        return None;
    }
    today.checked_sub_days(Days::new(u64::from(retention_days)))
}

/// Parse the date out of a rotated log file name (pure function).
///
/// Returns `None` for any name that is not exactly `events-YYYY-MM-DD.jsonl`,
/// which keeps unrelated files in the log directory safe from deletion.
///
/// # Examples
///
/// ```
/// use chrono::NaiveDate;
/// use emergent_engine::retention::parse_log_file_date;
///
/// assert_eq!(
///     parse_log_file_date("events-2026-09-19.jsonl"),
///     NaiveDate::from_ymd_opt(2026, 9, 19)
/// );
/// assert_eq!(parse_log_file_date("emergent.log"), None);
/// ```
#[must_use]
pub fn parse_log_file_date(file_name: &str) -> Option<NaiveDate> {
    let rest = file_name.strip_prefix(LOG_FILE_PREFIX)?;
    let date = rest.strip_suffix(LOG_FILE_SUFFIX)?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

/// Select the log file names that fall outside the retention window (pure function).
///
/// A file is expired when its date is strictly older than the oldest retained
/// date, so the boundary day is kept. Names that are not rotated event logs are
/// never selected, and an empty result is returned when pruning is disabled.
#[must_use]
pub fn expired_log_files<'a, I>(
    file_names: I,
    today: NaiveDate,
    retention_days: u32,
) -> Vec<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let Some(oldest_retained) = oldest_retained_log_date(today, retention_days) else {
        return Vec::new();
    };

    file_names
        .into_iter()
        .filter(|name| parse_log_file_date(name).is_some_and(|date| date < oldest_retained))
        .collect()
}

/// Delete events and log files that fall outside the retention window.
///
/// This is the impure edge: it reads the log directory, deletes files, and asks
/// the SQLite store to delete rows. Failures are collected rather than raised,
/// because a prune pass must never take the engine down.
pub fn prune(
    sqlite: Option<&SqliteEventStore>,
    log_dir: &Path,
    retention_days: u32,
    now: DateTime<Utc>,
) -> PruneReport {
    let mut report = PruneReport::default();

    if !is_pruning_enabled(retention_days) {
        return report;
    }

    let now_ms = u64::try_from(now.timestamp_millis()).unwrap_or(0);
    if let (Some(store), Some(cutoff_ms)) = (sqlite, sqlite_cutoff_ms(now_ms, retention_days)) {
        match store.delete_before(cutoff_ms) {
            Ok(deleted) => report.events_deleted = deleted,
            Err(error) => report.errors.push(format!("SQLite prune failed: {error}")),
        }
    }

    prune_log_dir(log_dir, now.date_naive(), retention_days, &mut report);

    report
}

/// Delete expired JSON log files, recording outcomes in the report.
fn prune_log_dir(log_dir: &Path, today: NaiveDate, retention_days: u32, report: &mut PruneReport) {
    let entries = match std::fs::read_dir(log_dir) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                report.errors.push(format!(
                    "Reading log directory {} failed: {error}",
                    log_dir.display()
                ));
            }
            return;
        }
    };

    let mut names = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => names.push(entry.file_name().to_string_lossy().into_owned()),
            Err(error) => report
                .errors
                .push(format!("Reading a log directory entry failed: {error}")),
        }
    }

    let expired = expired_log_files(names.iter().map(String::as_str), today, retention_days);
    for name in expired {
        let path = log_dir.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => report.files_deleted.push(path),
            Err(error) => report
                .errors
                .push(format!("Deleting {} failed: {error}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: u64 = MILLIS_PER_DAY;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap_or_default()
    }

    #[test]
    fn zero_days_disables_pruning() {
        assert!(!is_pruning_enabled(0));
        assert_eq!(sqlite_cutoff_ms(100 * DAY_MS, 0), None);
        assert_eq!(oldest_retained_log_date(date(2026, 9, 19), 0), None);
    }

    #[test]
    fn cutoff_subtracts_whole_days() {
        assert_eq!(sqlite_cutoff_ms(30 * DAY_MS, 1), Some(29 * DAY_MS));
        assert_eq!(sqlite_cutoff_ms(30 * DAY_MS, 30), Some(0));
    }

    #[test]
    fn cutoff_saturates_instead_of_wrapping() {
        assert_eq!(sqlite_cutoff_ms(DAY_MS, 365), Some(0));
        assert_eq!(sqlite_cutoff_ms(0, u32::MAX), Some(0));
    }

    #[test]
    fn oldest_retained_date_crosses_a_month_boundary() {
        assert_eq!(
            oldest_retained_log_date(date(2026, 3, 2), 5),
            Some(date(2026, 2, 25))
        );
    }

    #[test]
    fn log_file_dates_parse_only_for_rotated_event_logs() {
        assert_eq!(
            parse_log_file_date("events-2026-09-19.jsonl"),
            Some(date(2026, 9, 19))
        );
        assert_eq!(parse_log_file_date("events-2026-09-19.jsonl.gz"), None);
        assert_eq!(parse_log_file_date("events-not-a-date.jsonl"), None);
        assert_eq!(parse_log_file_date("events-2026-13-40.jsonl"), None);
        assert_eq!(parse_log_file_date("emergent.log"), None);
        assert_eq!(parse_log_file_date("events.db"), None);
    }

    #[test]
    fn expired_files_exclude_the_boundary_day_and_today() {
        let today = date(2026, 9, 19);
        let names = vec![
            "events-2026-09-11.jsonl",
            "events-2026-09-12.jsonl",
            "events-2026-09-19.jsonl",
        ];

        let expired = expired_log_files(names, today, 7);

        assert_eq!(expired, vec!["events-2026-09-11.jsonl"]);
    }

    #[test]
    fn expired_files_ignore_unrelated_names() {
        let today = date(2026, 9, 19);
        let names = vec!["emergent.log", "events.db", "notes.txt"];

        assert!(expired_log_files(names, today, 1).is_empty());
    }

    #[test]
    fn expired_files_are_empty_when_pruning_is_disabled() {
        let today = date(2026, 9, 19);
        let names = vec!["events-2000-01-01.jsonl"];

        assert!(expired_log_files(names, today, 0).is_empty());
    }

    #[test]
    fn prune_deletes_only_files_outside_the_window() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::TempDir::new()?;
        let old = dir.path().join("events-2026-09-01.jsonl");
        let recent = dir.path().join("events-2026-09-18.jsonl");
        let unrelated = dir.path().join("emergent.log");
        std::fs::write(&old, "{}\n")?;
        std::fs::write(&recent, "{}\n")?;
        std::fs::write(&unrelated, "noise\n")?;

        let now = date(2026, 9, 19)
            .and_hms_opt(12, 0, 0)
            .map(|naive| naive.and_utc())
            .ok_or("failed to build a timestamp")?;

        let report = prune(None, dir.path(), 7, now);

        assert_eq!(report.files_deleted, vec![old.clone()]);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(unrelated.exists());
        Ok(())
    }

    #[test]
    fn prune_does_nothing_when_disabled() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::TempDir::new()?;
        let old = dir.path().join("events-2000-01-01.jsonl");
        std::fs::write(&old, "{}\n")?;

        let now = date(2026, 9, 19)
            .and_hms_opt(12, 0, 0)
            .map(|naive| naive.and_utc())
            .ok_or("failed to build a timestamp")?;

        let report = prune(None, dir.path(), 0, now);

        assert!(report.is_empty());
        assert!(old.exists());
        Ok(())
    }

    #[test]
    fn prune_deletes_only_sqlite_rows_outside_the_window() -> Result<(), Box<dyn std::error::Error>>
    {
        use crate::event_store::EventStore;
        use crate::messages::{EmergentMessage, MessageId, Timestamp};

        let dir = tempfile::TempDir::new()?;
        let store = SqliteEventStore::in_memory()?;

        let now = date(2026, 9, 19)
            .and_hms_opt(12, 0, 0)
            .map(|naive| naive.and_utc())
            .ok_or("failed to build a timestamp")?;
        let now_ms = u64::try_from(now.timestamp_millis())?;

        let stale = EmergentMessage::new_with_id_and_timestamp(
            "timer.tick",
            MessageId::new(),
            Timestamp::from_millis(now_ms - 8 * DAY_MS),
        );
        let fresh = EmergentMessage::new_with_id_and_timestamp(
            "timer.tick",
            MessageId::new(),
            Timestamp::from_millis(now_ms - 2 * DAY_MS),
        );
        store.store(&stale)?;
        store.store(&fresh)?;
        assert_eq!(store.count()?, 2);

        let report = prune(Some(&store), dir.path(), 7, now);

        assert_eq!(report.events_deleted, 1);
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        assert_eq!(store.count()?, 1);
        Ok(())
    }

    #[test]
    fn prune_tolerates_a_missing_log_directory() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::TempDir::new()?;
        let missing = dir.path().join("does-not-exist");

        let now = date(2026, 9, 19)
            .and_hms_opt(12, 0, 0)
            .map(|naive| naive.and_utc())
            .ok_or("failed to build a timestamp")?;

        let report = prune(None, &missing, 7, now);

        assert!(report.is_empty());
        assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
        Ok(())
    }
}
