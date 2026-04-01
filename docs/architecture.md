# Architecture

## Overview

`webhookd` is a lightweight HTTP daemon that maps incoming webhook requests to
local shell scripts (or reverse proxies).  Each endpoint is configured in a
TOML file and can optionally validate payloads against a JSON Schema, extract
variables, render output templates, and authenticate requests.

```
HTTP Client
    |
    |  POST/GET <endpoint>
    v
+------------------------------+
|        Axum HTTP Server      |
|  (src/main.rs + src/lib.rs)  |
+------------------------------+
               |
               |  match request path to a Service
               v
+------------------------------+
|     Service  (service.rs)    |
|                              |
|  1. Authenticate (auth.rs)   |
|  2. Validate input (schema)  |
|  3. Extract assign_value     |
|  4. Render output template   |
|  5. Execute script / proxy   |
+------------------------------+
               |
               |  stdout / exit code
               v
          HTTP Response
          200 OK    |  stdout as body
          400       |  validation failed
          401       |  authentication failed
          404       |  no service for path
          500       |  script failed
```

## Module Structure

| Path               | Purpose                                                        |
|--------------------|----------------------------------------------------------------|
| `src/main.rs`      | Binary entry point -- loads TOML config, starts HTTP server    |
| `src/lib.rs`       | App router, dispatches requests to matching services           |
| `src/config.rs`    | TOML configuration types, parsing, and validation              |
| `src/schema.rs`    | JSON Schema validation with `assign_value` extraction          |
| `src/template.rs`  | Jinja2-style output template rendering (via MiniJinja)         |
| `src/auth.rs`      | Request authentication (header / JSON body path lookup)        |
| `src/service.rs`   | Service handler -- ties auth, schema, template, exec together  |
| `tests/`           | Integration tests using `axum-test`                            |
| `docs/`            | Project documentation and example config                       |
| `Makefile.toml`    | `cargo-make` task definitions                                  |

## Request Processing Pipeline

Each service processes requests through a five-stage pipeline:

### 1. Authentication (`auth.rs`)

If `[[services.authenticate]]` is configured, the handler verifies the
request carries a valid secret.  The secret can be looked up from:
- An HTTP header (default: `X-Webhook-Secret`)
- A JSON body path (jq-style dotted path)

The expected secret can come from a static config string or an environment
variable (`secret-env-var` takes precedence).

### 2. Schema Validation (`schema.rs`)

If an `input` schema is provided, the JSON payload is validated against it
using the `jsonschema` crate.  Invalid payloads are rejected with HTTP 400.

### 3. Variable Extraction (`schema.rs`)

Properties in the JSON Schema that define `"assign_value": "var-name"` have
their values captured from the payload into a variable map.  This works at
any nesting depth.

### 4. Template Rendering (`template.rs`)

If an `output` template is configured, it is rendered with the extracted
variables using MiniJinja (Jinja2 syntax).  The rendered output is passed to
the script via the `WEBHOOKD_OUTPUT` environment variable.

### 5. Action Execution (`service.rs`)

The configured shell script is executed with:
- `WEBHOOKD_BODY` -- the raw request body
- `WEBHOOKD_OUTPUT` -- the rendered output template (if any)

The script's stdout becomes the HTTP response body.

## Configuration

Configuration is loaded from a TOML file (default: `config.toml`).
See `docs/config.example.toml` for the full reference and
`docs/configuration.md` for the configuration reference tables.

## Error Handling

All errors are modelled with `thiserror`:

| Error                     | HTTP Status | Description                          |
|---------------------------|-------------|--------------------------------------|
| `AuthError::Unauthorized` | 401         | Authentication failed                |
| `SchemaError::*`          | 400         | Schema validation or JSON parse error|
| `ServiceError::ExecFailed`| 500         | Script execution failed              |
| No matching service       | 404         | No endpoint registered for the path  |
