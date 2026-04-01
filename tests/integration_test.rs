//! Integration tests for webhookd.
//!
//! These tests spin up the Axum router in-process and send real HTTP requests
//! via `axum-test`, validating end-to-end behaviour without a live TCP port.

use axum_test::TestServer;
use webhookd::config::WebhookdConfig;
use webhookd::App;

fn test_app() -> TestServer {
    let config = WebhookdConfig::parse(
        r#"
        port = 8080
        [[services]]
        endpoint = "/webhook/ping"
        exec = "echo pong"

        [[services]]
        endpoint = "/webhook/multi"
        exec = "printf 'line1\nline2'"
    "#,
    )
    .unwrap();

    let app = App::from_config(&config).unwrap();
    TestServer::new(app.router()).expect("failed to create test server")
}

#[tokio::test]
async fn get_known_endpoint_returns_200() {
    let server = test_app();
    let response = server.get("/webhook/ping").await;
    response.assert_status_ok();
    assert!(response.text().contains("pong"));
}

#[tokio::test]
async fn get_unknown_endpoint_returns_404() {
    let server = test_app();
    let response = server.get("/webhook/unknown").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn post_known_endpoint_returns_200() {
    let server = test_app();
    let response = server.post("/webhook/ping").await;
    response.assert_status_ok();
}

#[tokio::test]
async fn multiline_output_is_returned() {
    let server = test_app();
    let response = server.get("/webhook/multi").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("line1"));
    assert!(body.contains("line2"));
}

// ─── Schema validation tests ────────────────────────────────────────────────

fn schema_app() -> TestServer {
    let config = WebhookdConfig::parse(
        r#"
        port = 8080
        [[services]]
        endpoint = "/webhook/validated"
        exec = "echo $WEBHOOKD_OUTPUT"
        input = """
        {
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "assign_value": "item_id"
                },
                "name": {
                    "type": "string",
                    "assign_value": "item_name"
                }
            },
            "required": ["id"]
        }
        """
        output = "id={{ item_id }}, name={{ item_name }}"
    "#,
    )
    .unwrap();

    let app = App::from_config(&config).unwrap();
    TestServer::new(app.router()).expect("failed to create test server")
}

#[tokio::test]
async fn schema_validation_passes_and_template_renders() {
    let server = schema_app();
    let response = server
        .post("/webhook/validated")
        .text(r#"{"id": 42, "name": "Widget"}"#)
        .await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("id=42"), "body was: {body}");
    assert!(body.contains("name=Widget"), "body was: {body}");
}

#[tokio::test]
async fn schema_validation_fails_for_wrong_type() {
    let server = schema_app();
    let response = server
        .post("/webhook/validated")
        .text(r#"{"id": "not-a-number"}"#)
        .await;
    response.assert_status(axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn schema_validation_fails_for_missing_required() {
    let server = schema_app();
    let response = server
        .post("/webhook/validated")
        .text(r#"{"name": "Widget"}"#)
        .await;
    response.assert_status(axum::http::StatusCode::BAD_REQUEST);
}

// ─── Authentication tests ───────────────────────────────────────────────────

fn auth_app() -> TestServer {
    let config = WebhookdConfig::parse(
        r#"
        port = 8080
        [[services]]
        endpoint = "/webhook/secured"
        exec = "echo authenticated"
        [[services.authenticate]]
        http-header = "X-Secret"
        secret = "my-secret"
    "#,
    )
    .unwrap();

    let app = App::from_config(&config).unwrap();
    TestServer::new(app.router()).expect("failed to create test server")
}

#[tokio::test]
async fn auth_passes_with_correct_secret() {
    let server = auth_app();
    let response = server
        .post("/webhook/secured")
        .add_header("X-Secret".parse().unwrap(), "my-secret".parse().unwrap())
        .await;
    response.assert_status_ok();
    assert!(response.text().contains("authenticated"));
}

#[tokio::test]
async fn auth_fails_with_wrong_secret() {
    let server = auth_app();
    let response = server
        .post("/webhook/secured")
        .add_header("X-Secret".parse().unwrap(), "wrong".parse().unwrap())
        .await;
    response.assert_status(axum::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_fails_with_missing_header() {
    let server = auth_app();
    let response = server.post("/webhook/secured").await;
    response.assert_status(axum::http::StatusCode::UNAUTHORIZED);
}
