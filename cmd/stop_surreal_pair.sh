#!/bin/bash
set -euo pipefail

echo "=== Checking for SurrealDB processes on ports 8009 and 8010 ==="
echo ""

# Check port 8009
echo "Port 8009:"
if pids=$(sudo lsof -n -P -t -iTCP:8009 -sTCP:LISTEN 2>/dev/null); then
  echo "  Found PIDs: $pids"
  for pid in $pids; do
    echo "  Killing PID $pid..."
    sudo kill -9 "$pid" 2>/dev/null || true
  done
else
  echo "  No process found (or need sudo password)"
fi
echo ""

# Check port 8010
echo "Port 8010:"
if pids=$(sudo lsof -n -P -t -iTCP:8010 -sTCP:LISTEN 2>/dev/null); then
  echo "  Found PIDs: $pids"
  for pid in $pids; do
    echo "  Killing PID $pid..."
    sudo kill -9 "$pid" 2>/dev/null || true
  done
else
  echo "  No process found (or need sudo password)"
fi
echo ""

# Also check by process name
echo "All surreal processes:"
ps aux | grep "[s]urreal start" || echo "  None found"
echo ""

echo "=== Done ==="
echo "You can now run: bash cmd/run_surreal_pair.sh"
