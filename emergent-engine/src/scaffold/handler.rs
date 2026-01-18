//! Template handler actor for scaffold workflow.
//!
//! Receives `ScaffoldRequestMessage`, renders templates using MiniJinja,
//! and emits `TemplateRenderedMessage` for each file.

use acton_reactive::prelude::*;
use minijinja::{context, Environment};

use crate::scaffold::cli::build_template_context;
use crate::scaffold::messages::TemplateRendered;
use crate::scaffold::source::ScaffoldRequestMessage;
use crate::scaffold::templates::TemplateRegistry;

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

/// Message indicating all templates have been rendered.
#[acton_message]
#[derive(Clone)]
pub struct AllTemplatesRendered {
    /// Total number of files rendered.
    pub total_files: usize,
    /// Whether this is a dry run.
    pub dry_run: bool,
    /// Whether to output JSON.
    pub json_output: bool,
    /// The output directory.
    pub output_dir: std::path::PathBuf,
    /// List of files that were rendered.
    pub files: Vec<String>,
}

/// State for the template handler actor.
#[derive(Default, Clone, Debug)]
pub struct TemplateHandlerState;

/// Render a single template file.
fn render_template(
    env: &Environment<'_>,
    template_content: &str,
    context: &crate::scaffold::messages::TemplateContext,
) -> Result<String, String> {
    let template = env
        .template_from_str(template_content)
        .map_err(|e| format!("Failed to parse template: {e}"))?;

    template
        .render(context!(
            name => context.name,
            name_snake => context.name_snake,
            name_pascal => context.name_pascal,
            primitive_type => context.primitive_type,
            subscribes => context.subscribes,
            publishes => context.publishes,
            description => context.description,
            needs_chrono => context.needs_chrono,
        ))
        .map_err(|e| format!("Failed to render template: {e}"))
}

/// Build and configure the template handler actor.
///
/// This actor:
/// 1. Receives `ScaffoldRequestMessage` from the source
/// 2. Renders each template file using MiniJinja
/// 3. Emits `TemplateRenderedMessage` for each file
/// 4. Emits `AllTemplatesRendered` when done
pub fn build_template_handler_actor(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut actor = runtime.new_actor_with_name::<TemplateHandlerState>("scaffold_handler".to_string());

    actor.act_on::<ScaffoldRequestMessage>(|actor, envelope| {
        let request = envelope.message().request.clone();
        let broker = actor.broker().clone();

        Reply::pending(async move {
            let registry = TemplateRegistry::new();
            let context = build_template_context(&request);

            // Check if we have templates for this language
            if !registry.has_language(request.language) {
                eprintln!(
                    "No templates available for language: {}. Only Rust is currently supported.",
                    request.language
                );
                return;
            }

            // Get list of files to generate
            let files = registry.files_for(request.language, request.primitive_type);
            let total_files = files.len();

            if total_files == 0 {
                eprintln!(
                    "No templates found for {} {}",
                    request.language,
                    request.primitive_type
                );
                return;
            }

            // Create MiniJinja environment
            let env = Environment::new();
            let mut rendered_files = Vec::new();

            // Render each template
            for (index, filename) in files.iter().enumerate() {
                let template_content = match registry.get(request.language, request.primitive_type, filename) {
                    Some(content) => content,
                    None => {
                        eprintln!("Template not found: {filename}");
                        continue;
                    }
                };

                match render_template(&env, template_content, &context) {
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
                        eprintln!("Failed to render {filename}: {e}");
                    }
                }
            }

            // Signal completion
            let complete = AllTemplatesRendered {
                total_files: rendered_files.len(),
                dry_run: request.dry_run,
                json_output: request.json_output,
                output_dir: request.output_dir,
                files: rendered_files,
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
