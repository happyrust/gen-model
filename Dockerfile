# Multi-stage build for web-server
FROM rust:1.83.0-slim as builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    build-essential \
    cmake \
    g++ \
    git \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy Cargo files
COPY Cargo.toml Cargo.lock ./

# Create dummy source directory to cache dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release --bin web_server --no-default-features --features ws,sqlite-index,surreal-save,web_server && \
    rm -rf src target/release/deps/aios_database*

# Copy the actual source code
COPY src ./src
COPY . ./ 

# Build the application
RUN cargo build --release --bin web_server --no-default-features --features ws,sqlite-index,surreal-save,web_server

# Production image
FROM debian:12-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    sqlite3 \
    && rm -rf /var/lib/apt/lists/*

# Create application user
RUN useradd -r -s /bin/false appuser

# Set working directory
WORKDIR /app

# Copy the binary from builder stage
COPY --from=builder /app/target/release/web_server /usr/local/bin/web_server

# Copy configuration and assets
COPY --from=builder /app/DbOption.toml /app/
COPY --from=builder /app/assets /app/assets 2>/dev/null || true
COPY --from=builder /app/data /app/data 2>/dev/null || true
COPY --from=builder /app/web-test /app/web-test 2>/dev/null || true

# Set permissions
RUN chown -R appuser:appuser /app && \
    chmod +x /usr/local/bin/web_server

# Switch to non-root user
USER appuser

# Expose port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Start the application
CMD ["web_server"]
