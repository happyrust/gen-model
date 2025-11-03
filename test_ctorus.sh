#!/bin/bash
set -e

echo "=== Testing CTorus CSG Mesh Generation ==="
echo ""

cd /Volumes/DPC/work/plant-code/gen-model

# 清理旧的输出
rm -rf output/capture-L1/*

echo "1. Checking rs-core compilation..."
cd /Volumes/DPC/work/plant-code/rs-core
cargo check --lib 2>&1 | head -20
echo ""

echo "2. Running model generation test..."
cd /Volumes/DPC/work/plant-code/gen-model
cargo run --bin aios-database -- --debug-model 21491/18957 --capture output/capture-L1 --capture-include-descendants 2>&1 | tee /tmp/test_output.log | tail -100

echo ""
echo "3. Checking for CTorus warnings..."
grep -i "ctorus" /tmp/test_output.log || echo "No CTorus warnings found (good!)"

echo ""
echo "4. Checking output files..."
ls -lh output/capture-L1/ 2>/dev/null || echo "No output files found"

echo ""
echo "=== Test completed ==="


