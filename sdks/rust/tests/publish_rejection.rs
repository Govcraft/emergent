//! Integration test for what `publish` reports when the engine refuses.
//!
//! The IPC connection is rate limited to 100 messages per second with a burst
//! of 50, so a source in a tight loop loses most of what it publishes. Before
//! 0.14.0 every one of those calls returned `Ok(())` and the engine's
//! refusal went into acton's unclaimed-response drain at `trace` level. This
//! starts a real engine and checks that the loss is now accounted for.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use emergent_client::{EmergentMessage, EmergentSource};
use serde_json::json;
use tokio::time::sleep;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// How many messages the burst publishes. Well over the burst capacity of 50.
const BURST: u64 = 500;

/// Locate the engine binary in the workspace target directory.
fn engine_binary() -> TestResult<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sdks_dir = manifest
        .parent()
        .ok_or("CARGO_MANIFEST_DIR has no parent")?;
    let workspace_root = sdks_dir.parent().ok_or("sdks dir has no parent")?;

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
    async fn start() -> TestResult<Self> {
        // The SDK installs its file subscriber only when the process has none,
        // so installing a test subscriber first keeps primitive logs out of the
        // user's data directory.
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();

        let config_dir = tempfile::tempdir()?;
        let socket_path = config_dir.path().join("test.sock");
        let log_dir = config_dir.path().join("logs");
        let db_path = config_dir.path().join("events.db");

        let config_content = format!(
            r#"
[engine]
name = "rejection-test-engine"
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
            .env("XDG_DATA_HOME", config_dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            if socket_path.exists() {
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

#[tokio::test]
async fn a_rate_limited_burst_is_counted_rather_than_lost_silently() -> TestResult<()> {
    let engine = TestEngine::start().await?;
    let source = EmergentSource::connect_to("burst_source", engine.socket()).await?;

    for seq in 0..BURST {
        // Every call reports success: the engine has not answered yet.
        source
            .publish(EmergentMessage::new("burst.tick").with_payload(json!({ "seq": seq })))
            .await?;
    }

    // disconnect drains the watcher, so every answer has arrived by the time it
    // returns and the counts are final.
    source.disconnect().await?;

    let stats = source.publish_stats();
    assert_eq!(
        stats.accepted + stats.rejected + stats.unanswered,
        BURST,
        "every publish is accounted for exactly once: {stats:?}"
    );
    assert_eq!(stats.unanswered, 0, "the engine answered every publish");
    assert!(
        stats.rejected > 0,
        "a burst of {BURST} on one connection outruns the 100/s rate limit, \
         so the engine must have refused some of it: {stats:?}"
    );
    assert!(
        stats.accepted > 0,
        "the burst capacity of 50 means the first messages get through: {stats:?}"
    );

    Ok(())
}

#[tokio::test]
async fn a_publish_within_the_rate_limit_is_accepted() -> TestResult<()> {
    let engine = TestEngine::start().await?;
    let source = EmergentSource::connect_to("polite_source", engine.socket()).await?;

    for seq in 0..5_u64 {
        source
            .publish(EmergentMessage::new("polite.tick").with_payload(json!({ "seq": seq })))
            .await?;
    }

    source.disconnect().await?;

    assert_eq!(
        source.publish_stats(),
        emergent_client::PublishStats {
            accepted: 5,
            rejected: 0,
            unanswered: 0,
        }
    );

    Ok(())
}
