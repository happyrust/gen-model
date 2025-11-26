#!/bin/bash

# 测试 25688/7957 的布尔运算问题

echo "========================================="
echo "测试 25688/7957 布尔运算问题"
echo "========================================="

# 1. 查询 neg_relate 关系
echo ""
echo "1. 查询 neg_relate 关系："
surreal sql --conn http://localhost:8000 --user root --pass root --ns 1516 --db DESI --pretty <<'EOF'
SELECT * FROM neg_relate WHERE out = pe:25688_7957;
EOF

echo ""
echo "========================================="

