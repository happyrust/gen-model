#!/bin/bash

# Deploy script for aios web-server and aios-database
# Usage: ./deploy.sh [web-server|aios-database|docker]

set -e

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}🚀 AIOS Deployment Script${NC}"

# Function to check command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Function to build web-server
build_web_server() {
    echo -e "${YELLOW}Building web-server...${NC}"
    
    if ! command_exists cargo; then
        echo -e "${RED}Error: Rust/Cargo is not installed${NC}"
        exit 1
    fi
    
    cargo build --release --bin web_server --no-default-features --features ws,sqlite-index,surreal-save,web_server
    
    echo -e "${GREEN}✅ Web-server build completed${NC}"
}

# Function to build aios-database library
build_aios_database() {
    echo -e "${YELLOW}Building aios-database library...${NC}"
    
    if ! command_exists cargo; then
        echo -e "${RED}Error: Rust/Cargo is not installed${NC}"
        exit 1
    fi
    
    cargo build --release --lib --no-default-features --features ws,sqlite-index,surreal-save,web_server
    
    echo -e "${GREEN}✅ Aios-database library build completed${NC}"
}

# Function to create deployment package
create_package() {
    local app_name=$1
    echo -e "${YELLOW}Creating deployment package for ${app_name}...${NC}"
    
    mkdir -p "deploy/${app_name}"
    
    if [ "$app_name" = "web-server" ]; then
        cp target/release/web_server "deploy/${app_name}/"
        cp DbOption.toml "deploy/${app_name}/" 2>/dev/null || echo "Warning: DbOption.toml not found"
        cp -r assets "deploy/${app_name}/" 2>/dev/null || true
        cp -r data "deploy/${app_name}/" 2>/dev/null || true
        cp -r web-test "deploy/${app_name}/" 2>/dev/null || true
        
        case "$(uname -s)" in
            Linux*) tar -czf "deploy/${app_name}-linux-x86_64.tar.gz" -C deploy "${app_name}"
                   ;;
            Darwin*) tar -czf "deploy/${app_name}-macos-x86_64.tar.gz" -C deploy "${app_name}"
                    ;;
            CYGWIN*|MINGW*|MSYS*) zip -r "deploy/${app_name}-windows-x86_64.zip" "deploy/${app_name}"
                              ;;
        esac
    elif [ "$app_name" = "aios-database" ]; then
        cp target/release/libaios_database.* "deploy/${app_name}/" 2>/dev/null || \
        cp target/release/deps/libaios_database* "deploy/${app_name}/" 2>/dev/null || true
        
        # Copy header files (if generated)
        find target -name "*.h" -path "*/aios_database*" -exec cp {} "deploy/${app_name}/" \; 2>/dev/null || true
    fi
    
    echo -e "${GREEN}✅ Deployment package created for ${app_name}${NC}"
}

# Function to test deployment
test_deployment() {
    local app_name=$1
    echo -e "${YELLOW}Testing ${app_name} deployment...${NC}"
    
    if [ "$app_name" = "web-server" ]; then
        if [ -f "deploy/web-server/web_server" ] || [ -f "deploy/web-server/web_server.exe" ]; then
            echo -e "${GREEN}✅ Web-server binary exists${NC}"
            cd deploy/web-server
            ./web_server --version || echo "Version check failed, but binary exists"
            cd - > /dev/null
        fi
    fi
    
    echo -e "${GREEN}✅ Deployment test completed${NC}"
}

# Function to build Docker image
build_docker() {
    echo -e "${YELLOW}Building Docker image...${NC}"
    
    if ! command_exists docker; then
        echo -e "${RED}Error: Docker is not installed${NC}"
        exit 1
    fi
    
    docker build -t aios/web-server:latest .
    
    echo -e "${GREEN}✅ Docker image build completed${NC}"
    echo -e "${YELLOW}To run the container, use:${NC}"
    echo -e "${YELLOW}docker-compose up -d${NC}"
}

# Function to cleanup
cleanup() {
    echo -e "${YELLOW}Cleaning up...${NC}"
    rm -rf deploy/
    cargo clean
}

# Main deployment logic
case "${1:-all}" in
    "web-server")
        build_web_server
        create_package "web-server"
        test_deployment "web-server"
        ;;
    "aios-database")
        build_aios_database
        create_package "aios-database"
        test_deployment "aios-database"
        ;;
    "docker")
        build_docker
        ;;
    "all")
        build_web_server
        build_aios_database
        create_package "web-server"
        create_package "aios-database"
        test_deployment "web-server"
        test_deployment "aios-database"
        build_docker
        ;;
    "cleanup")
        cleanup
        ;;
    *)
        echo "Usage: $0 [web-server|aios-database|docker|all|cleanup]"
        echo ""
        echo " Commands:"
        echo "  web-server   - Build and package web-server only"
        echo "  aios-database - Build and package aios-database library only"
        echo "  docker       - Build Docker image"
        echo "  all          - Build everything (default)"
        echo "  cleanup      - Clean build artifacts and deployments"
        exit 1
        ;;
esac

echo -e "${GREEN}🎉 Deployment completed successfully!${NC}"
echo -e "${YELLOW}Check the 'deploy/' directory for packaged binaries${NC}"
