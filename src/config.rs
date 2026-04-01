//! Configuration types for webhookd.
//!
//! Parses and validates a TOML configuration file whose schema is documented
//! in `docs/config.example.toml`.  Every struct uses `serde::Deserialize` so
//! that the TOML can be loaded in one step, and an explicit [`validate`] pass
//! checks invariants that cannot be expressed in the type system alone.

use serde::Deserialize;
use std::fmt;
use std::path::Path;
use thiserror::Error;

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors produced when loading or validating a configuration file.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to parse TOML: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("validation error: {0}")]
    Validation(String),
}

// ─── Log level ───────────────────────────────────────────────────────────────

/// Supported log verbosity levels.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Debug => write!(f, "debug"),
            Self::Info => write!(f, "info"),
            Self::Warn => write!(f, "warn"),
            Self::Error => write!(f, "error"),
        }
    }
}

// ─── Top-level config ────────────────────────────────────────────────────────

/// Root configuration object.
///
/// Corresponds to the top-level keys in a webhookd TOML file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookdConfig {
    /// **Mandatory.** TCP port for the HTTP server.
    pub port: u16,

    /// Optional.  Default: `false`.
    /// Restart the server automatically when the config file changes.
    #[serde(default, rename = "hot-reload")]
    pub hot_reload: bool,

    /// Optional.  Default: `LogLevel::Info`.
    #[serde(default)]
    pub log: LogLevel,

    /// At least one service must be defined.
    #[serde(default)]
    pub services: Vec<ServiceConfig>,
}

// ─── Service ─────────────────────────────────────────────────────────────────

/// A single webhook service definition.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    /// **Mandatory.** URL path for this webhook (e.g. `/webhook/api1`).
    pub endpoint: String,

    /// Optional MIME type hint (`"json"` or `"xml"`).
    pub mimetype: Option<String>,

    /// Inline input validation schema (JSON Schema or XSD).
    pub input: Option<String>,

    /// Inline output template (Jinja2-style).
    pub output: Option<String>,

    /// Path to a file containing the input schema.  Overrides `input`.
    #[serde(rename = "input-file")]
    pub input_file: Option<String>,

    /// Path to a file containing the output template.  Overrides `output`.
    #[serde(rename = "output-file")]
    pub output_file: Option<String>,

    /// Path to a shell script to execute.  Mutually exclusive with `proxy`.
    pub exec: Option<String>,

    /// URL to proxy requests to.  Mutually exclusive with `exec`.
    pub proxy: Option<String>,

    /// Optional authentication block(s).
    #[serde(default)]
    pub authenticate: Vec<AuthenticateConfig>,
}

// ─── Authentication ──────────────────────────────────────────────────────────

/// Authentication configuration for a service.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticateConfig {
    /// jq-style path to the secret inside a JSON body.
    pub json: Option<String>,

    /// XPath expression to the secret inside an XML body.
    pub xml: Option<String>,

    /// HTTP header name that carries the secret.
    #[serde(rename = "http-header")]
    pub http_header: Option<String>,

    /// Static secret string.
    pub secret: Option<String>,

    /// Name of an env-var holding the secret (takes precedence over `secret`).
    #[serde(rename = "secret-env-var")]
    pub secret_env_var: Option<String>,
}

// ─── Loading ─────────────────────────────────────────────────────────────────

impl WebhookdConfig {
    /// Load a configuration from a TOML file at `path`, then validate it.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        Self::parse(&content)
    }

    /// Parse and validate a configuration from an in-memory TOML string.
    pub fn parse(toml_str: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(toml_str)?;
        config.validate()?;
        Ok(config)
    }
}

// ─── Validation ──────────────────────────────────────────────────────────────

impl WebhookdConfig {
    /// Check invariants that go beyond what `serde` can enforce.
    fn validate(&self) -> Result<(), ConfigError> {
        if self.services.is_empty() {
            return Err(ConfigError::Validation(
                "at least one [[services]] block must be defined".into(),
            ));
        }

        for (i, svc) in self.services.iter().enumerate() {
            let ctx = format!("services[{}] (endpoint '{}')", i, svc.endpoint);

            // endpoint must not be empty
            if svc.endpoint.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "{ctx}: endpoint must not be empty"
                )));
            }

            // exactly one of exec / proxy
            match (&svc.exec, &svc.proxy) {
                (None, None) => {
                    return Err(ConfigError::Validation(format!(
                        "{ctx}: exactly one of `exec` or `proxy` must be set"
                    )));
                }
                (Some(_), Some(_)) => {
                    return Err(ConfigError::Validation(format!(
                        "{ctx}: `exec` and `proxy` are mutually exclusive"
                    )));
                }
                _ => {}
            }

            // mimetype value check
            if let Some(ref mt) = svc.mimetype {
                if mt != "json" && mt != "xml" {
                    return Err(ConfigError::Validation(format!(
                        "{ctx}: mimetype must be \"json\" or \"xml\", got \"{mt}\""
                    )));
                }
            }

            // validate authenticate blocks
            for (j, auth) in svc.authenticate.iter().enumerate() {
                let auth_ctx = format!("{ctx} authenticate[{j}]");

                // at most one lookup
                let lookup_count = [
                    auth.json.is_some(),
                    auth.xml.is_some(),
                    auth.http_header.is_some(),
                ]
                .iter()
                .filter(|&&v| v)
                .count();

                if lookup_count > 1 {
                    return Err(ConfigError::Validation(format!(
                        "{auth_ctx}: only one lookup field (json, xml, http-header) can be specified"
                    )));
                }

                // at least one secret source
                if auth.secret.is_none() && auth.secret_env_var.is_none() {
                    return Err(ConfigError::Validation(format!(
                        "{auth_ctx}: at least one of `secret` or `secret-env-var` must be set"
                    )));
                }

                // mimetype vs authenticate lookup conflict
                if let Some(ref mt) = svc.mimetype {
                    if auth.json.is_some() && mt != "json" {
                        return Err(ConfigError::Validation(format!(
                            "{auth_ctx}: authenticate uses `json` lookup but mimetype is \"{mt}\""
                        )));
                    }
                    if auth.xml.is_some() && mt != "xml" {
                        return Err(ConfigError::Validation(format!(
                            "{auth_ctx}: authenticate uses `xml` lookup but mimetype is \"{mt}\""
                        )));
                    }
                }
            }
        }

        Ok(())
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────

    /// Minimal valid TOML that passes validation.
    fn minimal_toml() -> String {
        r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
        "#
        .to_string()
    }

    // ── parsing: mandatory fields ───────────────────────────────────────

    #[test]
    fn parse_minimal_config() {
        let cfg = WebhookdConfig::parse(&minimal_toml()).unwrap();
        assert_eq!(cfg.port, 8080);
        assert!(!cfg.hot_reload);
        assert_eq!(cfg.log, LogLevel::Info);
        assert_eq!(cfg.services.len(), 1);
        assert_eq!(cfg.services[0].endpoint, "/hook");
        assert_eq!(cfg.services[0].exec.as_deref(), Some("/bin/true"));
        assert!(cfg.services[0].proxy.is_none());
    }

    #[test]
    fn missing_port_is_parse_error() {
        let toml = r#"
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Toml(_)));
    }

    // ── parsing: optional fields / defaults ─────────────────────────────

    #[test]
    fn defaults_for_optional_fields() {
        let cfg = WebhookdConfig::parse(&minimal_toml()).unwrap();
        assert!(!cfg.hot_reload, "hot_reload should default to false");
        assert_eq!(cfg.log, LogLevel::Info, "log should default to info");
        assert!(
            cfg.services[0].mimetype.is_none(),
            "mimetype defaults to None"
        );
        assert!(cfg.services[0].input.is_none());
        assert!(cfg.services[0].output.is_none());
        assert!(cfg.services[0].input_file.is_none());
        assert!(cfg.services[0].output_file.is_none());
        assert!(cfg.services[0].authenticate.is_empty());
    }

    #[test]
    fn hot_reload_can_be_set_true() {
        let toml = r#"
            port = 9090
            hot-reload = true
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert!(cfg.hot_reload);
    }

    #[test]
    fn log_levels_parse_correctly() {
        for level in &["debug", "info", "warn", "error"] {
            let toml = format!(
                r#"
                port = 8080
                log = "{level}"
                [[services]]
                endpoint = "/hook"
                exec = "/bin/true"
            "#
            );
            let cfg = WebhookdConfig::parse(&toml).unwrap();
            let expected = match *level {
                "debug" => LogLevel::Debug,
                "info" => LogLevel::Info,
                "warn" => LogLevel::Warn,
                "error" => LogLevel::Error,
                _ => unreachable!(),
            };
            assert_eq!(cfg.log, expected);
        }
    }

    #[test]
    fn invalid_log_level_is_parse_error() {
        let toml = r#"
            port = 8080
            log = "trace"
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Toml(_)));
    }

    // ── validation: services ────────────────────────────────────────────

    #[test]
    fn no_services_is_validation_error() {
        let toml = "port = 8080\n";
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
        let msg = err.to_string();
        assert!(msg.contains("at least one"));
    }

    #[test]
    fn empty_endpoint_is_validation_error() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = ""
            exec = "/bin/true"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
        let msg = err.to_string();
        assert!(msg.contains("endpoint must not be empty"));
    }

    // ── validation: exec / proxy exclusivity ────────────────────────────

    #[test]
    fn neither_exec_nor_proxy_is_error() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
        let msg = err.to_string();
        assert!(msg.contains("exactly one of `exec` or `proxy`"));
    }

    #[test]
    fn both_exec_and_proxy_is_error() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            proxy = "http://localhost"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
        let msg = err.to_string();
        assert!(msg.contains("mutually exclusive"));
    }

    #[test]
    fn only_exec_is_valid() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/usr/local/bin/deploy.sh"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert_eq!(
            cfg.services[0].exec.as_deref(),
            Some("/usr/local/bin/deploy.sh")
        );
        assert!(cfg.services[0].proxy.is_none());
    }

    #[test]
    fn only_proxy_is_valid() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            proxy = "http://backend:3000/api"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert!(cfg.services[0].exec.is_none());
        assert_eq!(
            cfg.services[0].proxy.as_deref(),
            Some("http://backend:3000/api")
        );
    }

    // ── validation: mimetype ────────────────────────────────────────────

    #[test]
    fn valid_mimetype_json() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            mimetype = "json"
            exec = "/bin/true"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert_eq!(cfg.services[0].mimetype.as_deref(), Some("json"));
    }

    #[test]
    fn valid_mimetype_xml() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            mimetype = "xml"
            exec = "/bin/true"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert_eq!(cfg.services[0].mimetype.as_deref(), Some("xml"));
    }

    #[test]
    fn invalid_mimetype_is_error() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            mimetype = "yaml"
            exec = "/bin/true"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
        let msg = err.to_string();
        assert!(msg.contains("mimetype must be"));
    }

    // ── input / input-file, output / output-file ────────────────────────

    #[test]
    fn input_and_output_inline() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            input = '{"type": "object"}'
            output = '{"result": "ok"}'
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert!(cfg.services[0].input.is_some());
        assert!(cfg.services[0].output.is_some());
    }

    #[test]
    fn input_file_and_output_file() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            input-file = "/etc/webhookd/input.json"
            output-file = "/etc/webhookd/output.json"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert_eq!(
            cfg.services[0].input_file.as_deref(),
            Some("/etc/webhookd/input.json")
        );
        assert_eq!(
            cfg.services[0].output_file.as_deref(),
            Some("/etc/webhookd/output.json")
        );
    }

    #[test]
    fn both_input_and_input_file_parses_ok() {
        // input-file overrides input at runtime, but both may be present
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            input = '{"type": "object"}'
            input-file = "/etc/webhookd/input.json"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert!(cfg.services[0].input.is_some());
        assert!(cfg.services[0].input_file.is_some());
    }

    // ── authenticate ────────────────────────────────────────────────────

    #[test]
    fn authenticate_with_secret() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            [[services.authenticate]]
            secret = "s3cret"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert_eq!(cfg.services[0].authenticate.len(), 1);
        assert_eq!(
            cfg.services[0].authenticate[0].secret.as_deref(),
            Some("s3cret")
        );
    }

    #[test]
    fn authenticate_with_secret_env_var() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            [[services.authenticate]]
            secret-env-var = "MY_SECRET"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert_eq!(
            cfg.services[0].authenticate[0].secret_env_var.as_deref(),
            Some("MY_SECRET")
        );
    }

    #[test]
    fn authenticate_both_secret_sources_allowed() {
        // secret-env-var takes precedence at runtime
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            [[services.authenticate]]
            secret = "fallback"
            secret-env-var = "MY_SECRET"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        let auth = &cfg.services[0].authenticate[0];
        assert!(auth.secret.is_some());
        assert!(auth.secret_env_var.is_some());
    }

    #[test]
    fn authenticate_no_secret_is_error() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            [[services.authenticate]]
            json = "path.to.secret"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
        let msg = err.to_string();
        assert!(msg.contains("secret"));
    }

    // ── authenticate: lookup exclusivity ────────────────────────────────

    #[test]
    fn authenticate_json_lookup_only() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            [[services.authenticate]]
            json = "body.secret"
            secret = "abc"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert!(cfg.services[0].authenticate[0].json.is_some());
        assert!(cfg.services[0].authenticate[0].xml.is_none());
        assert!(cfg.services[0].authenticate[0].http_header.is_none());
    }

    #[test]
    fn authenticate_xml_lookup_only() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            [[services.authenticate]]
            xml = "//secret"
            secret = "abc"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert!(cfg.services[0].authenticate[0].xml.is_some());
    }

    #[test]
    fn authenticate_http_header_lookup_only() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            [[services.authenticate]]
            http-header = "X-My-Token"
            secret = "abc"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert_eq!(
            cfg.services[0].authenticate[0].http_header.as_deref(),
            Some("X-My-Token")
        );
    }

    #[test]
    fn authenticate_no_lookup_is_ok() {
        // defaults to http-header = "X-Webhook-Secret" at runtime
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            [[services.authenticate]]
            secret = "abc"
        "#;
        WebhookdConfig::parse(toml).unwrap();
    }

    #[test]
    fn authenticate_multiple_lookups_is_error() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            [[services.authenticate]]
            json = "body.secret"
            xml = "//secret"
            secret = "abc"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
        let msg = err.to_string();
        assert!(msg.contains("only one lookup"));
    }

    #[test]
    fn authenticate_json_and_header_is_error() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            [[services.authenticate]]
            json = "body.secret"
            http-header = "X-Token"
            secret = "abc"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    // ── mimetype vs authenticate conflict ───────────────────────────────

    #[test]
    fn mimetype_json_with_json_lookup_is_ok() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            mimetype = "json"
            exec = "/bin/true"
            [[services.authenticate]]
            json = "body.secret"
            secret = "abc"
        "#;
        WebhookdConfig::parse(toml).unwrap();
    }

    #[test]
    fn mimetype_xml_with_xml_lookup_is_ok() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            mimetype = "xml"
            exec = "/bin/true"
            [[services.authenticate]]
            xml = "//secret"
            secret = "abc"
        "#;
        WebhookdConfig::parse(toml).unwrap();
    }

    #[test]
    fn mimetype_json_with_xml_lookup_is_error() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            mimetype = "json"
            exec = "/bin/true"
            [[services.authenticate]]
            xml = "//secret"
            secret = "abc"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
        let msg = err.to_string();
        assert!(msg.contains("xml"));
        assert!(msg.contains("json"));
    }

    #[test]
    fn mimetype_xml_with_json_lookup_is_error() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            mimetype = "xml"
            exec = "/bin/true"
            [[services.authenticate]]
            json = "body.secret"
            secret = "abc"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    // ── multiple services ───────────────────────────────────────────────

    #[test]
    fn multiple_services_parse() {
        let toml = r#"
            port = 3000
            log = "debug"

            [[services]]
            endpoint = "/hook/a"
            exec = "/scripts/a.sh"

            [[services]]
            endpoint = "/hook/b"
            proxy = "http://backend:4000"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert_eq!(cfg.services.len(), 2);
        assert_eq!(cfg.services[0].endpoint, "/hook/a");
        assert_eq!(cfg.services[1].endpoint, "/hook/b");
    }

    // ── full-featured config ────────────────────────────────────────────

    #[test]
    fn full_featured_config() {
        let toml = r#"
            port = 8080
            hot-reload = true
            log = "debug"

            [[services]]
            endpoint = "/webhook/api1"
            mimetype = "json"
            exec = "/webhookd/scripts/api1.sh"
            input = '{"type": "object"}'
            output = '{"result": "ok"}'

            [[services.authenticate]]
            json = "example.path[0].secret"
            secret = "THIS-IS-YOUR-STATIC-SECRET"

            [[services]]
            endpoint = "/webhook/api2"
            proxy = "http://hostname/api/service"
            input-file = "/webhookd/templates/input"
            output-file = "/webhookd/templates/output"

            [[services.authenticate]]
            http-header = "X-Webhook-Secret"
            secret-env-var = "WEBHOOKD_SECRET"
        "#;
        let cfg = WebhookdConfig::parse(toml).unwrap();
        assert_eq!(cfg.port, 8080);
        assert!(cfg.hot_reload);
        assert_eq!(cfg.log, LogLevel::Debug);
        assert_eq!(cfg.services.len(), 2);

        let svc1 = &cfg.services[0];
        assert_eq!(svc1.endpoint, "/webhook/api1");
        assert_eq!(svc1.mimetype.as_deref(), Some("json"));
        assert_eq!(svc1.exec.as_deref(), Some("/webhookd/scripts/api1.sh"));
        assert!(svc1.input.is_some());
        assert!(svc1.output.is_some());
        assert_eq!(svc1.authenticate.len(), 1);
        assert!(svc1.authenticate[0].json.is_some());
        assert!(svc1.authenticate[0].secret.is_some());

        let svc2 = &cfg.services[1];
        assert_eq!(svc2.endpoint, "/webhook/api2");
        assert_eq!(svc2.proxy.as_deref(), Some("http://hostname/api/service"));
        assert_eq!(
            svc2.input_file.as_deref(),
            Some("/webhookd/templates/input")
        );
        assert_eq!(
            svc2.output_file.as_deref(),
            Some("/webhookd/templates/output")
        );
        assert_eq!(svc2.authenticate.len(), 1);
        assert!(svc2.authenticate[0].http_header.is_some());
        assert!(svc2.authenticate[0].secret_env_var.is_some());
    }

    // ── unknown fields are rejected ─────────────────────────────────────

    #[test]
    fn unknown_top_level_key_is_error() {
        let toml = r#"
            port = 8080
            unknown_key = "hello"
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Toml(_)));
    }

    #[test]
    fn unknown_service_key_is_error() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            unknown = true
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Toml(_)));
    }

    #[test]
    fn unknown_authenticate_key_is_error() {
        let toml = r#"
            port = 8080
            [[services]]
            endpoint = "/hook"
            exec = "/bin/true"
            [[services.authenticate]]
            secret = "abc"
            unknown = true
        "#;
        let err = WebhookdConfig::parse(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Toml(_)));
    }

    // ── from_file ───────────────────────────────────────────────────────

    #[test]
    fn from_file_loads_example_config() {
        // The example config file ships commented-out optional fields,
        // but the uncommented values should still parse correctly.
        let path = std::path::Path::new("docs/config.example.toml");
        let cfg = WebhookdConfig::from_file(path).unwrap();
        assert_eq!(cfg.port, 8080);
        assert!(!cfg.hot_reload);
        assert_eq!(cfg.log, LogLevel::Info);
        assert_eq!(cfg.services.len(), 1);
        assert_eq!(cfg.services[0].endpoint, "/webhook/api1");
    }

    #[test]
    fn from_file_missing_file_is_io_error() {
        let err = WebhookdConfig::from_file(Path::new("/nonexistent.toml")).unwrap_err();
        assert!(matches!(err, ConfigError::Io(_)));
    }

    // ── LogLevel display ────────────────────────────────────────────────

    #[test]
    fn log_level_display() {
        assert_eq!(LogLevel::Debug.to_string(), "debug");
        assert_eq!(LogLevel::Info.to_string(), "info");
        assert_eq!(LogLevel::Warn.to_string(), "warn");
        assert_eq!(LogLevel::Error.to_string(), "error");
    }

    #[test]
    fn log_level_default_is_info() {
        assert_eq!(LogLevel::default(), LogLevel::Info);
    }
}
