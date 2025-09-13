#!/bin/bash

# DB 7999 模型生成和空间索引测试脚本

echo "======================================"
echo "DB 7999 模型生成和空间索引测试"
echo "======================================"
echo ""

# 步骤1: 生成DB 7999的模型
echo "步骤1: 生成DB 7999的模型数据..."
echo "运行命令: cargo run --release -- -d 7999 -o db7999_model.xkt"

cargo run --release -- -d 7999 -o test_output/db7999_model.xkt

if [ $? -eq 0 ]; then
    echo "✓ 模型生成成功"
    echo ""
    
    # 检查生成的文件
    if [ -f "test_output/db7999_model.xkt" ]; then
        FILE_SIZE=$(ls -lh test_output/db7999_model.xkt | awk '{print $5}')
        echo "生成的XKT文件: test_output/db7999_model.xkt"
        echo "文件大小: $FILE_SIZE"
    fi
else
    echo "✗ 模型生成失败"
    exit 1
fi

echo ""
echo "步骤2: 构建空间索引..."
echo "运行命令: cargo run --example spatial_index_builder --features grpc,sqlite-index -- --db 7999"

# 构建空间索引
cargo run --example spatial_index_builder --features grpc,sqlite-index -- --db 7999

if [ $? -eq 0 ]; then
    echo "✓ 空间索引构建成功"
else
    echo "⚠️ 空间索引构建失败（可能需要实现spatial_index_builder示例）"
fi

echo ""
echo "步骤3: 测试SCTN空间查询..."

# 运行SCTN测试
cargo test test_sctn_7999 --features grpc,sqlite-index -- --nocapture

echo ""
echo "======================================"
echo "测试完成！"
echo "======================================"