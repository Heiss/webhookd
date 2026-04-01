//! webhookd – core library
//!
//! Exposes the router, configuration, and handler types used by the webhookd
//! binary.  The server is driven by a TOML configuration file that defines
//! webhook services.  Each service can validate incoming payloads against a
//! JSON Schema, extract variables, render output templates, authenticate
//! requests, and execute shell scripts.
//!
//! # Architecture
//!
//! ```text
//! Config (TOML) ──▶ WebhookdConfig ──▶ App ──▶ Axum Router
//!                                        │
//!               ┌────────────────────────┘
//!               │
//!               ▼
//!         Service (per endpoint)
//!           │
//!           ├─ authenticate  (auth.rs)
//!           ├─ validate + extract  (schema.rs)
//!           ├─ render template  (template.rs)
//!           └─ exec / proxy  (service.rs)
//! ```

pub mod auth;
pub mod config;
pub mod schema;
pub mod service;
pub mod template;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use config::WebhookdConfig;
use service::Service;
use std::sync::Arc;

// ─── App ─────────────────────────────────────────────────────────────────────

/// Shared application state used by Axum handlers.
#[derive(Clone)]
pub struct App {
    /// Compiled services indexed by endpoint path.
    services: Arc<Vec<Service>>,
}

impl App {
    /// Create a new [`App`] from a validated [`WebhookdConfig`].
    ///
    /// Compiles all service definitions (schemas, templates) at startup so
    /// that request handling is fast.
    pub fn from_config(config: &WebhookdConfig) -> Result<Self, service::ServiceError> {
        let mut services = Vec::new();
        for svc_config in &config.services {
            services.push(Service::from_config(svc_config)?);
        }
        Ok(Self {
            services: Arc::new(services),
        })
    }

    /// Build the Axum [`Router`] for this application.
    ///
    /// Registers a catch-all route that dispatches to the appropriate service
    /// based on the request path.
    pub fn router(self) -> Router {
        Router::new().fallback(any(handle_request)).with_state(self)
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// Main request handler.  Matches the request path against configured service
/// endpoints and delegates to the matched service.
async fn handle_request(State(app): State<App>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_string();
    let headers = request.headers().clone();

    // Read request body
    let body_bytes = match axum::body::to_bytes(request.into_body(), 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("failed to read body: {e}")).into_response();
        }
    };
    let body = String::from_utf8_lossy(&body_bytes).into_owned();

    // Find matching service
    let service = app.services.iter().find(|s| s.endpoint == path);

    match service {
        Some(svc) => match svc.handle_request(&headers, &body).await {
            Ok(output) => (StatusCode::OK, output).into_response(),
            Err(e) => (e.status_code(), format!("error: {e}")).into_response(),
        },
        None => (
            StatusCode::NOT_FOUND,
            format!("no service registered for path '{path}'"),
        )
            .into_response(),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> WebhookdConfig {
        WebhookdConfig::parse(
            r#"
            port = 8080
            [[services]]
            endpoint = "/webhook/ping"
            exec = "echo pong"
        "#,
        )
        .unwrap()
    }

    #[test]
    fn app_from_config_creates_services() {
        let config = test_config();
        let app = App::from_config(&config).unwrap();
        assert_eq!(app.services.len(), 1);
    }

    #[test]
    fn app_from_config_with_schema() {
        let config = WebhookdConfig::parse(
            r#"
            port = 8080
            [[services]]
            endpoint = "/webhook/test"
            exec = "echo ok"
            input = '{"type": "object", "properties": {"id": {"type": "integer", "assign_value": "my_id"}}}'
            output = "{{ my_id }}"
        "#,
        )
        .unwrap();
        let app = App::from_config(&config).unwrap();
        assert_eq!(app.services.len(), 1);
    }

    #[test]
    fn app_router_builds_without_panic() {
        let config = test_config();
        let app = App::from_config(&config).unwrap();
        let _router = app.router();
    }
}
