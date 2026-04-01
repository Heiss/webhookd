# Configuration

`webhookd` is configured through a TOML file.  A fully-documented example is
provided in [`config.example.toml`](config.example.toml).

## Quick Start

Create a file called `config.toml`:

```toml
port = 8080

[[services]]
endpoint = "/webhook/deploy"
exec = "/opt/webhookd/scripts/deploy.sh"
```

Run the server:

```sh
webhookd --config config.toml
```

## Reference

### Top-Level Keys

| Key          | Required | Default   | Description |
|--------------|----------|-----------|-------------|
| `port`       | **yes**  | —         | TCP port for the HTTP server. Can also be set via `-p` / `--port`. |
| `hot-reload` | no       | `false`   | Restart the server when the config file changes. Can also be set via `-R` / `--hot-reload`. |
| `log`        | no       | `"info"`  | Log level. One of `debug`, `info`, `warn`, `error`. |

### `[[services]]`

Each `[[services]]` block defines one webhook endpoint.  At least one service
must be configured.

| Key           | Required | Default | Description |
|---------------|----------|---------|-------------|
| `endpoint`    | **yes**  | —       | URL path, e.g. `"/webhook/api1"`. |
| `mimetype`    | no       | —       | Forces MIME validation (`"json"` or `"xml"`). See note below. |
| `input`       | no       | —       | Inline input schema (JSON Schema for JSON, XSD for XML). |
| `output`      | no       | —       | Inline output template (Jinja2-style). |
| `input-file`  | no       | —       | Path to a file with the input schema. Overrides `input`. |
| `output-file` | no       | —       | Path to a file with the output template. Overrides `output`. |
| `exec`        | one of ¹ | —       | Script path to execute. |
| `proxy`       | one of ¹ | —       | URL to proxy requests to. |

> ¹ Exactly one of `exec` or `proxy` must be set per service.

#### MIME Type Note

When an `authenticate` block uses `json` or `xml` as its lookup method, the
MIME type is inferred automatically.  Setting `mimetype` to a *different* value
than what `authenticate` implies is a configuration error.  Only set `mimetype`
when no `authenticate` block is used.

### `[[services.authenticate]]`

Optional.  When present, requests must carry a secret.

| Key              | Required | Default | Description |
|------------------|----------|---------|-------------|
| `json`           | at most one ² | —  | jq-style path to the secret in a JSON body. |
| `xml`            | at most one ² | —  | XPath to the secret in an XML body. |
| `http-header`    | at most one ² | `"X-Webhook-Secret"` | HTTP header carrying the secret. |
| `secret`         | one of ³ | —       | Static secret string. |
| `secret-env-var` | one of ³ | —       | Env-var name holding the secret (takes precedence). |

> ² At most one lookup method (`json`, `xml`, `http-header`) may be specified.
> If none is set, `http-header` with the default value `"X-Webhook-Secret"` is
> used.
>
> ³ At least one of `secret` or `secret-env-var` must be provided.  If both are
> set, `secret-env-var` takes precedence at runtime.
