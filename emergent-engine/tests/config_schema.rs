//! Every configuration the project ships must load under the strict schema.
//!
//! `deny_unknown_fields` turns a typo into a load error, which also means a key
//! this repository documents but the engine never added would break every user
//! who copied it. These tests parse the shipped configs and the full-config
//! snippets in the docs, so such a key cannot survive a review.

use emergent_engine::config::EmergentConfig;
use std::path::{Path, PathBuf};

/// Repository root, derived from this crate's manifest directory.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Collect files under `dir` whose name matches `matches`.
fn files_under(dir: &Path, matches: &dyn Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_under(&path, matches));
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if matches(&name) {
            found.push(path);
        }
    }
    found.sort();
    found
}

/// Returns whether a file name is an engine configuration rather than a manifest.
fn is_engine_config(name: &str) -> bool {
    if name == "Cargo.toml" || name == "pyproject.toml" {
        return false;
    }
    name.ends_with(".toml") || name.ends_with(".toml.example")
}

/// Extract fenced ```toml blocks that look like whole engine configurations.
///
/// A snippet counts when it opens an `[engine]` or `[event_store]` table, which
/// is what an operator copies wholesale. Fragments that show a single primitive
/// are left alone, because they are deliberately incomplete.
fn full_config_snippets(markdown: &str) -> Vec<String> {
    let mut snippets = Vec::new();
    let mut current: Option<String> = None;

    for line in markdown.lines() {
        match current.as_mut() {
            Some(block) => {
                if line.trim_start().starts_with("```") {
                    let finished = std::mem::take(block);
                    current = None;
                    if finished.contains("[engine]") || finished.contains("[event_store]") {
                        snippets.push(finished);
                    }
                } else {
                    block.push_str(line);
                    block.push('\n');
                }
            }
            None => {
                let fence = line.trim_start();
                if fence == "```toml" || fence == "``` toml" {
                    current = Some(String::new());
                }
            }
        }
    }

    snippets
}

#[test]
fn every_shipped_config_parses_under_the_strict_schema() {
    let root = repo_root();
    let mut checked = 0;

    for dir in ["config", "docker"] {
        for path in files_under(&root.join(dir), &is_engine_config) {
            let Ok(content) = std::fs::read_to_string(&path) else {
                panic!("could not read {}", path.display());
            };
            if let Err(error) = toml::from_str::<EmergentConfig>(&content) {
                panic!("{} does not load: {error}", path.display());
            }
            checked += 1;
        }
    }

    assert!(
        checked >= 8,
        "expected the shipped configs, found {checked}"
    );
}

#[test]
fn every_full_config_in_the_docs_parses_under_the_strict_schema() {
    let root = repo_root();
    let markdown = |name: &str| name.ends_with(".md");
    let mut sources = files_under(&root.join("docs"), &markdown);
    sources.extend(files_under(&root.join("skills"), &markdown));
    sources.extend(files_under(&root.join("docker"), &markdown));
    sources.push(root.join("README.md"));
    sources.push(root.join("CLAUDE.md"));

    let mut checked = 0;
    for path in sources {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for snippet in full_config_snippets(&content) {
            if let Err(error) = toml::from_str::<EmergentConfig>(&snippet) {
                panic!(
                    "a config snippet in {} does not load: {error}",
                    path.display()
                );
            }
            checked += 1;
        }
    }

    assert!(checked >= 4, "expected documented configs, found {checked}");
}

#[test]
fn the_generated_starter_config_parses_under_the_strict_schema()
-> Result<(), Box<dyn std::error::Error>> {
    let generated = emergent_engine::init::template::render_config_template("emergent")?;
    toml::from_str::<EmergentConfig>(&generated)?;
    Ok(())
}

#[test]
fn snippet_extraction_only_takes_whole_configs() {
    let markdown = "\
text

```toml
[engine]
name = \"demo\"
```

```toml
[[handlers]]
name = \"filter\"
```

```bash
echo '[engine]'
```
";

    let snippets = full_config_snippets(markdown);

    assert_eq!(snippets.len(), 1);
    assert!(snippets[0].contains("[engine]"));
}
