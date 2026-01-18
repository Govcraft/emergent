//! Scaffold command - Generate primitives using Emergent's own architecture.
//!
//! This module implements the `emergent scaffold` subcommand, which uses
//! in-process actors to demonstrate the event-driven architecture while
//! providing practical developer tooling.
//!
//! # Architecture
//!
//! ```text
//! CLI Source ──> Template Handler ──> File Writer Sink
//!     │               │                     │
//! StartScaffold  ScaffoldRequest     TemplateRendered
//!                                          │
//!                                    Files on disk
//! ```
//!
//! # Usage
//!
//! ```bash
//! # Interactive wizard
//! emergent scaffold
//!
//! # Full flag mode (scriptable)
//! emergent scaffold \
//!   --type handler \
//!   --name filter \
//!   --subscribes "timer.tick" \
//!   --publishes "timer.filtered"
//!
//! # Preview mode
//! emergent scaffold --type source --name timer --dry-run
//! ```

pub mod cli;
pub mod handler;
pub mod messages;
pub mod sink;
pub mod source;
pub mod templates;

use acton_reactive::prelude::*;

use cli::ScaffoldArgs;
use handler::build_template_handler_actor;
use sink::{build_file_writer_actor, ScaffoldCompleteMessage};
use source::{build_cli_source_actor, StartScaffold};

/// Run the scaffold command with the given arguments.
///
/// This function:
/// 1. Creates the actor runtime
/// 2. Builds the source, handler, and sink actors
/// 3. Triggers the workflow by sending `StartScaffold`
/// 4. Waits for completion
///
/// # Errors
///
/// Returns an error if the actor runtime fails to shut down.
pub async fn run_scaffold(args: ScaffoldArgs) -> anyhow::Result<()> {
    // Create a new acton runtime for the scaffold operation
    let mut runtime = ActonApp::launch_async().await;

    // Build the actor pipeline
    let source_handle = build_cli_source_actor(&mut runtime);
    let _handler_handle = build_template_handler_actor(&mut runtime);
    let _sink_handle = build_file_writer_actor(&mut runtime);

    // Create a completion listener
    let mut completion_actor = runtime.new_actor_with_name::<()>("scaffold_completion".to_string());

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(tx)));

    let tx_clone = tx.clone();
    completion_actor.act_on::<ScaffoldCompleteMessage>(move |_actor, _envelope| {
        let tx = tx_clone.clone();
        Reply::pending(async move {
            if let Some(tx) = tx.lock().await.take() {
                let _ = tx.send(());
            }
        })
    });

    let completion_handle = completion_actor.start().await;
    completion_handle.subscribe::<ScaffoldCompleteMessage>().await;

    // Trigger the workflow
    source_handle.send(StartScaffold { args }).await;

    // Wait for completion with a timeout
    let timeout = tokio::time::Duration::from_secs(30);
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(())) => {
            // Success - let the actors clean up
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        Ok(Err(_)) => {
            // Channel closed unexpectedly
            eprintln!("Scaffold workflow ended unexpectedly");
        }
        Err(_) => {
            // Timeout
            eprintln!("Scaffold workflow timed out");
        }
    }

    // Shutdown the runtime
    runtime.shutdown_all().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::scaffold::messages::{Language, PrimitiveType};

    #[test]
    fn test_language_from_str() {
        assert_eq!("rust".parse::<Language>().ok(), Some(Language::Rust));
        assert_eq!("rs".parse::<Language>().ok(), Some(Language::Rust));
        assert_eq!("typescript".parse::<Language>().ok(), Some(Language::TypeScript));
        assert_eq!("ts".parse::<Language>().ok(), Some(Language::TypeScript));
        assert_eq!("python".parse::<Language>().ok(), Some(Language::Python));
        assert_eq!("py".parse::<Language>().ok(), Some(Language::Python));
        assert!("invalid".parse::<Language>().is_err());
    }

    #[test]
    fn test_primitive_type_from_str() {
        assert_eq!("source".parse::<PrimitiveType>().ok(), Some(PrimitiveType::Source));
        assert_eq!("handler".parse::<PrimitiveType>().ok(), Some(PrimitiveType::Handler));
        assert_eq!("sink".parse::<PrimitiveType>().ok(), Some(PrimitiveType::Sink));
        assert!("invalid".parse::<PrimitiveType>().is_err());
    }
}
