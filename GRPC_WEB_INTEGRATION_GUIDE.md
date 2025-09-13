# gRPC-Web Integration Guide

## Overview

This guide explains the implementation of gRPC-Web support for the spatial query service, enabling web browsers to communicate with the gRPC backend service.

## Architecture

```
┌─────────────┐     HTTP/REST      ┌─────────────┐
│   Browser   │ ←─────────────────→ │   Web UI    │
│             │                     │  (Port 8000) │
└─────────────┘                     └─────────────┘
       ↓                                    ↓
   gRPC-Web                            HTTP API
       ↓                                    ↓
┌─────────────┐                     ┌─────────────┐
│ gRPC-Web    │                     │   SQLite    │
│   Proxy     │                     │   Spatial   │
│ (Port 8080) │                     │    Index    │
└─────────────┘                     └─────────────┘
       ↓
     gRPC
       ↓
┌─────────────┐
│ gRPC Service│
│ (Port 50051)│
└─────────────┘
```

## Components

### 1. gRPC Service (`spatial_query_service.rs`)
- Native gRPC service implementing spatial queries
- Runs on port 50051
- Provides high-performance RPC methods:
  - `QueryIntersectingElements`
  - `BatchQueryIntersecting`
  - `RebuildSpatialIndex`
  - `GetIndexStats`

### 2. gRPC-Web Proxy
Two options are provided:

#### Option A: grpcwebproxy (Recommended for development)
- Lightweight Go-based proxy
- Easy to install and configure
- Suitable for development and testing

#### Option B: Envoy (Recommended for production)
- Industry-standard proxy
- Advanced features and better performance
- Requires Docker

### 3. gRPC-Web Client (`grpc-client.js`)
- JavaScript client library for browser
- Handles gRPC-Web protocol
- Provides methods matching the gRPC service
- Includes performance benchmarking

### 4. Unified Web UI (`spatial_query_unified.html`)
- Interactive interface supporting both HTTP and gRPC
- Real-time performance comparison
- Visual query results display

## Setup Instructions

### Step 1: Start the gRPC Service

```bash
# Start the main application with gRPC support
cargo run --bin gen_model -- --grpc --port 50051
```

### Step 2: Setup gRPC-Web Proxy

```bash
# Install and start the gRPC-Web proxy
./setup-grpc-web.sh

# Or use Envoy instead
./setup-grpc-web.sh --use-envoy
```

### Step 3: Start the Web UI

```bash
# Start the web UI server
cargo run --bin web_ui

# The UI will be available at http://localhost:8000/spatial-query
```

### Step 4: Test the Integration

```bash
# Run the integration test
./test_grpc_web_integration.sh
```

## Usage

### Web UI

1. Open http://localhost:8000/spatial-query
2. Select interface type:
   - **HTTP API**: Traditional REST API
   - **gRPC Service**: gRPC-Web interface
   - **Both Interfaces**: Compare performance

3. Enter query parameters (bounding box coordinates)
4. Click "Execute Query" or press Ctrl+Enter
5. View results and performance metrics

### Performance Benchmarking

Press Ctrl+B in the web UI to run automated performance benchmarks comparing HTTP and gRPC interfaces.

### Programmatic Usage

```javascript
// Create gRPC client
const client = new GrpcSpatialQueryClient('http://localhost:8080');

// Query intersecting elements
const response = await client.queryIntersectingElements({
    refno: 1000,
    customBbox: {
        min: { x: 0, y: 0, z: 0 },
        max: { x: 10, y: 10, z: 10 }
    },
    tolerance: 0.001,
    maxResults: 1000
});

// Run benchmark
const benchmark = await client.benchmarkComparison(request, 10);
console.log(`gRPC is ${benchmark.improvement.factor}x faster`);
```

## Performance Comparison

### Expected Performance Characteristics

| Interface | Latency | Throughput | Use Case |
|-----------|---------|------------|----------|
| HTTP REST | ~20-50ms | Medium | Web browsers, simple queries |
| gRPC | ~5-15ms | High | Service-to-service, batch operations |

### Factors Affecting Performance

1. **Network Overhead**: gRPC uses HTTP/2 with binary protocol
2. **Serialization**: Protocol Buffers vs JSON
3. **Connection Reuse**: gRPC maintains persistent connections
4. **Streaming**: gRPC supports bidirectional streaming

## Troubleshooting

### Common Issues

1. **gRPC-Web proxy not accessible**
   - Check if proxy is running: `lsof -i:8080`
   - Restart proxy: `./setup-grpc-web.sh`

2. **CORS errors in browser**
   - Ensure proxy CORS configuration is correct
   - Check browser console for specific errors

3. **Performance not improved**
   - Verify gRPC service is running
   - Check network latency
   - Ensure proper connection pooling

### Debug Commands

```bash
# Check service status
lsof -i:8000   # Web UI
lsof -i:8080   # gRPC-Web proxy
lsof -i:50051  # gRPC service

# Test gRPC service directly
grpcurl -plaintext localhost:50051 list

# Monitor proxy logs (if using Docker/Envoy)
docker logs grpc-web-envoy -f
```

## Configuration Files

- `grpc-web-proxy.yaml`: Proxy configuration
- `envoy-grpc-web.yaml`: Envoy configuration
- `proto/spatial_query_service.proto`: Service definition

## Future Enhancements

1. **Protocol Buffer Generation**: Automate .proto to JavaScript compilation
2. **Streaming Support**: Implement server-streaming for real-time updates
3. **Load Balancing**: Add multiple gRPC backend support
4. **Authentication**: Implement JWT/OAuth for secure access
5. **Metrics Collection**: Add Prometheus metrics for monitoring

## References

- [gRPC-Web Documentation](https://github.com/grpc/grpc-web)
- [Envoy Proxy](https://www.envoyproxy.io/)
- [Protocol Buffers](https://developers.google.com/protocol-buffers)