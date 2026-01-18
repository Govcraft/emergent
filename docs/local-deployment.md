# Local Deployment

This guide covers deploying the Emergent engine as a systemd user service on Linux.

## Installation

### Build and Install the Binary

```bash
# Build the release binary
cargo build --release -p emergent-engine

# Install to user bin directory
mkdir -p ~/.local/bin
cp ./target/release/emergent ~/.local/bin/
chmod +x ~/.local/bin/emergent
```

### Create Directory Structure

```bash
# Config directory (XDG_CONFIG_HOME)
mkdir -p ~/.config/emergent

# Data directory (XDG_DATA_HOME)
mkdir -p ~/.local/share/emergent/logs
```

### Install Configuration

Copy or create your configuration file:

```bash
cp ./config/emergent.toml ~/.config/emergent/emergent.toml
```

Update paths in the config to use absolute paths:

```toml
[event_store]
json_log_dir = "/home/<user>/.local/share/emergent/logs"
sqlite_path = "/home/<user>/.local/share/emergent/events.db"
```

## Systemd Service Setup

### Create the Service Unit

Create `~/.config/systemd/user/emergent.service`:

```ini
[Unit]
Description=Emergent Event-Driven Workflow Engine
Documentation=https://github.com/emergent
After=network.target

[Service]
Type=simple
ExecStart=%h/.local/bin/emergent --config %h/.config/emergent/emergent.toml
Restart=on-failure
RestartSec=5

# Environment
Environment=RUST_LOG=info

# Hardening
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=default.target
```

### Create the Config Watcher (Optional)

To automatically restart the engine when the config file changes, create two additional units.

`~/.config/systemd/user/emergent-config-watcher.path`:

```ini
[Unit]
Description=Watch Emergent config directory for changes

[Path]
PathChanged=%h/.config/emergent/emergent.toml
PathModified=%h/.config/emergent/emergent.toml

[Install]
WantedBy=default.target
```

`~/.config/systemd/user/emergent-config-watcher.service`:

```ini
[Unit]
Description=Restart Emergent on config change

[Service]
Type=oneshot
ExecStart=/usr/bin/systemctl --user restart emergent.service
```

### Enable and Start

```bash
# Reload systemd to pick up new units
systemctl --user daemon-reload

# Enable services to start on login
systemctl --user enable emergent.service
systemctl --user enable emergent-config-watcher.path

# Start services
systemctl --user start emergent.service
systemctl --user start emergent-config-watcher.path
```

## Administration Commands

### Service Control

```bash
# Start the engine
systemctl --user start emergent

# Stop the engine
systemctl --user stop emergent

# Restart the engine
systemctl --user restart emergent

# Check service status
systemctl --user status emergent
```

### Monitoring

```bash
# Follow logs in real-time
journalctl --user -u emergent -f

# View recent logs
journalctl --user -u emergent -n 50

# View logs since boot
journalctl --user -u emergent -b

# View logs from a specific time
journalctl --user -u emergent --since "1 hour ago"
```

### Config Watcher

```bash
# Check watcher status
systemctl --user status emergent-config-watcher.path

# Manually trigger a restart (instead of editing config)
systemctl --user restart emergent
```

## Updating the Engine

To install a new version:

```bash
# Build the new release
cargo build --release -p emergent-engine

# Stop, install, start
systemctl --user stop emergent
cp ./target/release/emergent ~/.local/bin/emergent
systemctl --user start emergent
```

## File Locations

| Purpose | Path |
|---------|------|
| Binary | `~/.local/bin/emergent` |
| Configuration | `~/.config/emergent/emergent.toml` |
| Event logs (JSON) | `~/.local/share/emergent/logs/` |
| Event database | `~/.local/share/emergent/events.db` |
| IPC socket | `/run/user/<uid>/emergent.sock` |
| Service unit | `~/.config/systemd/user/emergent.service` |
| Path watcher | `~/.config/systemd/user/emergent-config-watcher.path` |

## Troubleshooting

### Service fails to start

Check the logs for errors:

```bash
journalctl --user -u emergent -n 100 --no-pager
```

Common issues:
- Config file has relative paths (use absolute paths)
- Missing directories (create `~/.local/share/emergent/logs`)
- Binary not executable (`chmod +x ~/.local/bin/emergent`)

### Config watcher not triggering

Verify the path unit is active:

```bash
systemctl --user status emergent-config-watcher.path
```

The watcher triggers on file modification. If editing with some editors that use atomic saves (write to temp, then rename), you may need to touch the file:

```bash
touch ~/.config/emergent/emergent.toml
```

### Socket permission issues

The IPC socket is created in `$XDG_RUNTIME_DIR` (typically `/run/user/<uid>/`). Ensure this directory exists and is writable by your user.
