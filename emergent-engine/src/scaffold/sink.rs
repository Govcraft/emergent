//! File writer sink actor for scaffold workflow.
//!
//! Receives `TemplateRenderedMessage` and writes files to disk.
//! In dry-run mode, only previews the output.

use std::fs;
use std::sync::{Arc, Mutex};

use acton_reactive::prelude::*;
use serde_json::json;

use crate::scaffold::handler::{AllTemplatesRendered, TemplateRenderedMessage};
use crate::scaffold::messages::ScaffoldComplete;

/// State for the file writer sink actor.
#[derive(Default, Clone, Debug)]
pub struct FileWriterState {
    /// Files that have been written.
    files_written: Arc<Mutex<Vec<String>>>,
}

/// Message to signal scaffold completion.
#[acton_message]
#[derive(Clone)]
pub struct ScaffoldCompleteMessage {
    /// The completion result.
    pub result: ScaffoldComplete,
}

/// Build and configure the file writer sink actor.
///
/// This actor:
/// 1. Receives `TemplateRenderedMessage` from the handler
/// 2. Writes files to disk (or previews in dry-run mode)
/// 3. Emits `ScaffoldCompleteMessage` when all files are written
pub fn build_file_writer_actor(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut actor = runtime.new_actor_with_name::<FileWriterState>("scaffold_sink".to_string());

    // Handle individual rendered templates
    actor.act_on::<TemplateRenderedMessage>(|actor, envelope| {
        let msg = envelope.message();
        let rendered = &msg.rendered;
        let output_dir = msg.output_dir.clone();
        let dry_run = msg.dry_run;
        let json_output = msg.json_output;
        let files_written = actor.model.files_written.clone();

        let file_path = rendered.file_path.clone();
        let content = rendered.content.clone();

        Reply::pending(async move {
            let full_path = output_dir.join(&file_path);

            if dry_run {
                // Preview mode - just show what would be written
                if json_output {
                    let output = json!({
                        "action": "preview",
                        "file": full_path.display().to_string(),
                        "size": content.len(),
                    });
                    println!("{output}");
                } else {
                    println!("Would create: {}", full_path.display());
                    println!("--- {} ---", file_path);
                    println!("{content}");
                    println!("---");
                }
            } else {
                // Create parent directories if needed
                if let Some(parent) = full_path.parent()
                    && let Err(e) = fs::create_dir_all(parent)
                {
                    eprintln!("Failed to create directory {}: {e}", parent.display());
                    return;
                }

                // Write the file
                if let Err(e) = fs::write(&full_path, &content) {
                    eprintln!("Failed to write {}: {e}", full_path.display());
                    return;
                }

                if json_output {
                    let output = json!({
                        "action": "created",
                        "file": full_path.display().to_string(),
                        "size": content.len(),
                    });
                    println!("{output}");
                } else {
                    println!("Created: {}", full_path.display());
                }
            }

            // Track written file
            if let Ok(mut files) = files_written.lock() {
                files.push(file_path);
            }
        })
    });

    // Handle completion
    actor.act_on::<AllTemplatesRendered>(|actor, envelope| {
        let msg = envelope.message();
        let broker = actor.broker().clone();
        let output_dir = msg.output_dir.clone();
        let dry_run = msg.dry_run;
        let json_output = msg.json_output;
        let files = msg.files.clone();

        Reply::pending(async move {
            let result = ScaffoldComplete {
                files_written: files.clone(),
                output_dir: output_dir.clone(),
                success: true,
                error: None,
                dry_run,
            };

            if json_output {
                let output = json!({
                    "action": if dry_run { "preview_complete" } else { "scaffold_complete" },
                    "files": files,
                    "output_dir": output_dir.display().to_string(),
                    "success": true,
                });
                println!("{output}");
            } else if dry_run {
                println!("\nDry run complete. {} file(s) would be created in {}",
                    files.len(),
                    output_dir.display()
                );
            } else {
                println!("\nScaffold complete! {} file(s) created in {}",
                    files.len(),
                    output_dir.display()
                );
                println!("\nNext steps:");
                println!("  cd {}", output_dir.display());
                println!("  cargo build");
            }

            broker.broadcast(ScaffoldCompleteMessage { result }).await;
        })
    });

    // Subscribe to messages
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let handle = actor.start().await;
            handle.subscribe::<TemplateRenderedMessage>().await;
            handle.subscribe::<AllTemplatesRendered>().await;
            handle
        })
    })
}
