//! webhookd – binary entry point
//!
//! Loads a TOML configuration file, compiles all service definitions, and
//! starts the HTTP server.

use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::TcpListener;
use tracing_subscriber::{fmt, EnvFilter};
use webhookd::config::WebhookdConfig;
use webhookd::App;

#[tokio::main]
async fn main() {
    // Initialise structured logging; use RUST_LOG to control verbosity.
    fmt().with_env_filter(EnvFilter::from_default_env()).init();

    // Determine config file path from args or default
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.toml"));

    // Load and validate configuration
    let config = WebhookdConfig::from_file(&config_path).unwrap_or_else(|e| {
        eprintln!(
            "error: failed to load config from {}: {e}",
            config_path.display()
        );
        std::process::exit(1);
    });

    // Set log level from config
    let log_level = config.log.to_string();
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", &log_level);
    }

    tracing::info!(
        "loaded {} service(s) from {}",
        config.services.len(),
        config_path.display()
    );

    let port = config.port;

    // Build application
    let app = App::from_config(&config).unwrap_or_else(|e| {
        eprintln!("error: failed to build application: {e}");
        std::process::exit(1);
    });

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("webhookd listening on {addr}");

    let listener = TcpListener::bind(addr).await.expect("bind failed");
    axum::serve(listener, app.router())
        .await
        .expect("server error");
}
