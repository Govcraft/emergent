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
pub mod outcome;
pub mod sdk;
pub mod sink;
pub mod source;
pub mod templates;

use acton_reactive::prelude::*;

use cli::ScaffoldArgs;
use handler::build_template_handler_actor;
use sink::{ScaffoldCompleteMessage, build_file_writer_actor};
use source::{ScaffoldAborted, StartScaffold, build_cli_source_actor};

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
/// Returns an error if any file the scaffold was asked to produce failed, if
/// the run ended before reporting a result, or if the actor runtime fails to
/// shut down. The command exits non-zero in each case, so a partial crate on
/// disk is never reported as success.
pub async fn run_scaffold(args: ScaffoldArgs) -> anyhow::Result<()> {
    // Create a new acton runtime for the scaffold operation
    let mut runtime = ActonApp::launch_async().await;

    // Build the actor pipeline
    let source_handle = build_cli_source_actor(&mut runtime);
    let _handler_handle = build_template_handler_actor(&mut runtime);
    let _sink_handle = build_file_writer_actor(&mut runtime);

    // Create a completion listener
    let mut completion_actor = runtime.new_actor_with_name::<()>("scaffold_completion".to_string());

    // The outcome of the run: `Err` carries the message the command fails with.
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    let tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(tx)));

    let tx_clone = tx.clone();
    completion_actor.act_on::<ScaffoldCompleteMessage>(move |_actor, envelope| {
        let tx = tx_clone.clone();
        let outcome = envelope.message().result.error.clone().map_or(Ok(()), Err);
        Reply::pending(async move {
            if let Some(tx) = tx.lock().await.take() {
                let _ = tx.send(outcome);
            }
        })
    });

    let tx_clone = tx.clone();
    completion_actor.act_on::<ScaffoldAborted>(move |_actor, envelope| {
        let tx = tx_clone.clone();
        let msg = envelope.message();
        let outcome = if msg.cancelled {
            Ok(())
        } else {
            Err(msg.reason.clone())
        };
        Reply::pending(async move {
            if let Some(tx) = tx.lock().await.take() {
                let _ = tx.send(outcome);
            }
        })
    });

    let completion_handle = completion_actor.start().await;
    completion_handle
        .subscribe::<ScaffoldCompleteMessage>()
        .await;
    completion_handle.subscribe::<ScaffoldAborted>().await;

    // Trigger the workflow
    source_handle.send(StartScaffold { args }).await;

    // Wait for the outcome with a timeout
    let timeout = tokio::time::Duration::from_secs(30);
    let outcome = match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(outcome)) => {
            // Let the actors finish their current work before shutdown.
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            outcome
        }
        Ok(Err(_)) => Err("the scaffold workflow ended before reporting a result".to_string()),
        Err(_) => Err(format!(
            "the scaffold workflow timed out after {} seconds",
            timeout.as_secs()
        )),
    };

    // Shutdown the runtime
    runtime.shutdown_all().await?;

    match outcome {
        Ok(()) => Ok(()),
        Err(reason) => Err(anyhow::anyhow!(reason)),
    }
}

#[cfg(test)]
mod tests {
    use crate::scaffold::messages::{Language, PrimitiveType};

    #[test]
    fn test_language_from_str() {
        assert_eq!("rust".parse::<Language>().ok(), Some(Language::Rust));
        assert_eq!("rs".parse::<Language>().ok(), Some(Language::Rust));
        assert_eq!(
            "typescript".parse::<Language>().ok(),
            Some(Language::TypeScript)
        );
        assert_eq!("ts".parse::<Language>().ok(), Some(Language::TypeScript));
        assert_eq!("python".parse::<Language>().ok(), Some(Language::Python));
        assert_eq!("py".parse::<Language>().ok(), Some(Language::Python));
        assert!("invalid".parse::<Language>().is_err());
    }

    #[test]
    fn test_primitive_type_from_str() {
        assert_eq!(
            "source".parse::<PrimitiveType>().ok(),
            Some(PrimitiveType::Source)
        );
        assert_eq!(
            "handler".parse::<PrimitiveType>().ok(),
            Some(PrimitiveType::Handler)
        );
        assert_eq!(
            "sink".parse::<PrimitiveType>().ok(),
            Some(PrimitiveType::Sink)
        );
        assert!("invalid".parse::<PrimitiveType>().is_err());
    }
}
