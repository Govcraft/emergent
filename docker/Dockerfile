# ==============================================================================
# Emergent Engine Docker Image
# Multi-stage build for minimal image size
# ==============================================================================

# ------------------------------------------------------------------------------
# Stage 1: Builder
# ------------------------------------------------------------------------------
FROM rust:alpine AS builder

# Install build dependencies and add musl target
RUN apk add --no-cache musl-dev && \
    rustup target add x86_64-unknown-linux-musl

# Set up working directory
WORKDIR /build

# Copy workspace files needed for engine build
COPY Cargo.toml Cargo.lock ./

# Copy only the workspace members needed for emergent-engine
COPY emergent-engine ./emergent-engine
COPY sdks/rust ./sdks/rust
COPY examples ./examples

# Build release binary with static linking
RUN cargo build --release --package emergent-engine --target x86_64-unknown-linux-musl

# ------------------------------------------------------------------------------
# Stage 2: Runtime
# ------------------------------------------------------------------------------
FROM alpine:3.21

# Install runtime dependencies (minimal)
RUN apk add --no-cache ca-certificates

# Create non-root user for security
RUN addgroup -g 1000 emergent && \
    adduser -u 1000 -G emergent -h /home/emergent -D emergent

# Create directory structure for volumes
RUN mkdir -p /etc/emergent /var/lib/emergent/logs /run/emergent && \
    chown -R emergent:emergent /var/lib/emergent /run/emergent

# Copy the compiled binary
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/emergent /usr/local/bin/emergent

# Set ownership
RUN chmod +x /usr/local/bin/emergent

# Switch to non-root user
USER emergent

# Environment variables
ENV RUST_LOG=info

# Volume mount points
VOLUME ["/etc/emergent", "/var/lib/emergent", "/run/emergent"]

# Default command
ENTRYPOINT ["/usr/local/bin/emergent"]
CMD ["--config", "/etc/emergent/emergent.toml"]

# Labels
LABEL org.opencontainers.image.source="https://github.com/Govcraft/emergent"
LABEL org.opencontainers.image.description="Emergent - Event-driven workflow engine"
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"
