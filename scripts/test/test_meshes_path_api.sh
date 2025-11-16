#!/bin/bash

# 测试 meshes_path 参数的 API 脚本

echo "=========================================="
echo "测试 1: 不指定 meshes_path (使用默认配置)"
echo "=========================================="
curl -X POST http://localhost:8080/api/generate-by-refno \
  -H "Content-Type: application/json" \
  -d '{
    "db_num": 1112,
    "refnos": ["17496/201375"],
    "gen_mesh": true,
    "gen_model": false
  }' | jq '.'

echo ""
echo ""
echo "=========================================="
echo "测试 2: 指定自定义 meshes_path"
echo "=========================================="
curl -X POST http://localhost:8080/api/generate-by-refno \
  -H "Content-Type: application/json" \
  -d '{
    "db_num": 1112,
    "refnos": ["17496/201375"],
    "gen_mesh": true,
    "gen_model": false,
    "meshes_path": "/Volumes/DPC/work/plant-code/gen-model-fork/output/custom_meshes"
  }' | jq '.'

echo ""
echo ""
echo "=========================================="
echo "测试 3: 使用相对路径的 meshes_path"
echo "=========================================="
curl -X POST http://localhost:8080/api/generate-by-refno \
  -H "Content-Type: application/json" \
  -d '{
    "db_num": 1112,
    "refnos": ["17496/201375"],
    "gen_mesh": true,
    "gen_model": false,
    "meshes_path": "output/test_meshes"
  }' | jq '.'

echo ""
echo ""
echo "✅ 测试完成！"
echo "请检查服务器日志和输出目录以验证 meshes_path 参数是否生效。"

