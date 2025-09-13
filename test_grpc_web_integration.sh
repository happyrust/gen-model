#!/bin/bash

# Test script for gRPC-Web integration
# This script tests the complete gRPC-Web setup including proxy and client

set -e

echo "====================================="
echo "gRPC-Web Integration Test"
echo "====================================="
echo ""

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    if [ "$1" = "success" ]; then
        echo -e "${GREEN}✓${NC} $2"
    elif [ "$1" = "error" ]; then
        echo -e "${RED}✗${NC} $2"
    elif [ "$1" = "info" ]; then
        echo -e "${YELLOW}ℹ${NC} $2"
    fi
}

# Check if required services are running
check_services() {
    echo "1. Checking required services..."
    echo ""
    
    # Check if main application is running
    if lsof -i:8000 > /dev/null 2>&1; then
        print_status "success" "Web UI is running on port 8000"
    else
        print_status "error" "Web UI is not running on port 8000"
        print_status "info" "Start it with: cargo run --bin web_ui"
    fi
    
    # Check if gRPC service is running
    if lsof -i:50051 > /dev/null 2>&1; then
        print_status "success" "gRPC service is running on port 50051"
    else
        print_status "error" "gRPC service is not running on port 50051"
        print_status "info" "Start it with: cargo run --bin gen_model -- --grpc"
    fi
    
    # Check if gRPC-Web proxy is running
    if lsof -i:8080 > /dev/null 2>&1; then
        print_status "success" "gRPC-Web proxy is running on port 8080"
    else
        print_status "error" "gRPC-Web proxy is not running on port 8080"
        print_status "info" "Start it with: ./setup-grpc-web.sh"
    fi
    
    echo ""
}

# Test HTTP API endpoint
test_http_api() {
    echo "2. Testing HTTP API endpoint..."
    echo ""
    
    RESPONSE=$(curl -s -X GET "http://localhost:8000/api/sqlite-spatial/query?minx=0&maxx=10&miny=0&maxy=10&minz=0&maxz=10" 2>/dev/null || echo "FAILED")
    
    if [ "$RESPONSE" != "FAILED" ] && echo "$RESPONSE" | grep -q "results"; then
        print_status "success" "HTTP API is responding correctly"
        RESULT_COUNT=$(echo "$RESPONSE" | grep -o '"results":\[[^]]*\]' | grep -o '"id"' | wc -l)
        print_status "info" "Found $RESULT_COUNT results via HTTP"
    else
        print_status "error" "HTTP API test failed"
    fi
    
    echo ""
}

# Test gRPC-Web proxy
test_grpc_web() {
    echo "3. Testing gRPC-Web proxy..."
    echo ""
    
    # Test if proxy is accessible
    PROXY_TEST=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:8080/ 2>/dev/null || echo "000")
    
    if [ "$PROXY_TEST" != "000" ]; then
        print_status "success" "gRPC-Web proxy is accessible"
    else
        print_status "error" "Cannot connect to gRPC-Web proxy"
        return
    fi
    
    # Test gRPC-Web request (simplified test)
    # Note: Real gRPC-Web requests require proper encoding
    GRPC_TEST=$(curl -s -X POST \
        -H "Content-Type: application/grpc-web+proto" \
        -H "X-Grpc-Web: 1" \
        --data-binary "" \
        http://localhost:8080/spatial_query.SpatialQueryService/GetIndexStats \
        -o /dev/null -w "%{http_code}" 2>/dev/null || echo "000")
    
    if [ "$GRPC_TEST" = "200" ] || [ "$GRPC_TEST" = "204" ]; then
        print_status "success" "gRPC-Web proxy is handling requests"
    else
        print_status "info" "gRPC-Web proxy returned status: $GRPC_TEST"
    fi
    
    echo ""
}

# Test web UI integration
test_web_ui() {
    echo "4. Testing Web UI integration..."
    echo ""
    
    # Check if unified spatial query page is accessible
    UI_TEST=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:8000/spatial-query 2>/dev/null || echo "000")
    
    if [ "$UI_TEST" = "200" ]; then
        print_status "success" "Spatial query UI page is accessible"
    else
        print_status "error" "Cannot access spatial query UI (status: $UI_TEST)"
    fi
    
    # Check if gRPC client script is accessible
    SCRIPT_TEST=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:8000/static/grpc-client.js 2>/dev/null || echo "000")
    
    if [ "$SCRIPT_TEST" = "200" ]; then
        print_status "success" "gRPC client JavaScript is accessible"
    else
        print_status "error" "Cannot access gRPC client script (status: $SCRIPT_TEST)"
    fi
    
    echo ""
}

# Performance comparison test
performance_test() {
    echo "5. Running performance comparison..."
    echo ""
    
    # Test HTTP performance
    print_status "info" "Testing HTTP API performance (10 requests)..."
    HTTP_TOTAL=0
    for i in {1..10}; do
        START=$(date +%s%N)
        curl -s "http://localhost:8000/api/sqlite-spatial/query?minx=0&maxx=10&miny=0&maxy=10&minz=0&maxz=10" > /dev/null 2>&1
        END=$(date +%s%N)
        ELAPSED=$((($END - $START) / 1000000))
        HTTP_TOTAL=$(($HTTP_TOTAL + $ELAPSED))
    done
    HTTP_AVG=$(($HTTP_TOTAL / 10))
    print_status "success" "HTTP average response time: ${HTTP_AVG}ms"
    
    # Note: Real gRPC-Web performance test would require proper client
    print_status "info" "gRPC-Web performance test requires browser environment"
    
    echo ""
}

# Main test execution
main() {
    check_services
    test_http_api
    test_grpc_web
    test_web_ui
    performance_test
    
    echo "====================================="
    echo "Test Summary"
    echo "====================================="
    echo ""
    print_status "info" "Open http://localhost:8000/spatial-query in browser"
    print_status "info" "Select 'Both Interfaces' to compare HTTP vs gRPC"
    print_status "info" "Press Ctrl+B in the UI to run benchmark tests"
    echo ""
    echo "Integration test complete!"
}

# Run the tests
main