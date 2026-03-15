# Local Deployment

This guide covers deploying the Emergent engine as a systemd user service on Linux.

## Installation

### Arch Linux (AUR) (Recommended)

```bash
yay -S emergent-bin
```

This installs the engine to `/usr/bin/emergent` and updates automatically with `yay -Syu`.

### Cargo

```bash
cargo install emergent-engine
```

### Download a Pre-built Binary

Download the latest release from [GitHub Releases](https://github.com/Govcraft/emergent/releases/latest) for your platform:

```bash
# Linux (x86_64)
curl -LO https://github.com/Govcraft/emergent/releases/latest/download/emergent-x86_64-unknown-linux-gnu.tar.gz
tar xzf emergent-x86_64-unknown-linux-gnu.tar.gz

# Linux (aarch64 / ARM64)
curl -LO https://github.com/Govcraft/emergent/releases/latest/download/emergent-aarch64-unknown-linux-gnu.tar.gz
tar xzf emergent-aarch64-unknown-linux-gnu.tar.gz

# macOS (Apple Silicon)
curl -LO https://github.com/Govcraft/emergent/releases/latest/download/emergent-aarch64-apple-darwin.tar.gz
tar xzf emergent-aarch64-apple-darwin.tar.gz

# macOS (Intel)
curl -LO https://github.com/Govcraft/emergent/releases/latest/download/emergent-x86_64-apple-darwin.tar.gz
tar xzf emergent-x86_64-apple-darwin.tar.gz
```

Install to your user bin directory:

```bash
mkdir -p ~/.local/bin
mv emergent ~/.local/bin/
chmod +x ~/.local/bin/emergent
```

### Build from Source (Alternative)

If you need a custom build or want to contribute:

```bash
cargo build --release -p emergent-engine
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

Create your configuration file. If you cloned the repo, you can copy the example config. Otherwise, create one from scratch (see the [Configuration Reference](configuration.md)):

```bash
# If you cloned the repo:
cp ./config/emergent.toml ~/.config/emergent/emergent.toml

# Otherwise, create a new file:
touch ~/.config/emergent/emergent.toml
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
Documentation=https://github.com/Govcraft/emergent
After=network.target

[Service]
Type=simple
ExecStart=/usr/bin/emergent --config %h/.config/emergent/emergent.toml
Restart=on-failure
RestartSec=5

# Environment
Environment=RUST_LOG=info,acton_reactive=off
Environment=PATH=/usr/local/bin:/usr/bin:/bin:%h/.local/bin:%h/.cargo/bin

# Hardening
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=default.target
```

If you installed via manual download to `~/.local/bin/`, change `ExecStart` to `%h/.local/bin/emergent`.

### Secrets with systemd-creds

If your pipeline uses secrets (API tokens, credentials), encrypt them with `systemd-creds` and load them via `LoadCredentialEncrypted`. This encrypts secrets to your machine's TPM or host key -- they cannot be decrypted on another machine.

**Step 1: Encrypt each secret:**

```bash
mkdir -p ~/.config/emergent/secrets
echo -n "xoxb-your-token" | systemd-creds encrypt --user --name=SLACK_BOT_TOKEN - ~/.config/emergent/secrets/slack-bot-token.cred
echo -n "xapp-your-token" | systemd-creds encrypt --user --name=SLACK_APP_TOKEN - ~/.config/emergent/secrets/slack-app-token.cred
```

**Step 2: Add `LoadCredentialEncrypted` to the service unit:**

```ini
[Service]
LoadCredentialEncrypted=SLACK_BOT_TOKEN:%h/.config/emergent/secrets/slack-bot-token.cred
LoadCredentialEncrypted=SLACK_APP_TOKEN:%h/.config/emergent/secrets/slack-app-token.cred
```

**Step 3: Read credentials from `$CREDENTIALS_DIRECTORY` in your TOML config.**

The engine forwards `$CREDENTIALS_DIRECTORY` to all child primitives. Use `$(cat $CREDENTIALS_DIRECTORY/...)` in `sh -c` commands instead of environment variables:

```toml
args = [
    "--shell", "sh",
    "--command", "curl -s -X POST https://slack.com/api/apps.connections.open -H \"Authorization: Bearer $(cat $CREDENTIALS_DIRECTORY/SLACK_APP_TOKEN)\" | jq -c '{url: .url}'"
]
```

Secrets never appear in environment variables, shell history, or plaintext on disk. The decrypted values exist only as files in a tmpfs mount scoped to the service lifetime.

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

### AUR (Recommended)

```bash
yay -Syu
# The service restarts automatically if the config watcher is enabled,
# or restart manually:
systemctl --user restart emergent
```

### Cargo

```bash
cargo install emergent-engine
systemctl --user restart emergent
```

### From GitHub Releases

```bash
curl -LO https://github.com/Govcraft/emergent/releases/latest/download/emergent-x86_64-unknown-linux-gnu.tar.gz
tar xzf emergent-x86_64-unknown-linux-gnu.tar.gz

systemctl --user stop emergent
mv emergent ~/.local/bin/emergent
chmod +x ~/.local/bin/emergent
systemctl --user start emergent
```

### From Source

```bash
cargo build --release -p emergent-engine

systemctl --user stop emergent
cp ./target/release/emergent ~/.local/bin/emergent
systemctl --user start emergent
```

## File Locations

| Purpose | Path |
|---------|------|
| Binary (AUR) | `/usr/bin/emergent` |
| Binary (manual) | `~/.local/bin/emergent` |
| Configuration | `~/.config/emergent/emergent.toml` |
| Encrypted secrets | `~/.config/emergent/secrets/` |
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
