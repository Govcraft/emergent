//! Exec Source - Shell Command Executor
//!
//! A Source that executes shell commands and emits events for stdout, stderr, and exit codes.
//! Can run commands once or repeatedly on an interval.
//!
//! Sources are SILENT - they only produce domain messages.
//! All lifecycle events are published by the engine.
//!
//! # Usage
//!
//! ```bash
//! # Run command once
//! exec-source --command "ls -la"
//!
//! # Run command every 5 seconds
//! exec-source --command "date" --interval 5000
//!
//! # Run with arguments and custom working directory
//! exec-source --command "git" --args "status" --working-dir /path/to/repo
//! ```
//!
//! # Events Published
//!
//! - `exec.output` - stdout from command
//! - `exec.error` - stderr from command
//! - `exec.exit` - exit code

use clap::Parser;
use emergent_client::{EmergentMessage, EmergentSource};
use serde_json::json;
use std::time::Duration;
use tokio::{
    process::Command,
    signal::unix::{signal, SignalKind},
};

/// Command executor that emits output events.
#[derive(Parser, Debug, Clone)]
#[command(name = "exec-source")]
#[command(about = "Executes shell commands and emits output events")]
struct Args {
    /// Command to execute.
    #[arg(short, long, env = "EXEC_SOURCE_COMMAND")]
    command: String,

    /// Command arguments (space-separated).
    #[arg(short, long, env = "EXEC_SOURCE_ARGS")]
    args: Option<String>,

    /// Optional interval in milliseconds for repeated execution (0 = run once).
    #[arg(short, long, env = "EXEC_SOURCE_INTERVAL", default_value = "0")]
    interval: u64,

    /// Working directory for command execution.
    #[arg(short = 'd', long, env = "EXEC_SOURCE_WORKING_DIR")]
    working_dir: Option<String>,

    /// Shell to use (e.g., "bash", "sh").
    #[arg(short, long, env = "EXEC_SOURCE_SHELL")]
    shell: Option<String>,
}

/// Payload for exec.output events.
#[derive(Debug, serde::Serialize)]
struct ExecOutputPayload {
    command: String,
    stdout: String,
    exit_code: i32,
}

/// Payload for exec.error events.
#[derive(Debug, serde::Serialize)]
struct ExecErrorPayload {
    command: String,
    stderr: String,
    exit_code: i32,
}

/// Payload for exec.exit events.
#[derive(Debug, serde::Serialize)]
struct ExecExitPayload {
    command: String,
    exit_code: i32,
}

/// Builds a tokio Command from args.
fn build_command(args: &Args) -> Command {
    let mut cmd = if let Some(ref shell) = args.shell {
        let mut c = Command::new(shell);
        c.arg("-c");

        // Build full command string
        let full_cmd = if let Some(ref cmd_args) = args.args {
            format!("{} {}", args.command, cmd_args)
        } else {
            args.command.clone()
        };

        c.arg(full_cmd);
        c
    } else {
        let mut c = Command::new(&args.command);

        // Add arguments if provided
        if let Some(ref cmd_args) = args.args {
            for arg in cmd_args.split_whitespace() {
                c.arg(arg);
            }
        }

        c
    };

    // Set working directory if provided
    if let Some(ref working_dir) = args.working_dir {
        cmd.current_dir(working_dir);
    }

    cmd
}

/// Executes command once and publishes output events.
async fn execute_command(
    args: &Args,
    source: &EmergentSource,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = build_command(args);

    let output = cmd.output().await?;

    let exit_code = output.status.code().unwrap_or(-1);
    let command_str = args.command.clone();

    // Publish stdout if non-empty
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !stdout.trim().is_empty() {
        let payload = ExecOutputPayload {
            command: command_str.clone(),
            stdout,
            exit_code,
        };
        let message = EmergentMessage::new("exec.output").with_payload(json!(payload));
        let _ = source.publish(message).await;
    }

    // Publish stderr if non-empty
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !stderr.trim().is_empty() {
        let payload = ExecErrorPayload {
            command: command_str.clone(),
            stderr,
            exit_code,
        };
        let message = EmergentMessage::new("exec.error").with_payload(json!(payload));
        let _ = source.publish(message).await;
    }

    // Always publish exit event
    let payload = ExecExitPayload {
        command: command_str,
        exit_code,
    };
    let message = EmergentMessage::new("exec.exit").with_payload(json!(payload));
    let _ = source.publish(message).await;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Get the source name from environment (set by engine) or use default
    let name = std::env::var("EMERGENT_NAME").unwrap_or_else(|_| "exec-source".to_string());

    // Connect to the Emergent engine (silently - lifecycle events come from engine)
    let source = match EmergentSource::connect(&name).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect to Emergent engine: {e}");
            std::process::exit(1);
        }
    };

    // Set up SIGTERM handler for graceful shutdown
    let mut sigterm = signal(SignalKind::terminate())?;

    if args.interval == 0 {
        // Run once and exit
        execute_command(&args, &source).await?;
        let _ = source.disconnect().await;
    } else {
        // Run repeatedly on interval
        let mut interval = tokio::time::interval(Duration::from_millis(args.interval));

        loop {
            tokio::select! {
                _ = sigterm.recv() => {
                    let _ = source.disconnect().await;
                    break;
                }

                _ = interval.tick() => {
                    if let Err(e) = execute_command(&args, &source).await {
                        eprintln!("Command execution failed: {e}");
                    }
                }
            }
        }
    }

    Ok(())
}
