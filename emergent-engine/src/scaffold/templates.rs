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

/// Embedded TypeScript source templates.
const TS_SOURCE_DENO_JSON: &str = include_str!("../../templates/typescript/source/deno.json.j2");
const TS_SOURCE_MAIN_TS: &str = include_str!("../../templates/typescript/source/main.ts.j2");

/// Embedded TypeScript handler templates.
const TS_HANDLER_DENO_JSON: &str = include_str!("../../templates/typescript/handler/deno.json.j2");
const TS_HANDLER_MAIN_TS: &str = include_str!("../../templates/typescript/handler/main.ts.j2");

/// Embedded TypeScript sink templates.
const TS_SINK_DENO_JSON: &str = include_str!("../../templates/typescript/sink/deno.json.j2");
const TS_SINK_MAIN_TS: &str = include_str!("../../templates/typescript/sink/main.ts.j2");

/// Embedded Python source templates.
const PY_SOURCE_PYPROJECT_TOML: &str = include_str!("../../templates/python/source/pyproject.toml.j2");
const PY_SOURCE_MAIN_PY: &str = include_str!("../../templates/python/source/main.py.j2");

/// Embedded Python handler templates.
const PY_HANDLER_PYPROJECT_TOML: &str = include_str!("../../templates/python/handler/pyproject.toml.j2");
const PY_HANDLER_MAIN_PY: &str = include_str!("../../templates/python/handler/main.py.j2");

/// Embedded Python sink templates.
const PY_SINK_PYPROJECT_TOML: &str = include_str!("../../templates/python/sink/pyproject.toml.j2");
const PY_SINK_MAIN_PY: &str = include_str!("../../templates/python/sink/main.py.j2");

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

        // TypeScript source templates
        templates.insert(
            TemplateKey::new(Language::TypeScript, PrimitiveType::Source, "deno.json"),
            TS_SOURCE_DENO_JSON,
        );
        templates.insert(
            TemplateKey::new(Language::TypeScript, PrimitiveType::Source, "main.ts"),
            TS_SOURCE_MAIN_TS,
        );
        files_by_type.insert(
            (Language::TypeScript, PrimitiveType::Source),
            vec!["deno.json", "main.ts"],
        );

        // TypeScript handler templates
        templates.insert(
            TemplateKey::new(Language::TypeScript, PrimitiveType::Handler, "deno.json"),
            TS_HANDLER_DENO_JSON,
        );
        templates.insert(
            TemplateKey::new(Language::TypeScript, PrimitiveType::Handler, "main.ts"),
            TS_HANDLER_MAIN_TS,
        );
        files_by_type.insert(
            (Language::TypeScript, PrimitiveType::Handler),
            vec!["deno.json", "main.ts"],
        );

        // TypeScript sink templates
        templates.insert(
            TemplateKey::new(Language::TypeScript, PrimitiveType::Sink, "deno.json"),
            TS_SINK_DENO_JSON,
        );
        templates.insert(
            TemplateKey::new(Language::TypeScript, PrimitiveType::Sink, "main.ts"),
            TS_SINK_MAIN_TS,
        );
        files_by_type.insert(
            (Language::TypeScript, PrimitiveType::Sink),
            vec!["deno.json", "main.ts"],
        );

        // Python source templates
        templates.insert(
            TemplateKey::new(Language::Python, PrimitiveType::Source, "pyproject.toml"),
            PY_SOURCE_PYPROJECT_TOML,
        );
        templates.insert(
            TemplateKey::new(Language::Python, PrimitiveType::Source, "main.py"),
            PY_SOURCE_MAIN_PY,
        );
        files_by_type.insert(
            (Language::Python, PrimitiveType::Source),
            vec!["pyproject.toml", "main.py"],
        );

        // Python handler templates
        templates.insert(
            TemplateKey::new(Language::Python, PrimitiveType::Handler, "pyproject.toml"),
            PY_HANDLER_PYPROJECT_TOML,
        );
        templates.insert(
            TemplateKey::new(Language::Python, PrimitiveType::Handler, "main.py"),
            PY_HANDLER_MAIN_PY,
        );
        files_by_type.insert(
            (Language::Python, PrimitiveType::Handler),
            vec!["pyproject.toml", "main.py"],
        );

        // Python sink templates
        templates.insert(
            TemplateKey::new(Language::Python, PrimitiveType::Sink, "pyproject.toml"),
            PY_SINK_PYPROJECT_TOML,
        );
        templates.insert(
            TemplateKey::new(Language::Python, PrimitiveType::Sink, "main.py"),
            PY_SINK_MAIN_PY,
        );
        files_by_type.insert(
            (Language::Python, PrimitiveType::Sink),
            vec!["pyproject.toml", "main.py"],
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
        assert!(registry.has_language(Language::TypeScript));
        assert!(registry.has_language(Language::Python));
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

    #[test]
    fn test_typescript_file_lists() {
        let registry = TemplateRegistry::new();

        let source = registry.files_for(Language::TypeScript, PrimitiveType::Source);
        assert_eq!(source, vec!["deno.json", "main.ts"]);

        let handler = registry.files_for(Language::TypeScript, PrimitiveType::Handler);
        assert_eq!(handler, vec!["deno.json", "main.ts"]);

        let sink = registry.files_for(Language::TypeScript, PrimitiveType::Sink);
        assert_eq!(sink, vec!["deno.json", "main.ts"]);
    }

    #[test]
    fn test_python_file_lists() {
        let registry = TemplateRegistry::new();

        let source = registry.files_for(Language::Python, PrimitiveType::Source);
        assert_eq!(source, vec!["pyproject.toml", "main.py"]);

        let handler = registry.files_for(Language::Python, PrimitiveType::Handler);
        assert_eq!(handler, vec!["pyproject.toml", "main.py"]);

        let sink = registry.files_for(Language::Python, PrimitiveType::Sink);
        assert_eq!(sink, vec!["pyproject.toml", "main.py"]);
    }

    #[test]
    fn test_typescript_templates_contain_expected_content() {
        let registry = TemplateRegistry::new();

        let source_main = registry.get(Language::TypeScript, PrimitiveType::Source, "main.ts");
        assert!(source_main.is_some_and(|t| t.contains("EmergentSource")));

        let handler_main = registry.get(Language::TypeScript, PrimitiveType::Handler, "main.ts");
        assert!(handler_main.is_some_and(|t| t.contains("EmergentHandler")));

        let sink_main = registry.get(Language::TypeScript, PrimitiveType::Sink, "main.ts");
        assert!(sink_main.is_some_and(|t| t.contains("EmergentSink")));
    }

    #[test]
    fn test_python_templates_contain_expected_content() {
        let registry = TemplateRegistry::new();

        let source_main = registry.get(Language::Python, PrimitiveType::Source, "main.py");
        assert!(source_main.is_some_and(|t| t.contains("EmergentSource")));

        let handler_main = registry.get(Language::Python, PrimitiveType::Handler, "main.py");
        assert!(handler_main.is_some_and(|t| t.contains("EmergentHandler")));

        let sink_main = registry.get(Language::Python, PrimitiveType::Sink, "main.py");
        assert!(sink_main.is_some_and(|t| t.contains("EmergentSink")));
    }
}
