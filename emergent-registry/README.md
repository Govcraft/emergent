# Emergent Registry

Official registry of Emergent primitives for the marketplace.

## Structure

```
emergent-registry/
├── registry.toml              # Index of all primitives
└── primitives/
    └── <name>/
        └── manifest.toml      # Detailed manifest for each primitive
```

## Registry Schema

### registry.toml

```toml
[registry]
name = "emergent-official"
version = "1.0.0"
description = "Official Emergent primitives registry"

[[primitives]]
name = "example-source"
version = "0.1.0"
kind = "source"                    # source | handler | sink
description = "Brief description"
publishes = ["type.event"]         # for sources/handlers
subscribes = ["*"]                 # for handlers/sinks
tags = ["tag1", "tag2"]
```

### manifest.toml

```toml
[primitive]
name = "example-source"
version = "0.1.0"
kind = "source"
description = "Full description"
homepage = "https://github.com/..."
license = "MIT"

[messages]
publishes = ["type.event"]

[[args]]
name = "token"
long = "token"
short = "t"
env = "EXAMPLE_TOKEN"
required = true
description = "Authentication token"

[binaries]
release_url = "https://github.com/.../releases"

[binaries.checksums]
x86_64-unknown-linux-gnu = "sha256:..."

[binaries.targets]
x86_64-unknown-linux-gnu = "example-0.1.0-x86_64-unknown-linux-gnu.tar.gz"
```

## Available Primitives

| Name | Kind | Description |
|------|------|-------------|
| slack-source | source | Monitor Slack channels |
| slack-sink | sink | Post to Slack channels |
| github-source | source | GitHub webhook receiver |
| github-sink | sink | GitHub API interactions |
| http-source | source | Generic webhook receiver |
| http-sink | sink | Make HTTP requests |
| exec-source | source | Execute shell commands |

## Usage

Configure the registry URL in `~/.config/emergent/marketplace.toml`:

```toml
[registry]
url = "https://raw.githubusercontent.com/govcraft/emergent-registry/main"
```

Then use the marketplace CLI:

```bash
emergent marketplace list
emergent marketplace install http-source
emergent marketplace info slack-source
```
