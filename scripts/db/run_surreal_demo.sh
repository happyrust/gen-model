#!/usr/bin/env bash
 
PORT=8020
 
PIDS=$(lsof -ti :"$PORT" 2>/dev/null)
if [ -n "$PIDS" ]; then
  echo "Killing process on port $PORT: $PIDS"
  kill -9 $PIDS
fi
 
surreal start --user root --pass root --bind 127.0.0.1:8020 rocksdb://ams-demo.db
