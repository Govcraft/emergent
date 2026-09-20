//! Watching what the engine does with a fire-and-forget publish.
//!
//! [`EmergentSource::publish`](crate::EmergentSource::publish) and
//! [`EmergentHandler::publish`](crate::EmergentHandler::publish) do not wait for
//! the engine, but the engine answers every publish frame anyway: acton's IPC
//! listener replies `{"status": "delivered"}` once the message reaches the
//! broker's mailbox, and replies with an error when it refuses the frame. The
//! refusals are real losses. The per-connection rate limiter (100 requests per
//! second, burst 50 in acton-reactive 9.3.0) answers `RATE_LIMITED`, a full
//! broker mailbox answers `TARGET_BUSY`, a draining engine answers
//! `SHUTTING_DOWN`, and a frame the broker cannot deserialize answers
//! `SERIALIZATION_ERROR`.
//!
//! `IpcClient::send` registers no pending request, so acton's reader drains all
//! of those replies at `trace!` and `publish` returns `Ok(())` over a message
//! that was never delivered. The watcher here claims them instead. It writes
//! the same frame `send` would (the envelope still has `expects_reply` unset,
//! so the engine still takes its fire-and-forget path) but hands it to
//! `IpcClient::request_with_timeout`, which registers the correlation ID before
//! writing. The reply then lands on the watcher rather than in the drain.
//!
//! The watcher is one task per connection consuming a bounded queue in order,
//! so a hot publisher gets backpressure rather than an unbounded backlog, and
//! messages reach the socket in the order they were published.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use acton_reactive::ipc::{IpcClient, IpcEnvelope};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::warn;

use crate::error::ClientError;
use crate::{Result, types::MessageType};

/// How many publishes may be waiting on the engine before `publish` blocks.
///
/// Bounds the watcher's memory under a publisher that outruns the engine, and
/// turns the overrun into backpressure on the caller.
const WATCH_QUEUE_CAPACITY: usize = 1024;

/// How long the watcher waits for the engine's reply to one publish.
///
/// The engine answers a fire-and-forget frame as soon as the broker's mailbox
/// accepts it, so a wait this long means the connection is in trouble, not that
/// the engine is busy.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Shortest gap between two rejection log lines on one connection.
///
/// A publisher far over the rate limit is rejected thousands of times a second.
/// Every rejection is counted; the log carries the first one immediately and
/// then one line per interval, each naming how many it stands for.
const REJECTION_LOG_INTERVAL_MS: u64 = 1_000;

/// What the engine did with one fire-and-forget publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PublishOutcome {
    /// The engine took the message: it reached the broker's mailbox.
    Accepted,
    /// The engine refused the message. It was not delivered to anyone.
    Rejected(String),
    /// No reply arrived, so delivery is unknown.
    Unanswered(String),
}

/// Classify the engine's reply to one fire-and-forget publish.
///
/// The reply is an `IpcResponse`, flattened here to the three fields that carry
/// the verdict so the decision stays testable without building one.
pub(crate) fn classify_reply(
    success: bool,
    error_code: Option<&str>,
    error: Option<&str>,
) -> PublishOutcome {
    if success {
        PublishOutcome::Accepted
    } else {
        PublishOutcome::Rejected(rejection_reason(error_code, error))
    }
}

/// Render the engine's refusal as one human-readable reason.
///
/// Keeps the machine-readable code when the engine sent one, because that is
/// what tells a rate limit apart from a full mailbox, and falls back to the
/// message alone when it did not.
pub(crate) fn rejection_reason(error_code: Option<&str>, error: Option<&str>) -> String {
    match (error_code, error) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_string(),
        (None, Some(message)) => message.to_string(),
        (None, None) => "engine reported no reason".to_string(),
    }
}

/// Whether a rejection should be logged now.
///
/// `elapsed_since_last_ms` is `None` until the connection logs its first
/// rejection, which always goes out.
pub(crate) const fn should_log_rejection(
    elapsed_since_last_ms: Option<u64>,
    interval_ms: u64,
) -> bool {
    match elapsed_since_last_ms {
        None => true,
        Some(elapsed) => elapsed >= interval_ms,
    }
}

/// What the engine has done with this primitive's fire-and-forget publishes.
///
/// A snapshot, read with
/// [`EmergentSource::publish_stats`](crate::EmergentSource::publish_stats) or
/// [`EmergentHandler::publish_stats`](crate::EmergentHandler::publish_stats).
/// The three counts only cover [`publish`](crate::EmergentSource::publish):
/// [`publish_ack`](crate::EmergentSource::publish_ack) reports its own verdict
/// to the caller and is not counted here.
///
/// The counts lag the calls, because `publish` returns before the engine
/// answers. `accepted + rejected + unanswered` reaches the number of calls once
/// the queue drains.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PublishStats {
    /// Publishes the engine took: the message reached the broker.
    pub accepted: u64,
    /// Publishes the engine refused. Those messages were never delivered.
    pub rejected: u64,
    /// Publishes no reply arrived for, so their delivery is unknown.
    pub unanswered: u64,
}

/// The live counters behind [`PublishStats`].
#[derive(Debug, Default)]
struct PublishCounts {
    accepted: AtomicU64,
    rejected: AtomicU64,
    unanswered: AtomicU64,
}

impl PublishCounts {
    fn snapshot(&self) -> PublishStats {
        PublishStats {
            accepted: self.accepted.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            unanswered: self.unanswered.load(Ordering::Relaxed),
        }
    }
}

/// One item of work for the watcher task.
enum WatchCommand {
    /// Write this publish frame and classify the engine's reply.
    Publish {
        envelope: IpcEnvelope,
        message_type: MessageType,
    },
    /// Answer once everything queued ahead of this has been answered.
    Flush(oneshot::Sender<()>),
    /// Stop once everything queued ahead of this has been answered.
    Shutdown,
}

/// Throttles the rejection log without losing the count.
///
/// A publisher far over the rate limit is rejected faster than any log is worth
/// reading, so only one line per [`REJECTION_LOG_INTERVAL_MS`] goes out and each
/// carries how many rejections it stands for. Whatever is still held back when
/// the watcher goes idle is released by [`drain`](Self::drain), so a burst that
/// starts and ends inside one interval still reports its full count.
struct RejectionLog {
    last_logged: Option<Instant>,
    suppressed: u64,
    last_reason: String,
}

impl RejectionLog {
    fn new() -> Self {
        Self {
            last_logged: None,
            suppressed: 0,
            last_reason: String::new(),
        }
    }

    /// Record one rejection and return how many it stands for when it should be
    /// logged, or `None` when it is folded into a later line.
    fn admit(&mut self, now: Instant, reason: &str) -> Option<u64> {
        let elapsed = self
            .last_logged
            .map(|last| u64::try_from(now.duration_since(last).as_millis()).unwrap_or(u64::MAX));

        if should_log_rejection(elapsed, REJECTION_LOG_INTERVAL_MS) {
            let stood_for = self.suppressed + 1;
            self.suppressed = 0;
            self.last_logged = Some(now);
            Some(stood_for)
        } else {
            self.suppressed += 1;
            reason.clone_into(&mut self.last_reason);
            None
        }
    }

    /// Release the rejections held back since the last line, if any.
    ///
    /// Returns how many, and the reason the last of them gave.
    fn drain(&mut self) -> Option<(u64, String)> {
        if self.suppressed == 0 {
            return None;
        }
        let held = self.suppressed;
        self.suppressed = 0;
        Some((held, std::mem::take(&mut self.last_reason)))
    }
}

/// Claims the engine's replies to a connection's fire-and-forget publishes.
///
/// Cloneable, because `EmergentHandler` is: every clone shares one watcher
/// task, one queue and one set of counters.
#[derive(Clone)]
pub(crate) struct PublishWatcher {
    tx: mpsc::Sender<WatchCommand>,
    counts: Arc<PublishCounts>,
    task: Arc<std::sync::Mutex<Option<JoinHandle<()>>>>,
}

impl PublishWatcher {
    /// Start watching `client`'s publishes on behalf of the primitive `name`.
    pub(crate) fn spawn(client: Arc<IpcClient>, name: String) -> Self {
        let (tx, rx) = mpsc::channel(WATCH_QUEUE_CAPACITY);
        let counts = Arc::new(PublishCounts::default());
        let task = tokio::spawn(run(client, name, rx, Arc::clone(&counts)));

        Self {
            tx,
            counts,
            task: Arc::new(std::sync::Mutex::new(Some(task))),
        }
    }

    /// Queue one publish frame for writing, waiting only if the queue is full.
    pub(crate) async fn publish(
        &self,
        envelope: IpcEnvelope,
        message_type: MessageType,
    ) -> Result<()> {
        self.tx
            .send(WatchCommand::Publish {
                envelope,
                message_type,
            })
            .await
            .map_err(|_| {
                ClientError::ConnectionFailed("publish failed: connection closed".to_string())
            })
    }

    /// Wait until every publish queued so far has been answered.
    ///
    /// `publish_ack` writes its own frame straight to the client, so it has to
    /// wait for the queue to empty first or it would overtake the publishes
    /// that came before it.
    pub(crate) async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(WatchCommand::Flush(tx)).await.is_ok() {
            let _ = rx.await;
        }
    }

    /// Read the counters.
    pub(crate) fn stats(&self) -> PublishStats {
        self.counts.snapshot()
    }

    /// Answer every queued publish, then stop the task.
    ///
    /// Queued frames are written and classified first, so a primitive that
    /// disconnects right after publishing still learns what happened to the
    /// last messages.
    pub(crate) async fn shutdown(&self) {
        let _ = self.tx.send(WatchCommand::Shutdown).await;

        let task = match self.task.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };

        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

/// The watcher task: one publish at a time, in the order they were published.
async fn run(
    client: Arc<IpcClient>,
    name: String,
    mut rx: mpsc::Receiver<WatchCommand>,
    counts: Arc<PublishCounts>,
) {
    let mut rejection_log = RejectionLog::new();

    while let Some(command) = rx.recv().await {
        let (envelope, message_type) = match command {
            WatchCommand::Publish {
                envelope,
                message_type,
            } => (envelope, message_type),
            WatchCommand::Flush(reply) => {
                report_held_rejections(&name, &mut rejection_log);
                let _ = reply.send(());
                continue;
            }
            WatchCommand::Shutdown => break,
        };

        let outcome = match client.request_with_timeout(envelope, REPLY_TIMEOUT).await {
            Ok(response) => classify_reply(
                response.success,
                response.error_code.as_deref(),
                response.error.as_deref(),
            ),
            Err(e) => PublishOutcome::Unanswered(e.to_string()),
        };

        match outcome {
            PublishOutcome::Accepted => {
                counts.accepted.fetch_add(1, Ordering::Relaxed);
            }
            PublishOutcome::Rejected(reason) => {
                counts.rejected.fetch_add(1, Ordering::Relaxed);
                if let Some(stood_for) = rejection_log.admit(Instant::now(), &reason) {
                    warn!(
                        primitive.name = %name,
                        message.r#type = %message_type,
                        reason = %reason,
                        rejections = stood_for,
                        "engine rejected a published message; it was not delivered"
                    );
                }
            }
            PublishOutcome::Unanswered(detail) => {
                counts.unanswered.fetch_add(1, Ordering::Relaxed);
                if let Some(stood_for) = rejection_log.admit(Instant::now(), &detail) {
                    warn!(
                        primitive.name = %name,
                        message.r#type = %message_type,
                        reason = %detail,
                        rejections = stood_for,
                        "no engine reply for a published message; delivery is unknown"
                    );
                }
            }
        }

        // Nothing left to answer: say what the throttle has been holding back
        // rather than wait for a rejection that may never come.
        if rx.is_empty() {
            report_held_rejections(&name, &mut rejection_log);
        }
    }

    report_held_rejections(&name, &mut rejection_log);
}

/// Log the rejections the throttle held back, if any.
fn report_held_rejections(name: &str, rejection_log: &mut RejectionLog) {
    if let Some((held, reason)) = rejection_log.drain() {
        warn!(
            primitive.name = %name,
            reason = %reason,
            rejections = held,
            "further published messages were rejected; they were not delivered"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_reply_table() {
        let cases = [
            (true, None, None, PublishOutcome::Accepted),
            (
                true,
                Some("RATE_LIMITED"),
                Some("ignored"),
                PublishOutcome::Accepted,
            ),
            (
                false,
                Some("RATE_LIMITED"),
                Some("Rate limit exceeded, retry after 10ms"),
                PublishOutcome::Rejected(
                    "RATE_LIMITED: Rate limit exceeded, retry after 10ms".to_string(),
                ),
            ),
            (
                false,
                Some("TARGET_BUSY"),
                None,
                PublishOutcome::Rejected("TARGET_BUSY".to_string()),
            ),
            (
                false,
                None,
                Some("broker is gone"),
                PublishOutcome::Rejected("broker is gone".to_string()),
            ),
            (
                false,
                None,
                None,
                PublishOutcome::Rejected("engine reported no reason".to_string()),
            ),
        ];

        for (success, code, error, expected) in cases {
            assert_eq!(
                classify_reply(success, code, error),
                expected,
                "success={success} code={code:?} error={error:?}"
            );
        }
    }

    #[test]
    fn rejection_reason_table() {
        assert_eq!(
            rejection_reason(Some("SHUTTING_DOWN"), Some("Server is shutting down")),
            "SHUTTING_DOWN: Server is shutting down"
        );
        assert_eq!(rejection_reason(Some("TARGET_BUSY"), None), "TARGET_BUSY");
        assert_eq!(rejection_reason(None, Some("no route")), "no route");
        assert_eq!(rejection_reason(None, None), "engine reported no reason");
    }

    #[test]
    fn should_log_rejection_table() {
        let cases = [
            (None, 1_000, true),
            (Some(0), 1_000, false),
            (Some(999), 1_000, false),
            (Some(1_000), 1_000, true),
            (Some(60_000), 1_000, true),
            (Some(0), 0, true),
        ];

        for (elapsed, interval, expected) in cases {
            assert_eq!(
                should_log_rejection(elapsed, interval),
                expected,
                "elapsed={elapsed:?} interval={interval}"
            );
        }
    }

    #[test]
    fn rejection_log_carries_the_suppressed_count() {
        let mut log = RejectionLog::new();
        let start = Instant::now();

        assert_eq!(
            log.admit(start, "RATE_LIMITED"),
            Some(1),
            "the first rejection always logs"
        );
        assert_eq!(log.admit(start, "RATE_LIMITED"), None);
        assert_eq!(log.admit(start, "TARGET_BUSY"), None);

        let later = start + Duration::from_millis(REJECTION_LOG_INTERVAL_MS);
        assert_eq!(
            log.admit(later, "RATE_LIMITED"),
            Some(3),
            "the next line stands for the two it swallowed and itself"
        );
        assert_eq!(log.admit(later, "RATE_LIMITED"), None);
    }

    #[test]
    fn draining_releases_what_the_throttle_held() {
        let mut log = RejectionLog::new();
        let start = Instant::now();

        assert_eq!(log.drain(), None, "nothing held before the first rejection");
        assert_eq!(log.admit(start, "RATE_LIMITED"), Some(1));
        assert_eq!(log.drain(), None, "a logged rejection holds nothing back");

        assert_eq!(log.admit(start, "RATE_LIMITED"), None);
        assert_eq!(log.admit(start, "TARGET_BUSY"), None);
        assert_eq!(
            log.drain(),
            Some((2, "TARGET_BUSY".to_string())),
            "the tail carries the count and the last reason"
        );
        assert_eq!(log.drain(), None, "draining twice reports nothing twice");
    }

    #[test]
    fn publish_counts_snapshot_is_the_three_totals() {
        let counts = PublishCounts::default();
        assert_eq!(counts.snapshot(), PublishStats::default());

        counts.accepted.fetch_add(7, Ordering::Relaxed);
        counts.rejected.fetch_add(2, Ordering::Relaxed);
        counts.unanswered.fetch_add(1, Ordering::Relaxed);

        assert_eq!(
            counts.snapshot(),
            PublishStats {
                accepted: 7,
                rejected: 2,
                unanswered: 1,
            }
        );
    }
}
