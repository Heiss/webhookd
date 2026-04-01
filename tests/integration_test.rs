//! Integration tests for webhookd.
//!
//! These tests spin up the Axum router in-process and send real HTTP requests
//! via `axum-test`, validating end-to-end behaviour without a live TCP port.

use axum_test::TestServer;
use webhookd::{App, Config};

fn test_app() -> TestServer {
    let mut config = Config::default();
    config.hooks.insert("ping".into(), "echo pong".into());
    config
        .hooks
        .insert("multi".into(), "printf 'line1\\nline2'".into());

    let router = App::new(config).router();
    TestServer::new(router).expect("failed to create test server")
}

#[tokio::test]
async fn get_known_hook_returns_200() {
    let server = test_app();
    let response = server.get("/hooks/ping").await;
    response.assert_status_ok();
    assert!(response.text().contains("pong"));
}

#[tokio::test]
async fn get_unknown_hook_returns_404() {
    let server = test_app();
    let response = server.get("/hooks/unknown").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn post_known_hook_returns_200() {
    let server = test_app();
    let response = server.post("/hooks/ping").await;
    response.assert_status_ok();
}

#[tokio::test]
async fn multiline_output_is_returned() {
    let server = test_app();
    let response = server.get("/hooks/multi").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("line1"));
    assert!(body.contains("line2"));
}
