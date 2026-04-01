# webhookd

> Webhookify your scripts and HTTP API services

`webhookd` is a lightweight HTTP daemon written in Rust. It maps incoming
webhook requests to local shell scripts and streams their output back as the
HTTP response body.

```
POST /hooks/deploy  →  runs ./scripts/deploy.sh  →  returns stdout
```

---

## Table of Contents

- [Prerequisites](#prerequisites)
- [Installing Dependencies](#installing-dependencies)
- [Project Structure](#project-structure)
- [Building](#building)
- [Running](#running)
- [Testing](#testing)
- [Linting & Formatting](#linting--formatting)
- [All Available `cargo-make` Tasks](#all-available-cargo-make-tasks)
- [Configuration](#configuration)
- [Documentation](#documentation)

---

## Prerequisites

| Tool | Minimum Version | Notes |
|------|----------------|-------|
| [Rust & Cargo](https://www.rust-lang.org/tools/install) | 1.70 | Installed via `rustup` |
| [cargo-make](https://github.com/sagiegurari/cargo-make) | 0.37 | Task runner |

---

## Installing Dependencies

### 1. Install Rust (via rustup)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

Verify:

```bash
rustc --version
cargo --version
```

### 2. Install `cargo-make`

```bash
cargo install cargo-make
```

Verify:

```bash
cargo make --version
```

### 3. Install project dependencies

Cargo resolves and downloads all crate dependencies automatically on the first
build.  You can pre-fetch them without building with:

```bash
cargo fetch
```

---

## Project Structure

```
webhookd/
├── Cargo.toml              # Package manifest & dependency declarations
├── Makefile.toml           # cargo-make task definitions
├── README.md
├── docs/
│   └── architecture.md     # Architecture overview & module map
├── src/
│   ├── lib.rs              # Core library: Config, App, run_hook, error types
│   └── main.rs             # Binary entry point – server setup & config wiring
└── tests/
    └── integration_test.rs # End-to-end HTTP tests (axum-test)
```

---

## Building

**Debug build** (fast compile, includes debug symbols):

```bash
cargo make build
# or directly:
cargo build
```

**Release build** (optimised, recommended for production):

```bash
cargo make build-release
# or directly:
cargo build --release
```

The compiled binary is placed at:

- Debug: `target/debug/webhookd`
- Release: `target/release/webhookd`

---

## Running

```bash
cargo make run
# or directly:
cargo run
```

The server starts on `127.0.0.1:8080` by default.  Test it:

```bash
curl http://127.0.0.1:8080/hooks/ping
# → pong
```

Control log verbosity with the `RUST_LOG` environment variable:

```bash
RUST_LOG=debug cargo make run
```

---

## Testing

**Run all unit and integration tests:**

```bash
cargo make test
# or directly:
cargo test
```

**Run tests with printed output** (useful for debugging):

```bash
cargo make test-verbose
# or directly:
cargo test -- --nocapture
```

---

## Linting & Formatting

**Check formatting** (does not modify files – suitable for CI):

```bash
cargo make fmt-check
```

**Auto-format all source files:**

```bash
cargo make fmt
```

**Run Clippy** (static analysis, warnings treated as errors):

```bash
cargo make lint
```

**Auto-apply Clippy fixes:**

```bash
cargo make lint-fix
```

---

## All Available `cargo-make` Tasks

Run any task with `cargo make <task>`.

| Task | Description |
|------|-------------|
| `default` | Full CI pipeline: fmt-check → lint → build → test |
| `build` | Compile in debug mode |
| `build-release` | Compile in release mode |
| `test` | Run all unit and integration tests |
| `test-verbose` | Run tests with `--nocapture` |
| `lint` | Run Clippy (deny warnings) |
| `lint-fix` | Run Clippy with auto-fix |
| `fmt` | Format source code |
| `fmt-check` | Check formatting (CI-friendly) |
| `docs` | Build & open rustdoc |
| `docs-build` | Build rustdoc without opening |
| `run` | Run the server (debug) |
| `run-release` | Run the server (release) |
| `clean` | Remove build artefacts |
| `ci` | Full CI pipeline (same as `default`) |

---

## Configuration

Hooks are configured by inserting entries into `Config.hooks` in `src/main.rs`:

```rust
config.hooks.insert("deploy".into(), "./scripts/deploy.sh".into());
config.hooks.insert("notify".into(), "python3 scripts/notify.py".into());
```

Each key is the URL path segment (`/hooks/<key>`) and each value is a shell
command executed via `sh -c`.

---

## Documentation

Build and open the inline API documentation:

```bash
cargo make docs
```

For a high-level architecture overview see [`docs/architecture.md`](docs/architecture.md).

---

## License

MIT

