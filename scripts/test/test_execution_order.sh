#!/bin/bash

# 测试 Full Noun 模式执行顺序

echo "启动测试程序..."
./target/debug/aios-database 2>&1 | tee /tmp/full_noun_test.log &
PID=$!

# 等待10秒
sleep 10

# 终止程序
kill $PID 2>/dev/null

echo ""
echo "========================================"
echo "执行顺序验证："
echo "========================================"
grep -E "\[gen_model\]|Full Noun|📍|\[1/3\]|\[2/3\]|\[3/3\]|LOOP|PRIM|CATE" /tmp/full_noun_test.log

echo ""
echo "========================================"
echo "完整日志保存在: /tmp/full_noun_test.log"
echo "========================================"
