//! `emergent validate` runs the engine's startup checks without starting it.
//!
//! These drive the real binary, because the contract is the process: what is on
//! stdout, and the exit code a control plane branches on. Each run points
//! `XDG_CONFIG_HOME` at an empty directory so a developer's own
//! `acton/ipc.toml` cannot change the connection limit under test.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Write `content` as a config in `dir` and return its path.
fn write_config(dir: &Path, content: &str) -> std::io::Result<std::path::PathBuf> {
    let path = dir.join("emergent.toml");
    std::fs::write(&path, content)?;
    Ok(path)
}

/// Run the engine binary with `args`, isolated from the user's config.
fn emergent(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_emergent"))
        .args(args)
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env("XDG_RUNTIME_DIR", dir)
        .env_remove("RUST_LOG")
        .current_dir(dir)
        .output()
}

/// Run `emergent validate --json` and parse stdout, which must be JSON alone.
fn validate_json(
    dir: &Path,
    config: &Path,
    extra: &[&str],
) -> Result<(Value, i32), Box<dyn std::error::Error>> {
    let config = config.to_string_lossy();
    let mut args = vec!["validate", "--json", "--config", config.as_ref()];
    args.extend_from_slice(extra);
    let output = emergent(dir, &args)?;
    let report: Value = serde_json::from_slice(&output.stdout)?;
    Ok((report, output.status.code().unwrap_or(-1)))
}

const VALID: &str = r#"
[engine]
name = "validate-test"

[[sources]]
name = "tick"
path = "/bin/sh"
publishes = ["tick"]

[[sinks]]
name = "out"
path = "/bin/sh"
subscribes = ["tick"]
"#;

#[test]
fn a_valid_config_exits_zero_with_ok_true() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config = write_config(dir.path(), VALID)?;
    let (report, code) = validate_json(dir.path(), &config, &[])?;

    assert_eq!(code, 0);
    assert_eq!(report["ok"], true);
    assert_eq!(report["engine_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["errors"], serde_json::json!([]));
    assert_eq!(report["warnings"], serde_json::json!([]));
    Ok(())
}

#[test]
fn an_unknown_key_exits_one_naming_the_key() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config = write_config(
        dir.path(),
        &format!("{VALID}publishes = [\"not-for-a-sink\"]\n"),
    )?;
    let (report, code) = validate_json(dir.path(), &config, &[])?;

    assert_eq!(code, 1);
    assert_eq!(report["ok"], false);
    assert_eq!(report["errors"][0]["code"], "unknown_field");
    assert_eq!(report["errors"][0]["path"], "sinks[0].publishes");
    Ok(())
}

#[test]
fn missing_paths_are_errors_unless_the_check_is_skipped() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config = write_config(
        dir.path(),
        &VALID.replace("/bin/sh", "/nonexistent/emergent-validate-test"),
    )?;

    let (report, code) = validate_json(dir.path(), &config, &[])?;
    assert_eq!(code, 1);
    let codes: Vec<&Value> = report["errors"]
        .as_array()
        .map(|a| a.iter().map(|e| &e["code"]).collect())
        .unwrap_or_default();
    assert_eq!(codes, vec!["path_not_found", "path_not_found"]);
    assert_eq!(report["errors"][1]["path"], "sinks[0].path");

    let (report, code) = validate_json(dir.path(), &config, &["--skip-path-check"])?;
    assert_eq!(code, 0);
    assert_eq!(report["ok"], true);
    Ok(())
}

#[test]
fn the_connection_limit_is_checked() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config = write_config(
        dir.path(),
        &VALID.replace(
            "name = \"validate-test\"",
            "name = \"validate-test\"\nmax_connections = 5",
        ),
    )?;
    let (report, code) = validate_json(dir.path(), &config, &[])?;

    assert_eq!(code, 1);
    assert_eq!(report["errors"][0]["code"], "connection_capacity");
    assert_eq!(report["errors"][0]["path"], "engine.max_connections");
    Ok(())
}

#[test]
fn a_missing_file_is_reported_as_json() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (report, code) = validate_json(dir.path(), &dir.path().join("absent.toml"), &[])?;

    assert_eq!(code, 1);
    assert_eq!(report["errors"][0]["code"], "config_not_found");
    Ok(())
}

#[test]
fn human_output_names_each_problem() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config = write_config(
        dir.path(),
        &VALID.replace(
            "[\"tick\"]\n\n[[sinks]]",
            "[\"tick\"]\nrestart = \"sometimes\"\n\n[[sinks]]",
        ),
    )?;
    let config_arg = config.to_string_lossy();
    let output = emergent(dir.path(), &["validate", "--config", config_arg.as_ref()])?;
    let stdout = String::from_utf8(output.stdout)?;

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout.contains("error[invalid_restart_policy] at sources[0].restart"),
        "{stdout}"
    );
    assert!(stdout.contains("is invalid: 1 error(s)"), "{stdout}");
    Ok(())
}

#[test]
fn startup_refuses_with_the_same_check_before_logging_anywhere() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config = write_config(
        dir.path(),
        &VALID.replace(
            "name = \"validate-test\"",
            "name = \"validate-test\"\nmax_connections = 5",
        ),
    )?;
    let config_arg = config.to_string_lossy();
    let output = emergent(
        dir.path(),
        &["--log-stdout", "--config", config_arg.as_ref()],
    )?;
    let stderr = String::from_utf8(output.stderr)?;

    assert_ne!(output.status.code(), Some(0));
    assert!(
        stderr.contains("IPC connection limit of 5 is too low for 2 enabled primitive(s)"),
        "{stderr}"
    );
    // Refused before anything was created: no socket, no log, no event store.
    assert!(!dir.path().join("xdg-data").exists());
    assert!(!dir.path().join("validate-test.sock").exists());
    Ok(())
}
