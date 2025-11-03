#!/bin/bash

# Setup script for gRPC-Web proxy
# This script sets up either Envoy or grpcwebproxy for browser gRPC support

set -e

echo "gRPC-Web Proxy Setup"
echo "===================="

# Function to check if a command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Option 1: Use grpcwebproxy (simpler, Go-based)
setup_grpcwebproxy() {
    echo "Setting up grpcwebproxy..."
    
    if ! command_exists grpcwebproxy; then
        echo "Installing grpcwebproxy..."
        
        # Detect OS and architecture
        OS=$(uname -s | tr '[:upper:]' '[:lower:]')
        ARCH=$(uname -m)
        if [ "$ARCH" = "x86_64" ]; then
            ARCH="amd64"
        elif [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
            ARCH="arm64"
        fi
        
        # Download latest release
        VERSION="0.15.0"
        URL="https://github.com/improbable-eng/grpc-web/releases/download/v${VERSION}/grpcwebproxy-v${VERSION}-${OS}-${ARCH}.tar.gz"
        
        echo "Downloading from: $URL"
        curl -L "$URL" -o grpcwebproxy.tar.gz
        tar -xzf grpcwebproxy.tar.gz
        chmod +x grpcwebproxy
        
        # Move to local bin directory
        mkdir -p ./bin
        mv grpcwebproxy ./bin/
        rm grpcwebproxy.tar.gz
        
        echo "grpcwebproxy installed to ./bin/grpcwebproxy"
    fi
    
    echo "Starting grpcwebproxy..."
    ./bin/grpcwebproxy \
        --backend_addr=localhost:50051 \
        --run_tls_server=false \
        --allow_all_origins \
        --server_http_debug_port=8080 \
        --server_http_max_read_timeout=30s \
        --server_http_max_write_timeout=30s &
    
    PROXY_PID=$!
    echo "grpcwebproxy started with PID: $PROXY_PID"
    echo $PROXY_PID > .grpcwebproxy.pid
}

# Option 2: Use Envoy (more powerful, but requires Docker)
setup_envoy() {
    echo "Setting up Envoy proxy..."
    
    if ! command_exists docker; then
        echo "Docker is required for Envoy. Please install Docker first."
        echo "Alternatively, run with --use-grpcwebproxy flag"
        exit 1
    fi
    
    echo "Starting Envoy with Docker..."
    docker run -d \
        --name grpc-web-envoy \
        -p 8080:8080 \
        -p 9901:9901 \
        -v "$(pwd)/envoy-grpc-web.yaml:/etc/envoy/envoy.yaml" \
        --network host \
        envoyproxy/envoy:v1.28-latest \
        -c /etc/envoy/envoy.yaml
    
    echo "Envoy proxy started on port 8080"
    echo "Admin interface available at http://localhost:9901"
}

# Parse command line arguments
USE_GRPCWEBPROXY=true
while [[ $# -gt 0 ]]; do
    case $1 in
        --use-envoy)
            USE_GRPCWEBPROXY=false
            shift
            ;;
        --use-grpcwebproxy)
            USE_GRPCWEBPROXY=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--use-envoy | --use-grpcwebproxy]"
            exit 1
            ;;
    esac
done

# Check if gRPC service is running
if ! nc -z localhost 50051 2>/dev/null; then
    echo "Warning: gRPC service is not running on localhost:50051"
    echo "Please ensure your gRPC service is running before using the proxy"
fi

# Setup the chosen proxy
if [ "$USE_GRPCWEBPROXY" = true ]; then
    setup_grpcwebproxy
else
    setup_envoy
fi

echo ""
echo "gRPC-Web proxy is ready!"
echo "========================"
echo "Proxy endpoint: http://localhost:8080"
echo "gRPC backend: localhost:50051"
echo ""
echo "To stop the proxy:"
if [ "$USE_GRPCWEBPROXY" = true ]; then
    echo "  kill \$(cat .grpcwebproxy.pid)"
else
    echo "  docker stop grpc-web-envoy && docker rm grpc-web-envoy"
fi