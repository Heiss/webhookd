//! webhookd – core library
//!
//! Exposes the router and handler types used by the webhookd binary.
//! Callers build a [`Config`], construct an [`App`] from it, and pass the
//! resulting Axum router to a TCP listener.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use std::{collections::HashMap, sync::Arc};
use thiserror::Error;
use tokio::process::Command;

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors that can occur while running webhookd.
#[derive(Debug, Error)]
pub enum WebhookdError {
    #[error("Script not found for hook '{0}'")]
    HookNotFound(String),

    #[error("Script execution failed: {0}")]
    ScriptError(#[from] std::io::Error),
}

// ─── Config ──────────────────────────────────────────────────────────────────

/// Application configuration.
///
/// Maps hook names (URL path segments) to shell script paths.
///
/// # Example
/// ```
/// use webhookd::Config;
/// let mut cfg = Config::default();
/// cfg.hooks.insert("deploy".into(), "./scripts/deploy.sh".into());
/// ```
#[derive(Debug, Default, Clone)]
pub struct Config {
    /// Map of hook name → absolute or relative path to an executable script.
    pub hooks: HashMap<String, String>,
}

// ─── App ─────────────────────────────────────────────────────────────────────

/// Shared application state used by Axum handlers.
#[derive(Clone)]
pub struct App {
    pub config: Arc<Config>,
}

impl App {
    /// Create a new [`App`] from the given [`Config`].
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    /// Build the Axum [`Router`] for this application.
    pub fn router(self) -> Router {
        Router::new()
            .route("/hooks/:hook", any(handle_hook))
            .with_state(self)
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// Execute the script registered under `hook` and return its combined output.
async fn handle_hook(State(app): State<App>, Path(hook): Path<String>) -> Response {
    match run_hook(&app.config, &hook).await {
        Ok(output) => (StatusCode::OK, output).into_response(),
        Err(WebhookdError::HookNotFound(_)) => {
            (StatusCode::NOT_FOUND, format!("hook '{hook}' not found")).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("error: {e}")).into_response(),
    }
}

/// Run the script for the given hook name and return its stdout as a `String`.
pub async fn run_hook(config: &Config, hook: &str) -> Result<String, WebhookdError> {
    let script = config
        .hooks
        .get(hook)
        .ok_or_else(|| WebhookdError::HookNotFound(hook.to_string()))?;

    let output = Command::new("sh")
        .arg("-c")
        .arg(script)
        .output()
        .await
        .map_err(WebhookdError::ScriptError)?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if output.status.success() {
        Ok(stdout)
    } else {
        Err(WebhookdError::ScriptError(std::io::Error::other(format!(
            "script exited with {}: {stderr}",
            output.status
        ))))
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_echo(hook: &str, message: &str) -> Config {
        let mut cfg = Config::default();
        cfg.hooks
            .insert(hook.to_string(), format!("echo '{message}'"));
        cfg
    }

    #[test]
    fn config_default_has_no_hooks() {
        let cfg = Config::default();
        assert!(cfg.hooks.is_empty());
    }

    #[test]
    fn config_insert_hook() {
        let mut cfg = Config::default();
        cfg.hooks.insert("ping".into(), "./ping.sh".into());
        assert_eq!(cfg.hooks.get("ping"), Some(&"./ping.sh".to_string()));
    }

    #[tokio::test]
    async fn run_hook_returns_output() {
        let cfg = config_with_echo("ping", "pong");
        let result = run_hook(&cfg, "ping").await.unwrap();
        assert_eq!(result.trim(), "pong");
    }

    #[tokio::test]
    async fn run_hook_missing_returns_error() {
        let cfg = Config::default();
        let err = run_hook(&cfg, "missing").await.unwrap_err();
        assert!(matches!(err, WebhookdError::HookNotFound(_)));
    }

    #[tokio::test]
    async fn run_hook_failed_script_returns_error() {
        let mut cfg = Config::default();
        cfg.hooks.insert("fail".into(), "exit 1".into());
        let err = run_hook(&cfg, "fail").await.unwrap_err();
        assert!(matches!(err, WebhookdError::ScriptError(_)));
    }
}
