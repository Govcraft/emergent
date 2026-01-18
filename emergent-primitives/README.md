# Emergent Primitives

Official marketplace primitives for the [Emergent](https://github.com/govcraft/emergent) workflow engine.

## Available Primitives

| Name | Kind | Description | Status |
|------|------|-------------|--------|
| `http-source` | source | Generic HTTP webhook receiver | ✅ Ready |
| `http-sink` | sink | Make outbound HTTP requests | ✅ Ready |
| `exec-source` | source | Execute shell commands | ✅ Ready |
| `slack-source` | source | Monitor Slack channels | 🚧 Stub |
| `slack-sink` | sink | Post to Slack channels | 🚧 Stub |
| `github-source` | source | GitHub webhook receiver | 🚧 Stub |
| `github-sink` | sink | GitHub API interactions | 🚧 Stub |

## Installation

Install via the Emergent marketplace CLI:

```bash
emergent marketplace install http-source
emergent marketplace install exec-source
```

Or download binaries directly from [GitHub Releases](https://github.com/govcraft/emergent-primitives/releases).

## Usage

### http-source

Receive HTTP webhooks and emit `http.request` events.

```bash
http-source --port 8080 --path /webhook
```

**Arguments:**
- `--port`, `-p`: Port to listen on (default: 8080)
- `--host`, `-H`: Host to bind (default: 0.0.0.0)
- `--path`: URL path (default: /)
- `--secret`, `-s`: HMAC-SHA256 secret for signature validation (env: `HTTP_WEBHOOK_SECRET`)

**Publishes:** `http.request`

### http-sink

Make outbound HTTP requests from consumed events.

```bash
http-sink --base-url https://api.example.com --timeout 30
```

**Arguments:**
- `--base-url`, `-u`: Base URL for requests (env: `HTTP_BASE_URL`)
- `--timeout`, `-t`: Request timeout in seconds (default: 30)
- `--retries`, `-r`: Retry attempts (default: 3)
- `--auth-header`: Authorization header (env: `HTTP_AUTH_HEADER`)

**Message Payload:**
```json
{
  "url": "/endpoint",
  "method": "POST",
  "headers": { "Content-Type": "application/json" },
  "body": { "data": "value" }
}
```

### exec-source

Execute shell commands and emit output events.

```bash
exec-source --command date --interval 5000
```

**Arguments:**
- `--command`, `-c`: Command to execute (required)
- `--args`, `-a`: Command arguments
- `--interval`, `-i`: Repeat interval in milliseconds
- `--working-dir`, `-w`: Working directory
- `--shell`, `-s`: Shell to use (default: sh)

**Publishes:** `exec.output`, `exec.error`, `exec.exit`

## Development

### Prerequisites

- Rust 2024 edition
- emergent-client SDK

### Building

```bash
cargo build --release
```

### Testing

```bash
cargo nextest run
```

### Linting

```bash
cargo clippy --all-targets -- -D warnings
```

## Release

Releases are automated via GitHub Actions. To create a new release:

1. Tag the commit: `git tag v0.1.0`
2. Push the tag: `git push origin v0.1.0`

The workflow will:
- Build for Linux (x86_64, aarch64), macOS (x86_64, aarch64), Windows (x86_64)
- Create archives (tar.gz for Unix, zip for Windows)
- Generate SHA256 checksums
- Upload to GitHub Releases

## License

MIT
