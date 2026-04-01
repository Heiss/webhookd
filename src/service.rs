//! Webhook service handler.
//!
//! Implements the request processing pipeline for each configured service:
//!
//! 1. **Authentication** — verify the request carries a valid secret
//! 2. **Schema validation** — validate the payload against JSON Schema
//! 3. **Variable extraction** — capture `assign_value` fields from the payload
//! 4. **Template rendering** — render the output template with extracted variables
//! 5. **Action execution** — run the configured script (`exec`) or proxy the request

use crate::auth;
use crate::config::{AuthenticateConfig, ServiceConfig};
use crate::schema::InputSchema;
use crate::template;
use crate::xml_schema::XmlInputSchema;
use axum::http::{HeaderMap, StatusCode};
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;
use tokio::process::Command;

/// Errors produced during service request processing.
#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("authentication failed: {0}")]
    AuthFailed(#[from] auth::AuthError),

    #[error("schema validation error: {0}")]
    SchemaError(#[from] crate::schema::SchemaError),

    #[error("XML schema validation error: {0}")]
    XmlSchemaError(#[from] crate::xml_schema::XmlSchemaError),

    #[error("template rendering error: {0}")]
    TemplateError(#[from] template::TemplateError),

    #[error("script execution failed: {0}")]
    ExecFailed(String),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("invalid JSON body: {0}")]
    InvalidJson(String),
}

impl ServiceError {
    /// Map error to an appropriate HTTP status code.
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::AuthFailed(_) => StatusCode::UNAUTHORIZED,
            Self::SchemaError(_) | Self::XmlSchemaError(_) | Self::InvalidJson(_) => {
                StatusCode::BAD_REQUEST
            }
            Self::TemplateError(_) | Self::ExecFailed(_) | Self::IoError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

/// A compiled, ready-to-handle webhook service.
///
/// Created from a [`ServiceConfig`] at startup.  Holds compiled schemas and
/// resolved templates so that request handling is fast.
#[derive(Debug, Clone)]
pub struct Service {
    /// The endpoint path (e.g. `/webhook/api1`).
    pub endpoint: String,
    /// Compiled JSON input schema (if configured and mimetype is json).
    input_schema: Option<InputSchema>,
    /// Compiled XML input schema (if configured and mimetype is xml).
    xml_input_schema: Option<XmlInputSchema>,
    /// Output template string (if configured).
    output_template: Option<String>,
    /// Script to execute (mutually exclusive with `proxy_url`).
    exec_command: Option<String>,
    /// URL to proxy to (mutually exclusive with `exec_command`).
    proxy_url: Option<String>,
    /// Authentication configurations.
    authenticate: Vec<AuthenticateConfig>,
    /// Effective MIME type for the service.
    mimetype: Option<String>,
}

impl Service {
    /// Build a [`Service`] from a validated [`ServiceConfig`].
    ///
    /// This compiles the input schema (if any) and resolves input/output
    /// from files if `input-file` / `output-file` are set.
    pub fn from_config(config: &ServiceConfig) -> Result<Self, ServiceError> {
        // Resolve input schema: input-file takes precedence over input
        let input_str = if let Some(ref path) = config.input_file {
            Some(std::fs::read_to_string(path).map_err(|e| {
                ServiceError::IoError(std::io::Error::new(
                    e.kind(),
                    format!("failed to read input-file '{path}': {e}"),
                ))
            })?)
        } else {
            config.input.clone()
        };

        // Determine effective mimetype: explicit or inferred from schema content
        let effective_mimetype = config.mimetype.clone().or_else(|| {
            input_str.as_ref().and_then(|s| {
                let trimmed = s.trim();
                if trimmed.starts_with('{') {
                    Some("json".to_string())
                } else if trimmed.starts_with('<') {
                    Some("xml".to_string())
                } else {
                    None
                }
            })
        });

        // Compile the appropriate schema based on mimetype
        let mut input_schema = None;
        let mut xml_input_schema = None;

        if let Some(ref schema_str) = input_str {
            match effective_mimetype.as_deref() {
                Some("xml") => {
                    xml_input_schema = Some(XmlInputSchema::compile(schema_str)?);
                }
                _ => {
                    // Default to JSON schema
                    input_schema = Some(InputSchema::compile(schema_str)?);
                }
            }
        }

        // Resolve output template: output-file takes precedence over output
        let output_template = if let Some(ref path) = config.output_file {
            Some(std::fs::read_to_string(path).map_err(|e| {
                ServiceError::IoError(std::io::Error::new(
                    e.kind(),
                    format!("failed to read output-file '{path}': {e}"),
                ))
            })?)
        } else {
            config.output.clone()
        };

        Ok(Self {
            endpoint: config.endpoint.clone(),
            input_schema,
            xml_input_schema,
            output_template,
            exec_command: config.exec.clone(),
            proxy_url: config.proxy.clone(),
            authenticate: config.authenticate.clone(),
            mimetype: effective_mimetype,
        })
    }

    /// Handle an incoming request.
    ///
    /// Runs the full pipeline: authenticate → validate → extract → template → exec.
    /// Returns the response body string.
    pub async fn handle_request(
        &self,
        headers: &HeaderMap,
        body: &str,
    ) -> Result<String, ServiceError> {
        // 1. Parse body as JSON if we have JSON schema or JSON auth
        let parsed_body: Option<Value> = if self.needs_json_body() {
            if body.is_empty() {
                None
            } else {
                Some(
                    serde_json::from_str(body)
                        .map_err(|e| ServiceError::InvalidJson(e.to_string()))?,
                )
            }
        } else {
            None
        };

        // 2. Authentication
        for auth_config in &self.authenticate {
            auth::check_auth(auth_config, headers, parsed_body.as_ref(), body)?;
        }

        // 3. Schema validation + variable extraction
        let vars: HashMap<String, Value> = if let Some(ref schema) = self.input_schema {
            schema.validate_and_extract(body)?
        } else if let Some(ref xml_schema) = self.xml_input_schema {
            xml_schema.validate_and_extract(body)?
        } else {
            HashMap::new()
        };

        // 4. Template rendering (if output template is configured)
        let rendered_output = if let Some(ref tmpl) = self.output_template {
            Some(template::render(tmpl, &vars)?)
        } else {
            None
        };

        // 5. Execute action
        if let Some(ref cmd) = self.exec_command {
            let output = self
                .execute_script(cmd, body, rendered_output.as_deref())
                .await?;
            Ok(output)
        } else if let Some(ref _proxy_url) = self.proxy_url {
            // Proxy support is a placeholder for future implementation
            Ok(format!("proxy to {} is not yet implemented", _proxy_url))
        } else {
            // Should not happen if config validation passed
            Err(ServiceError::ExecFailed(
                "no exec or proxy configured".into(),
            ))
        }
    }

    /// Check if this service needs the body parsed as JSON.
    fn needs_json_body(&self) -> bool {
        // Need JSON if we have a JSON schema, or if any auth uses JSON path lookup
        self.input_schema.is_some()
            || self.mimetype.as_deref() == Some("json")
            || self.authenticate.iter().any(|a| a.json.is_some())
    }

    /// Execute a shell script, passing the request body via stdin and
    /// the rendered output (if any) as the `WEBHOOKD_OUTPUT` environment variable.
    async fn execute_script(
        &self,
        cmd: &str,
        body: &str,
        rendered_output: Option<&str>,
    ) -> Result<String, ServiceError> {
        let mut command = Command::new("sh");
        command.arg("-c").arg(cmd);

        // Pass body via WEBHOOKD_BODY env var
        command.env("WEBHOOKD_BODY", body);

        // Pass rendered output via WEBHOOKD_OUTPUT env var if available
        if let Some(output) = rendered_output {
            command.env("WEBHOOKD_OUTPUT", output);
        }

        let output = command.output().await?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        if output.status.success() {
            Ok(stdout)
        } else {
            Err(ServiceError::ExecFailed(format!(
                "script exited with {}: {}",
                output.status, stderr
            )))
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServiceConfig;

    fn minimal_service_config() -> ServiceConfig {
        ServiceConfig {
            endpoint: "/test".to_string(),
            mimetype: None,
            input: None,
            output: None,
            input_file: None,
            output_file: None,
            exec: Some("echo hello".to_string()),
            proxy: None,
            authenticate: vec![],
        }
    }

    // ── from_config ─────────────────────────────────────────────────────

    #[test]
    fn from_config_minimal() {
        let config = minimal_service_config();
        let svc = Service::from_config(&config).unwrap();
        assert_eq!(svc.endpoint, "/test");
        assert!(svc.input_schema.is_none());
        assert!(svc.output_template.is_none());
        assert_eq!(svc.exec_command.as_deref(), Some("echo hello"));
    }

    #[test]
    fn from_config_with_schema() {
        let mut config = minimal_service_config();
        config.input = Some(
            r#"{"type": "object", "properties": {"id": {"type": "integer", "assign_value": "my-id"}}}"#.to_string()
        );
        let svc = Service::from_config(&config).unwrap();
        assert!(svc.input_schema.is_some());
    }

    #[test]
    fn from_config_with_output_template() {
        let mut config = minimal_service_config();
        config.output = Some(r#"{"result": {{ id }}}"#.to_string());
        let svc = Service::from_config(&config).unwrap();
        assert!(svc.output_template.is_some());
    }

    #[test]
    fn from_config_invalid_schema_is_error() {
        let mut config = minimal_service_config();
        config.input = Some("not valid json".to_string());
        let err = Service::from_config(&config).unwrap_err();
        assert!(matches!(err, ServiceError::SchemaError(_)));
    }

    // ── handle_request ──────────────────────────────────────────────────

    #[tokio::test]
    async fn handle_simple_exec() {
        let config = minimal_service_config();
        let svc = Service::from_config(&config).unwrap();
        let headers = HeaderMap::new();

        let result = svc.handle_request(&headers, "").await.unwrap();
        assert_eq!(result.trim(), "hello");
    }

    #[tokio::test]
    async fn handle_exec_receives_body_env() {
        let mut config = minimal_service_config();
        config.exec = Some("echo $WEBHOOKD_BODY".to_string());
        let svc = Service::from_config(&config).unwrap();
        let headers = HeaderMap::new();

        let result = svc.handle_request(&headers, "test-body").await.unwrap();
        assert_eq!(result.trim(), "test-body");
    }

    #[tokio::test]
    async fn handle_with_schema_validation_pass() {
        let mut config = minimal_service_config();
        config.input =
            Some(r#"{"type": "object", "properties": {"id": {"type": "integer"}}}"#.to_string());
        config.exec = Some("echo valid".to_string());
        let svc = Service::from_config(&config).unwrap();
        let headers = HeaderMap::new();

        let result = svc.handle_request(&headers, r#"{"id": 42}"#).await.unwrap();
        assert_eq!(result.trim(), "valid");
    }

    #[tokio::test]
    async fn handle_with_schema_validation_fail() {
        let mut config = minimal_service_config();
        config.input = Some(
            r#"{"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}"#
                .to_string(),
        );
        let svc = Service::from_config(&config).unwrap();
        let headers = HeaderMap::new();

        let err = svc
            .handle_request(&headers, r#"{"id": "not-int"}"#)
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::SchemaError(_)));
    }

    #[tokio::test]
    async fn handle_with_output_template() {
        let mut config = minimal_service_config();
        config.input = Some(
            r#"{"type": "object", "properties": {"id": {"type": "integer", "assign_value": "my_id"}}}"#
                .to_string(),
        );
        config.output = Some("id={{ my_id }}".to_string());
        config.exec = Some("echo $WEBHOOKD_OUTPUT".to_string());
        let svc = Service::from_config(&config).unwrap();
        let headers = HeaderMap::new();

        let result = svc.handle_request(&headers, r#"{"id": 42}"#).await.unwrap();
        assert_eq!(result.trim(), "id=42");
    }

    #[tokio::test]
    async fn handle_with_auth_success() {
        let mut config = minimal_service_config();
        config.authenticate = vec![AuthenticateConfig {
            json: None,
            xml: None,
            http_header: Some("X-Token".to_string()),
            secret: Some("s3cret".to_string()),
            secret_env_var: None,
        }];

        let svc = Service::from_config(&config).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("X-Token", "s3cret".parse().unwrap());

        let result = svc.handle_request(&headers, "").await.unwrap();
        assert_eq!(result.trim(), "hello");
    }

    #[tokio::test]
    async fn handle_with_auth_fail() {
        let mut config = minimal_service_config();
        config.authenticate = vec![AuthenticateConfig {
            json: None,
            xml: None,
            http_header: Some("X-Token".to_string()),
            secret: Some("s3cret".to_string()),
            secret_env_var: None,
        }];

        let svc = Service::from_config(&config).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("X-Token", "wrong".parse().unwrap());

        let err = svc.handle_request(&headers, "").await.unwrap_err();
        assert!(matches!(err, ServiceError::AuthFailed(_)));
        assert_eq!(err.status_code(), StatusCode::UNAUTHORIZED);
    }

    // ── full pipeline test ──────────────────────────────────────────────

    #[tokio::test]
    async fn full_pipeline_schema_to_template_to_exec() {
        let config = ServiceConfig {
            endpoint: "/webhook/test".to_string(),
            mimetype: Some("json".to_string()),
            input: Some(
                r#"{
                    "type": "object",
                    "properties": {
                        "productId": {
                            "type": "integer",
                            "assign_value": "pid"
                        },
                        "name": {
                            "type": "string",
                            "assign_value": "pname"
                        }
                    },
                    "required": ["productId"]
                }"#
                .to_string(),
            ),
            output: Some("Product {{ pid }}: {{ pname }}".to_string()),
            input_file: None,
            output_file: None,
            exec: Some("echo $WEBHOOKD_OUTPUT".to_string()),
            proxy: None,
            authenticate: vec![AuthenticateConfig {
                json: None,
                xml: None,
                http_header: Some("X-Auth".to_string()),
                secret: Some("test-secret".to_string()),
                secret_env_var: None,
            }],
        };

        let svc = Service::from_config(&config).unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("X-Auth", "test-secret".parse().unwrap());

        let body = r#"{"productId": 42, "name": "Widget"}"#;
        let result = svc.handle_request(&headers, body).await.unwrap();
        assert_eq!(result.trim(), "Product 42: Widget");
    }

    // ── status codes ────────────────────────────────────────────────────

    #[test]
    fn status_code_mapping() {
        assert_eq!(
            ServiceError::AuthFailed(auth::AuthError::Unauthorized("x".into())).status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            ServiceError::InvalidJson("x".into()).status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ServiceError::ExecFailed("x".into()).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    // ── XML schema tests ────────────────────────────────────────────────

    #[test]
    fn from_config_with_xml_schema() {
        let mut config = minimal_service_config();
        config.mimetype = Some("xml".to_string());
        config.input = Some(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                        xmlns:whd="https://webhookd.dev/schema">
                <xs:element name="data">
                    <xs:complexType>
                        <xs:sequence>
                            <xs:element name="id" type="xs:integer"
                                        whd:assign_value="my_id"/>
                        </xs:sequence>
                    </xs:complexType>
                </xs:element>
            </xs:schema>"#
                .to_string(),
        );
        let svc = Service::from_config(&config).unwrap();
        assert!(svc.xml_input_schema.is_some());
        assert!(svc.input_schema.is_none());
    }

    #[test]
    fn from_config_auto_detects_xml_schema() {
        let mut config = minimal_service_config();
        // No explicit mimetype, but schema starts with '<'
        config.input = Some(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
                <xs:element name="data">
                    <xs:complexType>
                        <xs:sequence>
                            <xs:element name="id" type="xs:integer"/>
                        </xs:sequence>
                    </xs:complexType>
                </xs:element>
            </xs:schema>"#
                .to_string(),
        );
        let svc = Service::from_config(&config).unwrap();
        assert!(svc.xml_input_schema.is_some());
        assert!(svc.input_schema.is_none());
    }

    #[tokio::test]
    async fn handle_xml_schema_validation_pass() {
        let mut config = minimal_service_config();
        config.mimetype = Some("xml".to_string());
        config.input = Some(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                        xmlns:whd="https://webhookd.dev/schema">
                <xs:element name="data">
                    <xs:complexType>
                        <xs:sequence>
                            <xs:element name="id" type="xs:integer"
                                        whd:assign_value="item_id"/>
                        </xs:sequence>
                    </xs:complexType>
                </xs:element>
            </xs:schema>"#
                .to_string(),
        );
        config.output = Some("id={{ item_id }}".to_string());
        config.exec = Some("echo $WEBHOOKD_OUTPUT".to_string());
        let svc = Service::from_config(&config).unwrap();
        let headers = HeaderMap::new();

        let result = svc
            .handle_request(&headers, "<data><id>42</id></data>")
            .await
            .unwrap();
        assert_eq!(result.trim(), "id=42");
    }

    #[tokio::test]
    async fn handle_xml_schema_validation_fail() {
        let mut config = minimal_service_config();
        config.mimetype = Some("xml".to_string());
        config.input = Some(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
                <xs:element name="data">
                    <xs:complexType>
                        <xs:sequence>
                            <xs:element name="id" type="xs:integer"/>
                        </xs:sequence>
                    </xs:complexType>
                </xs:element>
            </xs:schema>"#
                .to_string(),
        );
        let svc = Service::from_config(&config).unwrap();
        let headers = HeaderMap::new();

        let err = svc
            .handle_request(&headers, "<data><id>not-integer</id></data>")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::XmlSchemaError(_)));
    }

    #[tokio::test]
    async fn full_pipeline_xml_schema_to_template_to_exec() {
        let config = ServiceConfig {
            endpoint: "/webhook/xml-test".to_string(),
            mimetype: Some("xml".to_string()),
            input: Some(
                r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                            xmlns:whd="https://webhookd.dev/schema">
                    <xs:element name="order">
                        <xs:complexType>
                            <xs:sequence>
                                <xs:element name="orderId" type="xs:integer"
                                            whd:assign_value="oid"/>
                                <xs:element name="customer" type="xs:string"
                                            whd:assign_value="cust"/>
                            </xs:sequence>
                        </xs:complexType>
                    </xs:element>
                </xs:schema>"#
                    .to_string(),
            ),
            output: Some("Order {{ oid }} by {{ cust }}".to_string()),
            input_file: None,
            output_file: None,
            exec: Some("echo $WEBHOOKD_OUTPUT".to_string()),
            proxy: None,
            authenticate: vec![AuthenticateConfig {
                json: None,
                xml: None,
                http_header: Some("X-Auth".to_string()),
                secret: Some("xml-secret".to_string()),
                secret_env_var: None,
            }],
        };

        let svc = Service::from_config(&config).unwrap();

        let mut headers = HeaderMap::new();
        headers.insert("X-Auth", "xml-secret".parse().unwrap());

        let body = "<order><orderId>99</orderId><customer>Alice</customer></order>";
        let result = svc.handle_request(&headers, body).await.unwrap();
        assert_eq!(result.trim(), "Order 99 by Alice");
    }

    #[test]
    fn xml_schema_error_status_code() {
        assert_eq!(
            ServiceError::XmlSchemaError(crate::xml_schema::XmlSchemaError::ValidationFailed(
                "x".into()
            ))
            .status_code(),
            StatusCode::BAD_REQUEST
        );
    }
}
