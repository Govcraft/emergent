# Emergent Error Handling Guide

## Clippy Rules

The Emergent workspace denies two clippy lints. These are **non-negotiable**
for code in the workspace, and the right default for a primitive of your own.

### Denied Lints

```toml
# In the emergent workspace root; member crates opt in with `[lints] workspace = true`
[workspace.lints.clippy]
unwrap_used = "deny"
expect_used = "deny"

# In a standalone primitive crate, the same thing is spelled:
# [lints.clippy]
# unwrap_used = "deny"
# expect_used = "deny"
```

These are clippy lints, so `cargo check` and `cargo build` still succeed. Only
`cargo clippy` fails, which is why it is a required gate.

## Correct Patterns

### Instead of `unwrap()` or `expect()`

#### For Option Types

```rust
// BAD - fails cargo clippy (unwrap_used / expect_used are denied)
let value = some_option.unwrap();
let value = some_option.expect("should have value");

// GOOD - handle the None case
let value = match some_option {
    Some(v) => v,
    None => return Err(MyPrimitiveError::MissingValue),
};

// GOOD - with default
let value = some_option.unwrap_or_default();
let value = some_option.unwrap_or(default_value);
let value = some_option.unwrap_or_else(|| compute_default());

// GOOD - propagate with ?
let value = some_option.ok_or(MyPrimitiveError::MissingValue)?;
let value = some_option.ok_or_else(|| MyPrimitiveError::MissingValue)?;

// GOOD - if let for optional processing
if let Some(value) = some_option {
    process(value);
}
```

#### For Result Types

```rust
// BAD - fails cargo clippy (unwrap_used / expect_used are denied)
let value = some_result.unwrap();
let value = some_result.expect("should succeed");

// GOOD - propagate with ?
let value = some_result?;

// GOOD - map the error type
let value = some_result.map_err(|e| MyPrimitiveError::from(e))?;
let value = some_result.map_err(MyPrimitiveError::External)?;

// GOOD - handle explicitly
let value = match some_result {
    Ok(v) => v,
    Err(e) => {
        eprintln!("Error: {e}");
        return Err(e.into());
    }
};

// GOOD - with default on error
let value = some_result.unwrap_or_default();
let value = some_result.unwrap_or(fallback);

// GOOD - convert to option
let maybe_value = some_result.ok();
```

### Connection Handling

```rust
// BAD
let source = EmergentSource::connect(&name).await.unwrap();

// GOOD - handle connection failure
let source = match EmergentSource::connect(&name).await {
    Ok(s) => s,
    Err(e) => {
        eprintln!("Failed to connect to Emergent engine: {e}");
        std::process::exit(1);
    }
};

// GOOD - propagate in functions returning Result
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let source = EmergentSource::connect(&name).await?;
    // ...
    Ok(())
}
```

### Payload Parsing

```rust
// BAD
let data: MyPayload = msg.payload_as().unwrap();

// GOOD - skip invalid payloads
let data: MyPayload = match msg.payload_as() {
    Ok(d) => d,
    Err(_) => continue,  // Skip this message
};

// GOOD - with logging
let data: MyPayload = match msg.payload_as() {
    Ok(d) => d,
    Err(e) => {
        eprintln!("Invalid payload: {e}");
        continue;
    }
};

// GOOD - propagate error
let data: MyPayload = msg.payload_as().map_err(|e| {
    MyPrimitiveError::InvalidPayload(e.to_string())
})?;
```

### Environment Variables

```rust
// BAD
let name = std::env::var("EMERGENT_NAME").unwrap();

// GOOD - with fallback
let name = std::env::var("EMERGENT_NAME")
    .unwrap_or_else(|_| "default_name".to_string());

// GOOD - required variable
let name = std::env::var("REQUIRED_VAR")
    .map_err(|_| MyPrimitiveError::MissingEnvVar("REQUIRED_VAR"))?;
```

### Publishing Messages

```rust
// BAD
source.publish(message).await.unwrap();

// GOOD - fire-and-forget (ignore errors)
let _ = source.publish(message).await;

// GOOD - log errors
if let Err(e) = source.publish(message).await {
    eprintln!("Failed to publish: {e}");
}

// GOOD - propagate if critical
source.publish(message).await?;
```

## Error Types

### The SDK's Own Errors

| Type | Returned by | Variants |
|---|---|---|
| `emergent_client::ClientError` | `connect`, `subscribe`, `publish`, `discover`, `disconnect` | `ConnectionFailed`, `SocketNotFound`, `IoError`, `IpcError`, `SerializationError`, `SubscriptionFailed`, `PublishFailed`, `DiscoveryFailed`, `Timeout`, `EngineError`, `ProtocolError` |
| `emergent_client::helpers::HelperError` | `run_source`, `run_handler`, `run_sink` | `ConnectionFailed { name, error }`, `UserFunction`, `PublishFailed`, `SubscribeFailed`, `SignalHandlerFailed`, `DisconnectFailed` |
| `serde_json::Error` | `msg.payload_as::<T>()` | n/a |

The closures passed to the `run_*` helpers return `Result<(), String>`, which is
why the examples end calls with `.map_err(|e| e.to_string())`. An `Err` from
the closure makes the helper return `HelperError::UserFunction`, which ends the
primitive, and the engine does not restart it unless the primitive's config sets
a `restart` policy (after 0.10.10). Return `Err` only for a failure
that should stop the process; for one bad message, log it (or publish a
`<domain>.failed` event) and return `Ok(())`.

### Creating Custom Errors

Use `thiserror` for custom error types:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MyPrimitiveError {
    #[error("failed to connect: {0}")]
    ConnectionFailed(String),

    #[error("invalid payload: {0}")]
    InvalidPayload(String),

    #[error("missing required field: {0}")]
    MissingField(&'static str),

    #[error("missing value")]
    MissingValue,

    #[error("missing environment variable: {0}")]
    MissingEnvVar(&'static str),

    #[error("fetch failed: {0}")]
    FetchFailed(String),

    #[error("external service error: {0}")]
    External(#[from] reqwest::Error),

    // Lets `?` convert SDK errors, as in `EmergentSource::connect(&name).await?`
    #[error(transparent)]
    Client(#[from] emergent_client::ClientError),
}
```

### Function Signatures

```rust
// For main functions
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ...
}

// For internal functions with specific error types
async fn process_message(msg: &EmergentMessage) -> Result<(), MyPrimitiveError> {
    // ...
}

// For simple internal functions
fn parse_config(s: &str) -> Option<Config> {
    // Returns None on parse failure
}
```

## Common Patterns

### Safe Integer Parsing

```rust
// BAD
let count: u64 = payload["count"].as_u64().unwrap();

// GOOD
let count: u64 = payload
    .get("count")
    .and_then(|v| v.as_u64())
    .unwrap_or(0);

// GOOD - with error
let count: u64 = payload
    .get("count")
    .and_then(|v| v.as_u64())
    .ok_or(MyPrimitiveError::MissingField("count"))?;
```

### Safe String Extraction

```rust
// BAD
let name: &str = payload["name"].as_str().unwrap();

// GOOD
let name: &str = payload
    .get("name")
    .and_then(|v| v.as_str())
    .unwrap_or("unknown");

// GOOD - with owned String
let name: String = payload
    .get("name")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string())
    .unwrap_or_default();
```

### Handling Multiple Fallible Operations

```rust
// Chain with ? for early return
async fn process() -> Result<(), MyPrimitiveError> {
    let source = EmergentSource::connect(&name).await?;
    let data = fetch_data().await?;
    let result = transform(data)?;
    source.publish(result).await?;
    Ok(())
}

// Or handle each error specifically
async fn process_with_context() -> Result<(), MyPrimitiveError> {
    let source = EmergentSource::connect(&name)
        .await
        .map_err(|e| MyPrimitiveError::ConnectionFailed(e.to_string()))?;

    let data = fetch_data()
        .await
        .map_err(|e| MyPrimitiveError::FetchFailed(e.to_string()))?;

    // ...
    Ok(())
}
```

## Testing Error Handling

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handles_missing_field() {
        let payload = json!({});
        let result = parse_payload(&payload);
        assert!(result.is_err());
    }

    #[test]
    fn test_handles_invalid_type() {
        let payload = json!({"count": "not a number"});
        let result = parse_payload(&payload);
        assert!(result.is_err());
    }
}
```
