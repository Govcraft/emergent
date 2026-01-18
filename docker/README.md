# Emergent Engine Docker

Minimal Docker image for the Emergent event-driven workflow engine.

## Quick Start

```bash
# Create directories and config
mkdir -p config data run
cp emergent.toml.example config/emergent.toml

# Run with docker-compose
docker-compose up
```

## Image Details

- **Base:** Alpine Linux 3.21
- **Size:** ~20MB
- **User:** Non-root `emergent` user (UID 1000)
- **Binary:** Statically linked with musl

## Volume Mounts

| Mount Point | Purpose | Mode |
|-------------|---------|------|
| `/etc/emergent` | Configuration files | Read-only |
| `/var/lib/emergent` | Events DB + JSON logs | Read-write |
| `/run/emergent` | IPC socket directory | Read-write |

## Configuration

Copy `emergent.toml.example` to `config/emergent.toml` and customize:

```toml
[engine]
name = "emergent"
socket_path = "/run/emergent/emergent.sock"
wire_format = "messagepack"

[event_store]
json_log_dir = "/var/lib/emergent/logs"
sqlite_path = "/var/lib/emergent/events.db"
retention_days = 30
```

## Running Primitives

Primitives (sources, handlers, sinks) can connect to the containerized engine via the shared socket:

### From Host

```bash
EMERGENT_SOCKET=./run/emergent.sock ./my-primitive
```

### From Another Container

```yaml
services:
  my-primitive:
    volumes:
      - ./run:/run/emergent
    environment:
      - EMERGENT_SOCKET=/run/emergent/emergent.sock
```

## Building Locally

```bash
# From repository root
docker build -t emergent:local -f docker/Dockerfile .

# Check image size
docker images emergent:local
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level (trace, debug, info, warn, error) |

## GitHub Container Registry

```bash
# Pull from GHCR (after pushing)
docker pull ghcr.io/govcraft/emergent:latest
docker pull ghcr.io/govcraft/emergent:0.4.1
```

## Healthcheck

The container includes a healthcheck that verifies the engine process is running:

```yaml
healthcheck:
  test: ["CMD", "pgrep", "-x", "emergent"]
  interval: 30s
  timeout: 10s
  retries: 3
```

## Troubleshooting

**Engine won't start:**
- Verify config file exists at `config/emergent.toml`
- Check logs: `docker-compose logs`

**Socket permission issues:**
- Ensure `run/` directory is writable
- Check UID/GID mapping if using user namespaces

**Database errors:**
- Ensure `data/` directory is writable
- Check disk space
