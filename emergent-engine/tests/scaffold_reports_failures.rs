//! `emergent scaffold` must fail when it could not produce the crate it was
//! asked for.
//!
//! The scaffold used to print a failure to stderr, carry on, and exit 0 with a
//! partial crate on disk, or wait out its 30 second timeout and still exit 0
//! when the run never started. These tests drive the actor pipeline through
//! the same entry point the CLI uses and check the error that comes back.

use std::fs;
use std::path::PathBuf;

use emergent_engine::scaffold::cli::ScaffoldArgs;
use emergent_engine::scaffold::run_scaffold;

/// Scaffold arguments for a Rust handler, as the CLI would build them.
fn args(name: &str, output: Option<PathBuf>) -> ScaffoldArgs {
    ScaffoldArgs {
        language: Some("rust".to_string()),
        primitive_type: Some("handler".to_string()),
        name: Some(name.to_string()),
        subscribes: Some("timer.tick".to_string()),
        publishes: Some("timer.filtered".to_string()),
        output,
        description: Some("Scaffold failure test".to_string()),
        dry_run: false,
        json: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_the_scaffold_cannot_write_fails_the_command_and_is_named() {
    let root = match tempfile::TempDir::new() {
        Ok(root) => root,
        Err(e) => panic!("could not create a temporary directory: {e}"),
    };

    // A regular file where the output directory should be: every write below
    // it fails, which is the shape of a run that produces a partial crate.
    let blocked = root.path().join("blocked");
    if let Err(e) = fs::write(&blocked, "not a directory") {
        panic!("could not create the blocking file: {e}");
    }

    let result = run_scaffold(args("probe_handler", Some(blocked.clone()))).await;

    let Err(error) = result else {
        panic!("a scaffold that wrote no files must not succeed");
    };
    let error = error.to_string();

    assert!(
        error.contains("Cargo.toml") && error.contains("src/main.rs"),
        "the error must name every file that failed: {error}"
    );
    assert!(
        error.contains("0 of 2") || error.contains("2 of 2"),
        "the error must say how much of the crate failed: {error}"
    );
    assert!(
        blocked.is_file(),
        "the scaffold must not have replaced the blocking file"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_that_never_starts_fails_the_command() {
    // An invalid primitive name is rejected before any template runs, so the
    // workflow has no completion to report. It used to wait out the timeout
    // and exit 0 anyway.
    let started = std::time::Instant::now();
    let result = run_scaffold(args("Not-Snake-Case", None)).await;

    let Err(error) = result else {
        panic!("a scaffold that never started must not succeed");
    };
    assert!(
        error.to_string().contains("Invalid input"),
        "the error must carry the reason the run never started: {error}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "the command must fail at once instead of waiting out the timeout"
    );
    assert!(
        !PathBuf::from("./Not-Snake-Case").exists(),
        "a rejected run must not create an output directory"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_complete_run_succeeds() {
    let root = match tempfile::TempDir::new() {
        Ok(root) => root,
        Err(e) => panic!("could not create a temporary directory: {e}"),
    };
    let output = root.path().join("probe_handler");

    if let Err(e) = run_scaffold(args("probe_handler", Some(output.clone()))).await {
        panic!("a scaffold with a writable output directory must succeed: {e}");
    }

    assert!(output.join("Cargo.toml").is_file());
    assert!(output.join("src/main.rs").is_file());
}
