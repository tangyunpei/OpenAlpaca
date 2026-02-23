    use super::*;

    #[test]
    fn test_parse_toml_config() {
        let toml_str = r#"
[[tools]]
name = "weather_lookup"
description = "Get weather"

[tools.parameters]
type = "object"
required = ["location"]

[tools.parameters.properties.location]
type = "string"
description = "City name"

[tools.backend]
type = "http"
url = "https://api.example.com/weather?q={location}"
method = "GET"
timeout_secs = 10
"#;

        let config: ToolConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.tools.len(), 1);
        assert_eq!(config.tools[0].name, "weather_lookup");
        assert_eq!(config.tools[0].description, "Get weather");
        match &config.tools[0].backend {
            ToolBackendConfig::Http {
                url,
                method,
                timeout_secs,
                ..
            } => {
                assert!(url.contains("example.com"));
                assert_eq!(method.as_deref(), Some("GET"));
                assert_eq!(*timeout_secs, Some(10));
            }
            _ => panic!("Expected Http backend"),
        }
    }

    #[test]
    fn test_parse_command_backend() {
        let toml_str = r#"
[[tools]]
name = "git_log"
description = "Show git log"

[tools.parameters]
type = "object"

[tools.backend]
type = "command"
command = "git"
args_template = "log --oneline -n {count}"
timeout_secs = 15
"#;

        let config: ToolConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.tools.len(), 1);
        match &config.tools[0].backend {
            ToolBackendConfig::Command {
                command,
                args_template,
                timeout_secs,
            } => {
                assert_eq!(command, "git");
                assert_eq!(args_template.as_deref(), Some("log --oneline -n {count}"));
                assert_eq!(*timeout_secs, Some(15));
            }
            _ => panic!("Expected Command backend"),
        }
    }

    #[test]
    fn test_load_from_dir_nonexistent() {
        let tools = load_tools_from_dir(Path::new("/nonexistent/path"));
        assert!(tools.is_empty());
    }

    #[test]
    fn test_load_from_dir_with_file() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
[[tools]]
name = "test_tool"
description = "A test tool"

[tools.parameters]
type = "object"

[tools.backend]
type = "command"
command = "echo"
args_template = "hello"
"#;
        std::fs::write(dir.path().join("test.toml"), toml_content).unwrap();

        let tools = load_tools_from_dir(dir.path());
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].definition.name, "test_tool");
    }
