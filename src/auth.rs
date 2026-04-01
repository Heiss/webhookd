//! Authentication helpers for webhook services.
//!
//! Each service may define one or more `[[services.authenticate]]` blocks.
//! Authentication verifies that the incoming request carries a secret that
//! matches the configured value.
//!
//! # Lookup methods
//!
//! The secret can be looked up from:
//! - An HTTP header (default: `X-Webhook-Secret`)
//! - A JSON body path (jq-style dotted path)
//!
//! # Secret sources
//!
//! The expected secret can come from:
//! - A static `secret` string in the config
//! - An environment variable via `secret-env-var`
//!
//! If both are set, `secret-env-var` takes precedence.

use crate::config::AuthenticateConfig;
use axum::http::HeaderMap;
use serde_json::Value;
use thiserror::Error;

/// Errors produced during authentication.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("authentication failed: {0}")]
    Unauthorized(String),
}

/// Check authentication for a request against the configured auth block.
///
/// Returns `Ok(())` if authentication passes, or an `AuthError` if it fails.
pub fn check_auth(
    auth: &AuthenticateConfig,
    headers: &HeaderMap,
    body: Option<&Value>,
) -> Result<(), AuthError> {
    // Resolve the expected secret
    let expected_secret = resolve_secret(auth)?;

    // Determine where to look for the client-supplied secret
    let provided_secret = if let Some(ref json_path) = auth.json {
        // Look up secret from JSON body
        let body = body.ok_or_else(|| {
            AuthError::Unauthorized("JSON auth lookup requires a JSON body".into())
        })?;
        extract_json_path(body, json_path).ok_or_else(|| {
            AuthError::Unauthorized(format!("secret not found at JSON path '{json_path}'"))
        })?
    } else {
        // Look up secret from HTTP header
        let header_name = auth.http_header.as_deref().unwrap_or("X-Webhook-Secret");
        headers
            .get(header_name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| AuthError::Unauthorized(format!("missing header '{header_name}'")))?
    };

    if provided_secret == expected_secret {
        Ok(())
    } else {
        Err(AuthError::Unauthorized("secret mismatch".into()))
    }
}

/// Resolve the expected secret from config (static or env var).
fn resolve_secret(auth: &AuthenticateConfig) -> Result<String, AuthError> {
    // secret-env-var takes precedence
    if let Some(ref env_var) = auth.secret_env_var {
        return std::env::var(env_var)
            .map_err(|_| AuthError::Unauthorized(format!("env var '{env_var}' not set")));
    }

    if let Some(ref secret) = auth.secret {
        return Ok(secret.clone());
    }

    Err(AuthError::Unauthorized(
        "no secret or secret-env-var configured".into(),
    ))
}

/// Extract a value from a JSON body using a simple dotted path.
///
/// Supports paths like `"foo.bar.baz"` and simple array indices like
/// `"foo[0].bar"`.
fn extract_json_path(body: &Value, path: &str) -> Option<String> {
    let mut current = body;

    for segment in path.split('.') {
        // Handle array index: segment like "items[0]"
        if let Some(bracket_pos) = segment.find('[') {
            let key = &segment[..bracket_pos];
            let idx_str = &segment[bracket_pos + 1..segment.len() - 1];

            if !key.is_empty() {
                current = current.get(key)?;
            }

            let idx: usize = idx_str.parse().ok()?;
            current = current.get(idx)?;
        } else {
            current = current.get(segment)?;
        }
    }

    // Convert to string regardless of type
    match current {
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;

    fn auth_with_secret(secret: &str) -> AuthenticateConfig {
        AuthenticateConfig {
            json: None,
            xml: None,
            http_header: None,
            secret: Some(secret.to_string()),
            secret_env_var: None,
        }
    }

    fn auth_with_header(header: &str, secret: &str) -> AuthenticateConfig {
        AuthenticateConfig {
            json: None,
            xml: None,
            http_header: Some(header.to_string()),
            secret: Some(secret.to_string()),
            secret_env_var: None,
        }
    }

    fn auth_with_json_path(path: &str, secret: &str) -> AuthenticateConfig {
        AuthenticateConfig {
            json: Some(path.to_string()),
            xml: None,
            http_header: None,
            secret: Some(secret.to_string()),
            secret_env_var: None,
        }
    }

    // ── header-based auth ───────────────────────────────────────────────

    #[test]
    fn auth_default_header_success() {
        let auth = auth_with_secret("my-secret");
        let mut headers = HeaderMap::new();
        headers.insert("X-Webhook-Secret", HeaderValue::from_static("my-secret"));

        assert!(check_auth(&auth, &headers, None).is_ok());
    }

    #[test]
    fn auth_default_header_wrong_secret() {
        let auth = auth_with_secret("my-secret");
        let mut headers = HeaderMap::new();
        headers.insert("X-Webhook-Secret", HeaderValue::from_static("wrong"));

        assert!(check_auth(&auth, &headers, None).is_err());
    }

    #[test]
    fn auth_default_header_missing() {
        let auth = auth_with_secret("my-secret");
        let headers = HeaderMap::new();

        assert!(check_auth(&auth, &headers, None).is_err());
    }

    #[test]
    fn auth_custom_header_success() {
        let auth = auth_with_header("X-My-Token", "token123");
        let mut headers = HeaderMap::new();
        headers.insert("X-My-Token", HeaderValue::from_static("token123"));

        assert!(check_auth(&auth, &headers, None).is_ok());
    }

    #[test]
    fn auth_custom_header_wrong() {
        let auth = auth_with_header("X-My-Token", "token123");
        let mut headers = HeaderMap::new();
        headers.insert("X-My-Token", HeaderValue::from_static("bad"));

        assert!(check_auth(&auth, &headers, None).is_err());
    }

    // ── JSON path auth ──────────────────────────────────────────────────

    #[test]
    fn auth_json_path_success() {
        let auth = auth_with_json_path("auth.secret", "s3cret");
        let headers = HeaderMap::new();
        let body = json!({"auth": {"secret": "s3cret"}});

        assert!(check_auth(&auth, &headers, Some(&body)).is_ok());
    }

    #[test]
    fn auth_json_path_wrong_value() {
        let auth = auth_with_json_path("auth.secret", "s3cret");
        let headers = HeaderMap::new();
        let body = json!({"auth": {"secret": "wrong"}});

        assert!(check_auth(&auth, &headers, Some(&body)).is_err());
    }

    #[test]
    fn auth_json_path_missing() {
        let auth = auth_with_json_path("auth.secret", "s3cret");
        let headers = HeaderMap::new();
        let body = json!({"auth": {}});

        assert!(check_auth(&auth, &headers, Some(&body)).is_err());
    }

    #[test]
    fn auth_json_no_body_is_error() {
        let auth = auth_with_json_path("auth.secret", "s3cret");
        let headers = HeaderMap::new();

        assert!(check_auth(&auth, &headers, None).is_err());
    }

    // ── env var secret ──────────────────────────────────────────────────

    #[test]
    fn auth_env_var_secret() {
        let auth = AuthenticateConfig {
            json: None,
            xml: None,
            http_header: None,
            secret: Some("fallback".to_string()),
            secret_env_var: Some("WEBHOOKD_TEST_SECRET".to_string()),
        };

        // Set the env var
        std::env::set_var("WEBHOOKD_TEST_SECRET", "env-secret");

        let mut headers = HeaderMap::new();
        headers.insert("X-Webhook-Secret", HeaderValue::from_static("env-secret"));

        assert!(check_auth(&auth, &headers, None).is_ok());

        // Clean up
        std::env::remove_var("WEBHOOKD_TEST_SECRET");
    }

    #[test]
    fn auth_env_var_takes_precedence() {
        let auth = AuthenticateConfig {
            json: None,
            xml: None,
            http_header: None,
            secret: Some("static-secret".to_string()),
            secret_env_var: Some("WEBHOOKD_TEST_PREC".to_string()),
        };

        std::env::set_var("WEBHOOKD_TEST_PREC", "env-value");

        let mut headers = HeaderMap::new();
        // Using the env var value, not the static secret
        headers.insert("X-Webhook-Secret", HeaderValue::from_static("env-value"));
        assert!(check_auth(&auth, &headers, None).is_ok());

        // Static secret should NOT work
        let mut headers2 = HeaderMap::new();
        headers2.insert(
            "X-Webhook-Secret",
            HeaderValue::from_static("static-secret"),
        );
        assert!(check_auth(&auth, &headers2, None).is_err());

        std::env::remove_var("WEBHOOKD_TEST_PREC");
    }

    // ── extract_json_path ───────────────────────────────────────────────

    #[test]
    fn extract_simple_path() {
        let body = json!({"secret": "abc"});
        assert_eq!(extract_json_path(&body, "secret"), Some("abc".to_string()));
    }

    #[test]
    fn extract_nested_path() {
        let body = json!({"a": {"b": {"c": "deep"}}});
        assert_eq!(extract_json_path(&body, "a.b.c"), Some("deep".to_string()));
    }

    #[test]
    fn extract_with_array_index() {
        let body = json!({"items": [{"id": 1}, {"id": 2}]});
        assert_eq!(
            extract_json_path(&body, "items[1].id"),
            Some("2".to_string())
        );
    }

    #[test]
    fn extract_missing_path() {
        let body = json!({"a": 1});
        assert_eq!(extract_json_path(&body, "b"), None);
    }

    #[test]
    fn extract_number_as_string() {
        let body = json!({"count": 42});
        assert_eq!(extract_json_path(&body, "count"), Some("42".to_string()));
    }
}
