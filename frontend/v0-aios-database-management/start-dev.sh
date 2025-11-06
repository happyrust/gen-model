#!/bin/bash

# Start development environment for AIOS Database Management Platform
# This script starts both the Rust backend and Next.js frontend

echo "🚀 Starting AIOS Database Management Platform Development Environment"
echo ""

# Check if we're in the right directory
if [ ! -f "package.json" ]; then
    echo "❌ Error: Please run this script from the frontend directory"
    exit 1
fi

# Check if Rust backend is available
if [ ! -f "../../Cargo.toml" ]; then
    echo "❌ Error: Rust backend not found. Please ensure you're in the correct directory structure"
    exit 1
fi

# Function to kill process using a specific port
kill_port_process() {
    local port=$1
    local port_name=$2

    echo "🔍 Checking if port $port is in use..."

    # Find PID using the port (macOS compatible)
    local pid=$(lsof -ti:$port)

    if [ ! -z "$pid" ]; then
        echo "⚠️  Port $port is occupied by process $pid ($port_name)"
        echo "🔪 Killing process $pid..."
        kill -9 $pid 2>/dev/null

        # Wait a moment to ensure process is killed
        sleep 1

        # Verify the process is killed
        if lsof -ti:$port > /dev/null 2>&1; then
            echo "❌ Failed to kill process on port $port"
            exit 1
        else
            echo "✅ Successfully killed process on port $port"
        fi
    else
        echo "✅ Port $port is available"
    fi
    echo ""
}

# Kill processes occupying required ports
kill_port_process 8080 "Backend"
kill_port_process 3000 "Frontend"

echo "📋 Starting services..."
echo ""

# Start Rust backend in background
echo "🔧 Starting Rust backend service (port 8080)..."
cd ../../
cargo run --bin web_server --features "web_server,ws,gen_model,manifold,project_hd" &
BACKEND_PID=$!

# Wait a moment for backend to start
sleep 3

# Go back to frontend directory
cd frontend/v0-aios-database-management

# Start Next.js frontend
echo "🌐 Starting Next.js frontend (port 3000)..."
pnpm run dev &
FRONTEND_PID=$!

echo ""
echo "✅ Services started successfully!"
echo ""
echo "🔗 Access URLs:"
echo "   Frontend: http://localhost:3000"
echo "   Backend API: http://localhost:8080"
echo ""
echo "📝 Process IDs:"
echo "   Backend PID: $BACKEND_PID"
echo "   Frontend PID: $FRONTEND_PID"
echo ""
echo "🛑 To stop services, run:"
echo "   kill $BACKEND_PID $FRONTEND_PID"
echo ""

# Wait for user to stop
wait
