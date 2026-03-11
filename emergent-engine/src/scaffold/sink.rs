//! File writer sink actor for scaffold workflow.
//!
//! Receives `TemplateRenderedMessage` and writes files to disk.
//! In dry-run mode, only previews the output.

use std::fs;
use std::sync::{Arc, Mutex};

use acton_reactive::prelude::*;
use serde_json::json;

use crate::scaffold::handler::{AllTemplatesRendered, TemplateRenderedMessage};
use crate::scaffold::messages::{Language, PrimitiveType, ScaffoldComplete};

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
        let language = msg.language;
        let name = msg.name.clone();
        let primitive_type = msg.primitive_type;
        let subscribes = msg.subscribes.clone();
        let publishes = msg.publishes.clone();

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
                println!(
                    "\nDry run complete. {} file(s) would be created in {}",
                    files.len(),
                    output_dir.display()
                );
            } else {
                println!(
                    "\nScaffold complete! {} file(s) created in {}",
                    files.len(),
                    output_dir.display()
                );
                println!("\nNext steps:");
                println!("  1. Build your primitive:");
                println!("     cd {}", output_dir.display());
                match language {
                    Language::Rust => println!("     cargo build --release"),
                    Language::Python => {
                        println!("     uv sync        # or: pip install -e .");
                    }
                    Language::TypeScript => {
                        println!("     # no build step required for Deno");
                    }
                }

                println!("  2. Create a config file (if you don't have one):");
                println!("     emergent init");

                let toml_snippet = format_config_snippet(
                    &name,
                    primitive_type,
                    language,
                    &output_dir,
                    &subscribes,
                    &publishes,
                );
                println!("  3. Add to your emergent.toml:");
                for line in toml_snippet.lines() {
                    println!("     {line}");
                }

                println!("  4. Start the engine:");
                println!("     emergent --config ./emergent.toml");
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

/// Format a TOML config snippet for adding the scaffolded primitive to emergent.toml.
fn format_config_snippet(
    name: &str,
    primitive_type: PrimitiveType,
    language: Language,
    output_dir: &std::path::Path,
    subscribes: &[String],
    publishes: &[String],
) -> String {
    let section = match primitive_type {
        PrimitiveType::Source => "sources",
        PrimitiveType::Handler => "handlers",
        PrimitiveType::Sink => "sinks",
    };

    let output_dir_str = output_dir.display().to_string();

    let (path, args) = match language {
        Language::Rust => (
            format!("./target/release/{name}"),
            String::from("[]"),
        ),
        Language::Python => (
            String::from("uv"),
            format!(
                "[\"run\", \"--project\", \"{output_dir_str}\", \"python\", \"{output_dir_str}/main.py\"]"
            ),
        ),
        Language::TypeScript => (
            String::from("deno"),
            format!(
                "[\"run\", \"--allow-env\", \"--allow-read\", \"--allow-write\", \"--allow-net=unix\", \"{output_dir_str}/main.ts\"]"
            ),
        ),
    };

    let mut lines = Vec::new();
    lines.push(format!("[[{section}]]"));
    lines.push(format!("name = \"{name}\""));
    lines.push(format!("path = \"{path}\""));
    lines.push(format!("args = {args}"));
    lines.push(String::from("enabled = true"));

    if !subscribes.is_empty() {
        let subs: Vec<String> = subscribes.iter().map(|s| format!("\"{s}\"")).collect();
        lines.push(format!("subscribes = [{}]", subs.join(", ")));
    }

    if !publishes.is_empty() {
        let pubs: Vec<String> = publishes.iter().map(|p| format!("\"{p}\"")).collect();
        lines.push(format!("publishes = [{}]", pubs.join(", ")));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_config_snippet_rust_source() {
        let snippet = format_config_snippet(
            "my_timer",
            PrimitiveType::Source,
            Language::Rust,
            &PathBuf::from("./my_timer"),
            &[],
            &["timer.tick".to_string()],
        );
        assert!(snippet.contains("[[sources]]"));
        assert!(snippet.contains("name = \"my_timer\""));
        assert!(snippet.contains("path = \"./target/release/my_timer\""));
        assert!(snippet.contains("args = []"));
        assert!(snippet.contains("enabled = true"));
        assert!(snippet.contains("publishes = [\"timer.tick\"]"));
        assert!(!snippet.contains("subscribes"));
    }

    #[test]
    fn test_config_snippet_python_handler() {
        let snippet = format_config_snippet(
            "my_filter",
            PrimitiveType::Handler,
            Language::Python,
            &PathBuf::from("./my_filter"),
            &["timer.tick".to_string()],
            &["timer.filtered".to_string()],
        );
        assert!(snippet.contains("[[handlers]]"));
        assert!(snippet.contains("name = \"my_filter\""));
        assert!(snippet.contains("path = \"uv\""));
        assert!(snippet.contains("./my_filter/main.py"));
        assert!(snippet.contains("subscribes = [\"timer.tick\"]"));
        assert!(snippet.contains("publishes = [\"timer.filtered\"]"));
    }

    #[test]
    fn test_config_snippet_typescript_sink() {
        let snippet = format_config_snippet(
            "my_console",
            PrimitiveType::Sink,
            Language::TypeScript,
            &PathBuf::from("./my_console"),
            &["timer.tick".to_string(), "system.started.*".to_string()],
            &[],
        );
        assert!(snippet.contains("[[sinks]]"));
        assert!(snippet.contains("name = \"my_console\""));
        assert!(snippet.contains("path = \"deno\""));
        assert!(snippet.contains("./my_console/main.ts"));
        assert!(snippet.contains("subscribes = [\"timer.tick\", \"system.started.*\"]"));
        assert!(!snippet.contains("publishes"));
    }

    #[test]
    fn test_config_snippet_is_valid_toml() {
        let snippet = format_config_snippet(
            "my_timer",
            PrimitiveType::Source,
            Language::Rust,
            &PathBuf::from("./my_timer"),
            &[],
            &["timer.tick".to_string()],
        );
        // Wrap in a document context so TOML parser can handle the array-of-tables
        let doc = format!("[engine]\nname = \"test\"\n\n{snippet}");
        let result: Result<crate::config::EmergentConfig, _> = toml::from_str(&doc);
        assert!(result.is_ok(), "Snippet should produce valid TOML: {snippet}");
    }
}

