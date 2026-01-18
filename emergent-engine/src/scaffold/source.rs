//! CLI source actor for scaffold workflow.
//!
//! Collects user input via CLI arguments or interactive wizard,
//! then emits a `ScaffoldRequest` message.

use acton_reactive::prelude::*;

use crate::scaffold::cli::{build_from_args, build_template_context, has_required_args, run_wizard, ScaffoldArgs, ScaffoldError};
use crate::scaffold::messages::ScaffoldRequest;

/// Message to start the scaffold workflow.
#[acton_message]
#[derive(Clone)]
pub struct StartScaffold {
    /// CLI arguments from the user.
    pub args: ScaffoldArgs,
}

/// Message containing the scaffold request after user input.
#[acton_message]
#[derive(Clone)]
pub struct ScaffoldRequestMessage {
    /// The scaffold request with all parameters.
    pub request: ScaffoldRequest,
}

/// State for the CLI source actor.
#[derive(Default, Clone, Debug)]
pub struct CliSourceState;

/// Build and configure the CLI source actor.
///
/// This actor:
/// 1. Receives a `StartScaffold` message with CLI args
/// 2. Either uses args directly or runs interactive wizard
/// 3. Emits a `ScaffoldRequestMessage` for the handler
pub fn build_cli_source_actor(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut actor = runtime.new_actor_with_name::<CliSourceState>("scaffold_source".to_string());

    actor.act_on::<StartScaffold>(|actor, envelope| {
        let args = envelope.message().args.clone();
        let broker = actor.broker().clone();

        Reply::pending(async move {
            // Collect user input (either from args or wizard)
            let request_result = if has_required_args(&args) {
                build_from_args(&args)
            } else {
                run_wizard(&args)
            };

            match request_result {
                Ok(request) => {
                    // Build template context and emit request
                    let _context = build_template_context(&request);

                    let msg = ScaffoldRequestMessage { request };
                    broker.broadcast(msg).await;
                }
                Err(ScaffoldError::Cancelled) => {
                    eprintln!("Scaffold cancelled.");
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                }
            }
        })
    });

    // Start actor and return handle
    // We use block_on here because we need the handle synchronously
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(actor.start())
    })
}
