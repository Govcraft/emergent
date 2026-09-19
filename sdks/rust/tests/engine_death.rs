//! A Source must not outlive the engine that spawned it.
//!
//! SIGKILL is the case no graceful shutdown covers: there is no
//! `system.shutdown` broadcast and no SIGTERM, only the IPC socket reaching
//! EOF. Handlers and Sinks already see that as their subscription stream
//! ending. A Source subscribes to nothing, so `run_source` has to turn the
//! closed connection into the shutdown signal the user function waits on.
//!
//! This is the cross-platform half of the fix for Govcraft/emergent#56. The
//! Linux half, a parent-death signal armed in the child before exec, lives in
//! the engine and covers primitives that were not built on these helpers.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use emergent_client::helpers::run_source;
use tokio::time::{sleep, timeout};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Locate the engine binary in the workspace target directory.
fn engine_binary() -> TestResult<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // sdks/rust -> sdks -> workspace root
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

/// A running engine that can be killed outright, with cleanup on drop.
struct TestEngine {
    child: Child,
    socket_path: PathBuf,
    _config_dir: tempfile::TempDir,
}

impl TestEngine {
    /// Start an engine with a minimal config and a unique socket.
    async fn start() -> TestResult<Self> {
        let config_dir = tempfile::tempdir()?;
        let socket_path = config_dir.path().join("test.sock");
        let log_dir = config_dir.path().join("logs");
        let db_path = config_dir.path().join("events.db");

        let config_content = format!(
            r#"
[engine]
name = "engine-death-test"
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

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            if socket_path.exists() {
                // Give the IPC listener a moment to start accepting.
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

    /// Kill the engine the way a crash does: no signal it can handle.
    fn kill_hard(&mut self) -> TestResult {
        self.child.kill()?;
        self.child.wait()?;
        Ok(())
    }
}

impl Drop for TestEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn a_source_stops_when_the_engine_is_killed() -> TestResult {
    let mut engine = TestEngine::start().await?;

    // SAFETY: this test binary holds exactly one test, so no other thread is
    // reading or writing the environment while this runs. EMERGENT_SOCKET is
    // how the engine points a real primitive at its socket, and `run_source`
    // resolves the same way.
    unsafe {
        std::env::set_var("EMERGENT_SOCKET", engine.socket());
    }

    let source = tokio::spawn(run_source(
        Some("eof_probe"),
        |_source, mut shutdown| async move {
            shutdown.changed().await.map_err(|e| e.to_string())?;
            if !*shutdown.borrow_and_update() {
                return Err("the shutdown watch fired without asking to stop".to_string());
            }
            Ok(())
        },
    ));

    // Let the source finish connecting, so the engine's death is what stops it
    // rather than a connection that was never made.
    sleep(Duration::from_millis(300)).await;

    engine.kill_hard()?;

    let stopped = timeout(Duration::from_secs(5), source).await;
    let joined = stopped.map_err(|_| "the source outlived the engine")?;
    joined??;

    Ok(())
}
