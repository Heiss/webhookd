# Architecture

## Overview

`webhookd` is a lightweight HTTP daemon that maps incoming webhook requests to
local shell scripts and returns their output.

```
HTTP Client
    │
    │  POST/GET /hooks/<name>
    ▼
┌──────────────────────────────┐
│        Axum HTTP Server      │
│  (src/main.rs + src/lib.rs)  │
└──────────────┬───────────────┘
               │
               │  look up <name> in Config.hooks
               ▼
┌──────────────────────────────┐
│         Config               │
│  hooks: HashMap<name, path>  │
└──────────────┬───────────────┘
               │
               │  sh -c <path>
               ▼
┌──────────────────────────────┐
│       Shell Script           │
│  (arbitrary executable)      │
└──────────────┬───────────────┘
               │
               │  stdout / exit code
               ▼
         HTTP Response
         200 OK  │  stdout as body
         500     │  script failed
         404     │  hook not registered
```

## Module Structure

| Path               | Purpose                                          |
|--------------------|--------------------------------------------------|
| `src/main.rs`      | Binary entry point – wires config, server, logs  |
| `src/lib.rs`       | Core types (`Config`, `App`), handler, `run_hook`|
| `tests/`           | Integration tests using `axum-test`              |
| `docs/`            | Project documentation                            |
| `Makefile.toml`    | `cargo-make` task definitions                    |

## Configuration

At runtime, `Config` holds a `HashMap<String, String>` of

```
hook_name  →  shell_command
```

In the current implementation the map is populated in `main.rs`.
Future work will load it from a TOML file (see open issues).

## Error Handling

All errors are modelled with `thiserror` in `WebhookdError`:

* `HookNotFound` – no entry in the map → HTTP 404  
* `ScriptError`  – I/O error or non-zero exit → HTTP 500
