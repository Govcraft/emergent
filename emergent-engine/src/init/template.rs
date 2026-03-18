//! Template rendering for the init command.

/// Render the emergent.toml configuration template with the given engine name.
///
/// Uses minijinja to render the embedded Jinja2 template with a single
/// `name` variable substituted.
///
/// # Errors
///
/// Returns `minijinja::Error` if template rendering fails.
pub fn render_config_template(name: &str) -> Result<String, minijinja::Error> {
    let template_source = include_str!("../../templates/init/emergent.toml.j2");
    let mut env = minijinja::Environment::new();
    env.add_template("emergent.toml", template_source)?;
    let template = env.get_template("emergent.toml")?;
    template.render(minijinja::context! { name => name })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_default_name() -> Result<(), Box<dyn std::error::Error>> {
        let output = render_config_template("emergent")?;
        assert!(
            output.contains("name = \"emergent\""),
            "Expected default name in output"
        );
        Ok(())
    }

    #[test]
    fn test_render_custom_name() -> Result<(), Box<dyn std::error::Error>> {
        let output = render_config_template("my-pipeline")?;
        assert!(
            output.contains("name = \"my-pipeline\""),
            "Expected custom name in output"
        );
        Ok(())
    }

    #[test]
    fn test_render_produces_valid_toml() -> Result<(), Box<dyn std::error::Error>> {
        let output = render_config_template("emergent")?;
        let config: crate::config::EmergentConfig = toml::from_str(&output)?;
        assert_eq!(config.engine.name, "emergent");
        assert_eq!(config.engine.socket_path, "auto");
        assert_eq!(config.event_store.json_log_dir.to_string_lossy(), "auto");
        assert_eq!(config.event_store.sqlite_path.to_string_lossy(), "auto");
        assert_eq!(config.event_store.retention_days, 30);
        Ok(())
    }

    #[test]
    fn test_render_contains_commented_examples() -> Result<(), Box<dyn std::error::Error>> {
        let output = render_config_template("emergent")?;
        assert!(
            output.contains("# [[sources]]"),
            "Expected commented sources example"
        );
        assert!(
            output.contains("# [[handlers]]"),
            "Expected commented handlers example"
        );
        assert!(
            output.contains("# [[sinks]]"),
            "Expected commented sinks example"
        );
        Ok(())
    }
}
