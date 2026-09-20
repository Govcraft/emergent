//! What a second `subscribe` on one Handler or Sink does.
//!
//! A Rust client hands its push channel to the first stream, so there is never
//! a second stream to register and the first can never be orphaned. The second
//! call is refused. These tests pin that the refusal comes before anything is
//! sent: the engine must not start delivering the refused topics to the first
//! stream, which is what happened while the check sat after the request
//! (Govcraft/emergent#86).

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use emergent_client::{
    ClientError, EmergentHandler, EmergentMessage, EmergentSink, EmergentSource, MessageStream,
};
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
        // The SDK installs its file subscriber only when the process has
        // none, so installing a test subscriber first keeps primitive logs
        // out of the user's data directory.
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();

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
            // The engine writes its own log under the XDG data directory.
            .env("XDG_DATA_HOME", config_dir.path())
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

/// Publish three of each topic, then collect what `stream` yields in a second.
async fn topics_delivered(socket: &Path, stream: &mut MessageStream) -> TestResult<Vec<String>> {
    let source = EmergentSource::connect_to("second_subscribe_source", socket).await?;
    for n in 0..3 {
        for topic in ["rs86.a", "rs86.b"] {
            source
                .publish(EmergentMessage::new(topic).with_payload(json!({"n": n})))
                .await?;
        }
    }

    let mut delivered = Vec::new();
    while let Ok(Some(msg)) = timeout(Duration::from_secs(1), stream.next()).await {
        delivered.push(msg.message_type.to_string());
    }
    Ok(delivered)
}

#[tokio::test]
async fn a_second_sink_subscribe_is_refused_and_changes_nothing() -> TestResult {
    let engine = TestEngine::start().await?;
    let mut sink = EmergentSink::connect_to("second_subscribe_sink", engine.socket()).await?;
    let mut first = sink.subscribe(["rs86.a"]).await?;

    let second = sink.subscribe(["rs86.b"]).await;

    assert!(
        matches!(second, Err(ClientError::SubscriptionFailed(_))),
        "the second subscribe was not refused"
    );
    assert_eq!(sink.subscribed_types(), ["rs86.a"]);
    let delivered = topics_delivered(engine.socket(), &mut first).await?;
    assert_eq!(delivered, ["rs86.a", "rs86.a", "rs86.a"]);
    Ok(())
}

#[tokio::test]
async fn a_second_handler_subscribe_is_refused_and_changes_nothing() -> TestResult {
    let engine = TestEngine::start().await?;
    let mut handler =
        EmergentHandler::connect_to("second_subscribe_handler", engine.socket()).await?;
    let mut first = handler.subscribe(["rs86.a"]).await?;

    let second = handler.subscribe(["rs86.b"]).await;

    assert!(
        matches!(second, Err(ClientError::SubscriptionFailed(_))),
        "the second subscribe was not refused"
    );
    assert_eq!(handler.subscribed_types(), ["rs86.a"]);
    let delivered = topics_delivered(engine.socket(), &mut first).await?;
    assert_eq!(delivered, ["rs86.a", "rs86.a", "rs86.a"]);
    Ok(())
}
