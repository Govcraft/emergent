//! Integration tests for `publish_all` and `publish_stream`.
//!
//! These tests start a real Emergent engine, connect SDK primitives,
//! and verify that streamed/batched publishes are received by subscribers.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use emergent_client::{EmergentMessage, EmergentSink, EmergentSource};
use serde_json::json;
use tokio::time::{sleep, timeout};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

// ============================================================================
// Test Harness
// ============================================================================

/// Locate the engine binary in the workspace target directory.
fn engine_binary() -> TestResult<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // sdks/rust -> sdks -> workspace root
    let sdks_dir = manifest
        .parent()
        .ok_or("CARGO_MANIFEST_DIR has no parent")?;
    let workspace_root = sdks_dir.parent().ok_or("sdks dir has no parent")?;

    // Prefer debug (tests run in debug profile)
    let debug_bin = workspace_root.join("target/debug/emergent");
    if debug_bin.exists() {
        return Ok(debug_bin);
    }

    Ok(workspace_root.join("target/release/emergent"))
}

/// A running test engine with automatic cleanup.
struct TestEngine {
    child: Child,
    socket_path: PathBuf,
    _config_dir: tempfile::TempDir,
}

impl TestEngine {
    /// Start an engine with a minimal config and a unique socket.
    async fn start() -> TestResult<Self>
    where
        Self: Sized,
    {
        let config_dir = tempfile::tempdir()?;
        let socket_path = config_dir.path().join("test.sock");
        let log_dir = config_dir.path().join("logs");
        let db_path = config_dir.path().join("events.db");

        let config_content = format!(
            r#"
[engine]
name = "test-engine"
socket_path = "{socket}"
api_port = 0

[event_store]
json_log_dir = "{logs}"
sqlite_path = "{db}"
retention_days = 1
"#,
            socket = socket_path.display(),
            logs = log_dir.display(),
            db = db_path.display(),
        );

        let config_path = config_dir.path().join("test.toml");
        std::fs::write(&config_path, config_content)?;

        let config_str = config_path
            .to_str()
            .ok_or("config path is not valid UTF-8")?;

        let child = Command::new(engine_binary()?)
            .args(["--config", config_str])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        // Wait for socket to appear (engine is ready)
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            if socket_path.exists() {
                // Give the IPC listener a moment to start accepting
                sleep(Duration::from_millis(100)).await;
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }

        assert!(
            socket_path.exists(),
            "engine socket did not appear at {}",
            socket_path.display()
        );

        Ok(Self {
            child,
            socket_path,
            _config_dir: config_dir,
        })
    }

    fn socket(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for TestEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
async fn test_publish_all_received_by_subscriber() -> TestResult<()> {
    let engine = TestEngine::start().await?;
    let socket = engine.socket();

    // Connect sink first so it's subscribed before messages arrive
    let mut sink = EmergentSink::connect_to("test_sink", socket).await?;
    let mut stream = sink.subscribe(["test.batch"]).await?;

    // Connect source
    let source = EmergentSource::connect_to("test_source", socket).await?;

    // Build a batch of messages
    let messages: Vec<EmergentMessage> = (0..5)
        .map(|i| EmergentMessage::new("test.batch").with_payload(json!({"index": i})))
        .collect();

    // Publish all
    let count = source.publish_all(messages).await?;
    assert_eq!(count, 5);

    // Collect from subscriber with timeout
    let mut received = Vec::new();
    let result = timeout(Duration::from_secs(5), async {
        while let Some(msg) = stream.next().await {
            received.push(msg);
            if received.len() == 5 {
                break;
            }
        }
    })
    .await;

    assert!(result.is_ok(), "timed out waiting for messages");
    assert_eq!(received.len(), 5);

    // Verify payloads arrived in order
    for (i, msg) in received.iter().enumerate() {
        assert_eq!(msg.payload["index"], json!(i));
    }

    Ok(())
}

#[tokio::test]
async fn test_publish_stream_received_by_subscriber() -> TestResult<()> {
    let engine = TestEngine::start().await?;
    let socket = engine.socket();

    // Subscribe first
    let mut sink = EmergentSink::connect_to("test_sink", socket).await?;
    let mut stream = sink.subscribe(["test.stream"]).await?;

    // Connect source
    let source = EmergentSource::connect_to("test_source", socket).await?;

    // Create an async stream of messages via a channel
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        for i in 0..3 {
            let msg = EmergentMessage::new("test.stream").with_payload(json!({"seq": i}));
            let _ = tx.send(msg).await;
        }
        // Dropping tx closes the stream
    });

    let rx_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let count = source.publish_stream(rx_stream).await?;
    assert_eq!(count, 3);

    // Collect from subscriber
    let mut received = Vec::new();
    let result = timeout(Duration::from_secs(5), async {
        while let Some(msg) = stream.next().await {
            received.push(msg);
            if received.len() == 3 {
                break;
            }
        }
    })
    .await;

    assert!(result.is_ok(), "timed out waiting for messages");
    assert_eq!(received.len(), 3);

    for (i, msg) in received.iter().enumerate() {
        assert_eq!(msg.payload["seq"], json!(i));
    }

    Ok(())
}

#[tokio::test]
async fn test_publish_all_empty_iterator() -> TestResult<()> {
    let engine = TestEngine::start().await?;
    let socket = engine.socket();

    let source = EmergentSource::connect_to("test_source", socket).await?;

    let count = source.publish_all(std::iter::empty()).await?;
    assert_eq!(count, 0);

    Ok(())
}
