//! Exec Handler
//!
//! A Handler that subscribes to events, pipes the incoming payload through an
//! external executable's stdin, and publishes the executable's stdout as a new event.
//!
//! This enables arbitrary command-line tools (jq, claude, python scripts, etc.)
//! to participate in Emergent workflows without writing a custom primitive.
//!
//! # Data Flow
//!
//! 1. Receive an event matching configured subscriptions
//! 2. Serialize the event payload as JSON
//! 3. Spawn the command and write the payload to its stdin
//! 4. Capture stdout and publish as a new event
//! 5. On failure, publish an error event
//!
//! # Messages Published
//!
//! - Configurable success type (default: `exec.output`) — stdout from the command
//! - Configurable error type (default: `exec.error`) — on non-zero exit or timeout
//!
//! # Usage
//!
//! ```bash
//! # Pipe events through jq
//! exec-handler --publish-as data.transformed -- jq '.data | keys'
//!
//! # Pipe events through claude
//! exec-handler --publish-as ai.analysis --timeout 60000 -- claude -p "Analyze this"
//!
//! # Use defaults
//! exec-handler -- my-transform-script
//! ```

use clap::Parser;
use emergent_client::{EmergentHandler, EmergentMessage};
use serde_json::json;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::signal::unix::{SignalKind, signal};

/// Exec Handler — pipe event payloads through an executable.
///
/// Everything after `--` is the command and its arguments.
#[derive(Parser, Debug)]
#[command(name = "exec_handler")]
#[command(about = "Pipe event payloads through an executable and publish results")]
#[command(trailing_var_arg = true)]
struct Args {
    /// Message type for successful output.
    #[arg(short, long, default_value = "exec.output")]
    publish_as: String,

    /// Message type for error output.
    #[arg(short, long, default_value = "exec.error")]
    error_as: String,

    /// Per-execution timeout in milliseconds.
    #[arg(short, long, default_value = "30000")]
    timeout: u64,

    /// The command and arguments to execute (after --).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

/// Result of a successful command execution.
struct ExecResult {
    /// Parsed stdout payload (JSON or wrapped string).
    stdout_payload: serde_json::Value,
    /// Stderr output, if any.
    stderr: Option<String>,
}

/// Error from command execution.
enum ExecError {
    /// Command exited with non-zero status.
    Failed {
        exit_code: i32,
        stderr: String,
        command: String,
    },
    /// Command exceeded the timeout.
    Timeout { command: String },
    /// Failed to spawn the command.
    SpawnFailed { error: String, command: String },
    /// Failed to write to stdin.
    StdinFailed { error: String, command: String },
}

/// Execute a command, piping the payload JSON to its stdin and capturing stdout.
///
/// Returns the parsed stdout as a JSON value on success, or an error with
/// exit code and stderr on failure.
async fn execute_command(
    payload: &serde_json::Value,
    command: &[String],
    timeout_ms: u64,
) -> Result<ExecResult, ExecError> {
    let command_str = command.join(" ");

    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ExecError::SpawnFailed {
            error: e.to_string(),
            command: command_str.clone(),
        })?;

    // Write payload JSON to stdin, then close it
    let payload_bytes = serde_json::to_vec(payload).unwrap_or_default();
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&payload_bytes).await.map_err(|e| {
            ExecError::StdinFailed {
                error: e.to_string(),
                command: command_str.clone(),
            }
        })?;
        // stdin is dropped here, closing the pipe
    }

    // Wait for the process with timeout
    let output = tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output())
        .await
        .map_err(|_| ExecError::Timeout {
            command: command_str.clone(),
        })?
        .map_err(|e| ExecError::SpawnFailed {
            error: e.to_string(),
            command: command_str.clone(),
        })?;

    let exit_code = output.status.code().unwrap_or(-1);

    if exit_code != 0 {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(ExecError::Failed {
            exit_code,
            stderr,
            command: command_str,
        });
    }

    // Parse stdout: try JSON first, fall back to wrapped string
    let stdout_payload = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        let stdout_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        json!({"output": stdout_str})
    });

    let stderr = {
        let s = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    };

    Ok(ExecResult {
        stdout_payload,
        stderr,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Validate that a command was provided after --
    if args.command.is_empty() {
        eprintln!("Error: no command specified. Provide a command after '--'.");
        eprintln!("Example: exec-handler --publish-as data.out -- jq '.'");
        std::process::exit(1);
    }

    // Get the handler name from environment (set by engine) or use default
    let name = std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "exec_handler".to_string());

    // Connect to the Emergent engine
    let handler = match EmergentHandler::connect(&name).await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Failed to connect to Emergent engine: {e}");
            std::process::exit(1);
        }
    };

    // Resolve subscription topics from engine configuration
    let topics = match handler.get_my_subscriptions().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Failed to get subscriptions: {e}");
            std::process::exit(1);
        }
    };

    if topics.is_empty() {
        eprintln!("No subscription topics configured for '{name}'");
        std::process::exit(1);
    }

    // Subscribe to configured topics
    let topics_refs: Vec<&str> = topics.iter().map(|s| s.as_str()).collect();
    let mut stream = match handler.subscribe(&topics_refs).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to subscribe: {e}");
            std::process::exit(1);
        }
    };

    // Set up SIGTERM handler for graceful shutdown
    let mut sigterm = signal(SignalKind::terminate())?;

    // Process incoming messages
    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                let _ = handler.disconnect().await;
                break;
            }

            msg = stream.next() => {
                match msg {
                    Some(msg) => {
                        match execute_command(msg.payload(), &args.command, args.timeout).await {
                            Ok(result) => {
                                let mut output = EmergentMessage::new(&args.publish_as)
                                    .with_causation_id(msg.id())
                                    .with_correlation_id_option(msg.correlation_id.as_ref())
                                    .with_payload(result.stdout_payload);

                                if let Some(stderr) = result.stderr {
                                    output = output.with_metadata(json!({"stderr": stderr}));
                                }

                                let _ = handler.publish(output).await;
                            }
                            Err(exec_err) => {
                                let error_payload = match exec_err {
                                    ExecError::Failed { exit_code, stderr, command } => {
                                        json!({
                                            "exit_code": exit_code,
                                            "stderr": stderr,
                                            "command": command,
                                        })
                                    }
                                    ExecError::Timeout { command } => {
                                        json!({
                                            "exit_code": null,
                                            "stderr": "process timed out",
                                            "command": command,
                                        })
                                    }
                                    ExecError::SpawnFailed { error, command } => {
                                        json!({
                                            "exit_code": null,
                                            "stderr": error,
                                            "command": command,
                                        })
                                    }
                                    ExecError::StdinFailed { error, command } => {
                                        json!({
                                            "exit_code": null,
                                            "stderr": format!("stdin write failed: {error}"),
                                            "command": command,
                                        })
                                    }
                                };

                                let error_msg = EmergentMessage::new(&args.error_as)
                                    .with_causation_id(msg.id())
                                    .with_correlation_id_option(msg.correlation_id.as_ref())
                                    .with_payload(error_payload);

                                let _ = handler.publish(error_msg).await;
                            }
                        }
                    }
                    None => {
                        // Stream ended (graceful shutdown)
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_json_stdout_is_parsed_directly() {
        let payload = json!({"input": "hello"});
        let command = vec!["echo".to_string(), r#"{"result":"world"}"#.to_string()];

        let result = execute_command(&payload, &command, 5000).await;
        let result = result.unwrap_or_else(|_| panic!("expected success"));

        assert_eq!(result.stdout_payload, json!({"result": "world"}));
        assert!(result.stderr.is_none());
    }

    #[tokio::test]
    async fn test_plain_text_stdout_is_wrapped() {
        let payload = json!({"input": "hello"});
        let command = vec!["echo".to_string(), "plain text output".to_string()];

        let result = execute_command(&payload, &command, 5000).await;
        let result = result.unwrap_or_else(|_| panic!("expected success"));

        assert_eq!(
            result.stdout_payload,
            json!({"output": "plain text output"})
        );
    }

    #[tokio::test]
    async fn test_empty_stdout_produces_empty_output() {
        let payload = json!({"input": "hello"});
        // `cat /dev/null` consumes stdin and produces no stdout, exits 0
        let command = vec!["cat".to_string(), "/dev/null".to_string()];

        let result = execute_command(&payload, &command, 5000).await;
        let result = result.unwrap_or_else(|_| panic!("expected success"));

        assert_eq!(result.stdout_payload, json!({"output": ""}));
    }

    #[tokio::test]
    async fn test_nonzero_exit_returns_error() {
        let payload = json!({"input": "hello"});
        let command = vec!["false".to_string()];

        let result = execute_command(&payload, &command, 5000).await;
        assert!(result.is_err());

        if let Err(ExecError::Failed {
            exit_code,
            command,
            ..
        }) = result
        {
            assert_eq!(exit_code, 1);
            assert_eq!(command, "false");
        } else {
            panic!("expected ExecError::Failed");
        }
    }

    #[tokio::test]
    async fn test_timeout_kills_process() {
        let payload = json!({"input": "hello"});
        let command = vec!["sleep".to_string(), "10".to_string()];

        let result = execute_command(&payload, &command, 100).await;
        assert!(result.is_err());

        if let Err(ExecError::Timeout { command }) = result {
            assert_eq!(command, "sleep 10");
        } else {
            panic!("expected ExecError::Timeout");
        }
    }

    #[tokio::test]
    async fn test_stderr_on_success_is_captured() {
        let payload = json!({"input": "hello"});
        // Write to stderr and stdout
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"echo '{"ok":true}' && echo 'warning: something' >&2"#.to_string(),
        ];

        let result = execute_command(&payload, &command, 5000).await;
        let result = result.unwrap_or_else(|_| panic!("expected success"));

        assert_eq!(result.stdout_payload, json!({"ok": true}));
        assert_eq!(result.stderr.as_deref(), Some("warning: something"));
    }

    #[tokio::test]
    async fn test_spawn_failure_returns_error() {
        let payload = json!({"input": "hello"});
        let command = vec!["nonexistent_command_that_should_not_exist".to_string()];

        let result = execute_command(&payload, &command, 5000).await;
        assert!(result.is_err());

        if let Err(ExecError::SpawnFailed { command, .. }) = result {
            assert_eq!(command, "nonexistent_command_that_should_not_exist");
        } else {
            panic!("expected ExecError::SpawnFailed");
        }
    }

    #[tokio::test]
    async fn test_payload_is_piped_to_stdin() {
        let payload = json!({"name": "emergent"});
        // `cat` echoes stdin to stdout
        let command = vec!["cat".to_string()];

        let result = execute_command(&payload, &command, 5000).await;
        let result = result.unwrap_or_else(|_| panic!("expected success"));

        // cat should echo back the JSON payload, which gets parsed directly
        assert_eq!(result.stdout_payload, json!({"name": "emergent"}));
    }
}
