//! webhookd – binary entry point

use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing_subscriber::{fmt, EnvFilter};
use webhookd::{App, Config};

#[tokio::main]
async fn main() {
    // Initialise structured logging; use RUST_LOG to control verbosity.
    fmt().with_env_filter(EnvFilter::from_default_env()).init();

    // Build a demo configuration.  In a real deployment this would be read
    // from a TOML/YAML file or environment variables.
    let mut config = Config::default();
    config.hooks.insert("ping".into(), "echo pong".into());

    let app = App::new(config);

    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    tracing::info!("webhookd listening on {addr}");

    let listener = TcpListener::bind(addr).await.expect("bind failed");
    axum::serve(listener, app.router())
        .await
        .expect("server error");
}
