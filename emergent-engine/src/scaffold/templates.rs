//! Template registry for scaffold code generation.
//!
//! Templates are embedded at compile time via `include_str!` and organized by:
//! - Language (rust, typescript, python)
//! - Primitive type (source, handler, sink)
//! - Filename (Cargo.toml, main.rs, etc.)

use std::collections::HashMap;

use crate::scaffold::messages::{Language, PrimitiveType};

/// Embedded Rust source template.
const RUST_SOURCE_CARGO_TOML: &str = include_str!("../../templates/rust/source/Cargo.toml.j2");
const RUST_SOURCE_MAIN_RS: &str = include_str!("../../templates/rust/source/main.rs.j2");

/// Embedded Rust handler template.
const RUST_HANDLER_CARGO_TOML: &str = include_str!("../../templates/rust/handler/Cargo.toml.j2");
const RUST_HANDLER_MAIN_RS: &str = include_str!("../../templates/rust/handler/main.rs.j2");

/// Embedded Rust sink template.
const RUST_SINK_CARGO_TOML: &str = include_str!("../../templates/rust/sink/Cargo.toml.j2");
const RUST_SINK_MAIN_RS: &str = include_str!("../../templates/rust/sink/main.rs.j2");

/// Template key combining language, primitive type, and filename.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct TemplateKey {
    pub language: Language,
    pub primitive_type: PrimitiveType,
    pub filename: &'static str,
}

impl TemplateKey {
    /// Creates a new template key.
    #[must_use]
    pub const fn new(language: Language, primitive_type: PrimitiveType, filename: &'static str) -> Self {
        Self {
            language,
            primitive_type,
            filename,
        }
    }
}

/// Registry of embedded templates.
pub struct TemplateRegistry {
    templates: HashMap<TemplateKey, &'static str>,
    files_by_type: HashMap<(Language, PrimitiveType), Vec<&'static str>>,
}

impl Default for TemplateRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateRegistry {
    /// Creates a new template registry with all embedded templates.
    #[must_use]
    pub fn new() -> Self {
        let mut templates = HashMap::new();
        let mut files_by_type: HashMap<(Language, PrimitiveType), Vec<&'static str>> = HashMap::new();

        // Rust source templates
        templates.insert(
            TemplateKey::new(Language::Rust, PrimitiveType::Source, "Cargo.toml"),
            RUST_SOURCE_CARGO_TOML,
        );
        templates.insert(
            TemplateKey::new(Language::Rust, PrimitiveType::Source, "src/main.rs"),
            RUST_SOURCE_MAIN_RS,
        );
        files_by_type.insert(
            (Language::Rust, PrimitiveType::Source),
            vec!["Cargo.toml", "src/main.rs"],
        );

        // Rust handler templates
        templates.insert(
            TemplateKey::new(Language::Rust, PrimitiveType::Handler, "Cargo.toml"),
            RUST_HANDLER_CARGO_TOML,
        );
        templates.insert(
            TemplateKey::new(Language::Rust, PrimitiveType::Handler, "src/main.rs"),
            RUST_HANDLER_MAIN_RS,
        );
        files_by_type.insert(
            (Language::Rust, PrimitiveType::Handler),
            vec!["Cargo.toml", "src/main.rs"],
        );

        // Rust sink templates
        templates.insert(
            TemplateKey::new(Language::Rust, PrimitiveType::Sink, "Cargo.toml"),
            RUST_SINK_CARGO_TOML,
        );
        templates.insert(
            TemplateKey::new(Language::Rust, PrimitiveType::Sink, "src/main.rs"),
            RUST_SINK_MAIN_RS,
        );
        files_by_type.insert(
            (Language::Rust, PrimitiveType::Sink),
            vec!["Cargo.toml", "src/main.rs"],
        );

        Self { templates, files_by_type }
    }

    /// Gets a template by language, primitive type, and filename.
    #[must_use]
    pub fn get(&self, language: Language, primitive_type: PrimitiveType, filename: &str) -> Option<&'static str> {
        // Look for exact match first
        for (key, template) in &self.templates {
            if key.language == language && key.primitive_type == primitive_type && key.filename == filename {
                return Some(*template);
            }
        }
        None
    }

    /// Gets the list of files to generate for a language/primitive type combination.
    #[must_use]
    pub fn files_for(&self, language: Language, primitive_type: PrimitiveType) -> Vec<&'static str> {
        self.files_by_type
            .get(&(language, primitive_type))
            .cloned()
            .unwrap_or_default()
    }

    /// Checks if templates exist for a given language.
    #[must_use]
    pub fn has_language(&self, language: Language) -> bool {
        self.files_by_type.keys().any(|(lang, _)| *lang == language)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_has_rust_templates() {
        let registry = TemplateRegistry::new();

        assert!(registry.has_language(Language::Rust));
        assert!(!registry.has_language(Language::TypeScript));
        assert!(!registry.has_language(Language::Python));
    }

    #[test]
    fn test_registry_returns_correct_files() {
        let registry = TemplateRegistry::new();

        let source_files = registry.files_for(Language::Rust, PrimitiveType::Source);
        assert_eq!(source_files, vec!["Cargo.toml", "src/main.rs"]);

        let handler_files = registry.files_for(Language::Rust, PrimitiveType::Handler);
        assert_eq!(handler_files, vec!["Cargo.toml", "src/main.rs"]);

        let sink_files = registry.files_for(Language::Rust, PrimitiveType::Sink);
        assert_eq!(sink_files, vec!["Cargo.toml", "src/main.rs"]);
    }

    #[test]
    fn test_get_template_returns_content() {
        let registry = TemplateRegistry::new();

        let cargo = registry.get(Language::Rust, PrimitiveType::Source, "Cargo.toml");
        assert!(cargo.is_some());
        assert!(cargo.is_some_and(|t| t.contains("[package]")));
    }
}
