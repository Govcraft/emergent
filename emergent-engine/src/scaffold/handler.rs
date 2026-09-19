//! Template handler actor for scaffold workflow.
//!
//! Receives `ScaffoldRequestMessage`, renders templates using MiniJinja,
//! and emits `TemplateRenderedMessage` for each file.

use acton_reactive::prelude::*;

use crate::scaffold::cli::build_template_context;
use crate::scaffold::messages::{Language, PrimitiveType, TemplateRendered};
use crate::scaffold::outcome::TemplateFailure;
use crate::scaffold::source::ScaffoldRequestMessage;
use crate::scaffold::templates::{TemplateRegistry, render_template};

/// Message containing a rendered template file.
#[acton_message]
#[derive(Clone)]
pub struct TemplateRenderedMessage {
    /// The rendered template data.
    pub rendered: TemplateRendered,
    /// Whether this is a dry run.
    pub dry_run: bool,
    /// Whether to output JSON.
    pub json_output: bool,
    /// The output directory.
    pub output_dir: std::path::PathBuf,
}

/// Message indicating the template handler has finished.
///
/// It reports what the run was asked to produce as well as what it produced,
/// so a run that failed part way cannot be mistaken for a smaller success.
#[acton_message]
#[derive(Clone)]
pub struct AllTemplatesRendered {
    /// Number of files the run was asked to produce.
    pub total_files: usize,
    /// Whether this is a dry run.
    pub dry_run: bool,
    /// Whether to output JSON.
    pub json_output: bool,
    /// The output directory.
    pub output_dir: std::path::PathBuf,
    /// List of files that were rendered.
    pub files: Vec<String>,
    /// Files the run was asked to produce and could not.
    pub failures: Vec<TemplateFailure>,
    /// The language used for code generation.
    pub language: Language,
    /// Name of the primitive.
    pub name: String,
    /// Type of primitive scaffolded.
    pub primitive_type: PrimitiveType,
    /// Message types this primitive subscribes to.
    pub subscribes: Vec<String>,
    /// Message types this primitive publishes.
    pub publishes: Vec<String>,
}

/// State for the template handler actor.
#[derive(Default, Clone, Debug)]
pub struct TemplateHandlerState;

/// Build and configure the template handler actor.
///
/// This actor:
/// 1. Receives `ScaffoldRequestMessage` from the source
/// 2. Renders each template file using MiniJinja
/// 3. Emits `TemplateRenderedMessage` for each file
/// 4. Emits `AllTemplatesRendered` when done
pub fn build_template_handler_actor(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut actor =
        runtime.new_actor_with_name::<TemplateHandlerState>("scaffold_handler".to_string());

    actor.act_on::<ScaffoldRequestMessage>(|actor, envelope| {
        let request = envelope.message().request.clone();
        let broker = actor.broker().clone();

        Reply::pending(async move {
            let registry = TemplateRegistry::new();
            let context = build_template_context(&request);

            // Get list of files to generate
            let files = registry.files_for(request.language, request.primitive_type);
            let total_files = files.len();

            let mut rendered_files = Vec::new();
            let mut failures = Vec::new();

            if files.is_empty() {
                failures.push(TemplateFailure::new(
                    format!("{} {}", request.language, request.primitive_type),
                    "no templates are registered for this language and primitive type",
                ));
            }

            // Render each template
            for (index, filename) in files.iter().enumerate() {
                let template_content =
                    match registry.get(request.language, request.primitive_type, filename) {
                        Some(content) => content,
                        None => {
                            failures.push(TemplateFailure::new(*filename, "template not found"));
                            continue;
                        }
                    };

                match render_template(template_content, &context) {
                    Ok(content) => {
                        let rendered = TemplateRendered {
                            file_path: (*filename).to_string(),
                            content,
                            file_index: index,
                            total_files,
                        };

                        rendered_files.push((*filename).to_string());

                        let msg = TemplateRenderedMessage {
                            rendered,
                            dry_run: request.dry_run,
                            json_output: request.json_output,
                            output_dir: request.output_dir.clone(),
                        };

                        broker.broadcast(msg).await;
                    }
                    Err(e) => {
                        failures.push(TemplateFailure::new(*filename, e));
                    }
                }
            }

            // Signal completion
            let complete = AllTemplatesRendered {
                total_files,
                dry_run: request.dry_run,
                json_output: request.json_output,
                output_dir: request.output_dir,
                files: rendered_files,
                failures,
                language: request.language,
                name: request.name,
                primitive_type: request.primitive_type,
                subscribes: request.subscribes,
                publishes: request.publishes,
            };

            broker.broadcast(complete).await;
        })
    });

    // Subscribe to scaffold request messages
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let handle = actor.start().await;
            handle.subscribe::<ScaffoldRequestMessage>().await;
            handle
        })
    })
}
