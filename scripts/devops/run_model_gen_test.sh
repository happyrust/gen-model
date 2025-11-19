#!/bin/bash
# 模型生成和 AABB 更新测试脚本

set -e

echo "=================================="
echo "模型生成和 AABB 更新测试"
echo "=================================="
echo ""

# 检查 SurrealDB 是否运行
echo "📋 步骤 1: 检查 SurrealDB 状态"
echo "--------------------------------"
if ps aux | grep -v grep | grep "surreal.*8009" > /dev/null; then
    echo "✅ SurrealDB (8009) 运行中"
else
    echo "❌ SurrealDB (8009) 未运行"
    echo "请先启动 SurrealDB"
    exit 1
fi

# 检查数据库当前状态
echo ""
echo "📋 步骤 2: 检查数据库当前状态"
echo "--------------------------------"
echo "查询 inst_relate 表记录数..."

# 使用简单的 curl 查询
RESPONSE=$(curl -s -X POST http://127.0.0.1:8009/sql \
  -H "Content-Type: application/json" \
  -H "NS: 1516" \
  -H "DB: 1112" \
  -H "Accept: application/json" \
  -u "root:root" \
  -d '{"sql": "SELECT count() as total FROM inst_relate;"}' || echo "")

echo "当前数据库响应: $RESPONSE"

# 检查配置
echo ""
echo "📋 步骤 3: 检查配置文件"
echo "--------------------------------"
if grep -q "gen_model = true" DbOption.toml && grep -q "gen_mesh = true" DbOption.toml; then
    echo "✅ 配置正确: gen_model=true, gen_mesh=true"
else
    echo "❌ 配置需要检查"
fi

MANUAL_DB=$(grep "manual_db_nums" DbOption.toml | head -1)
echo "目标数据库配置: $MANUAL_DB"

# 运行模型生成（使用较短的超时）
echo ""
echo "📋 步骤 4: 运行模型生成"
echo "=================================="
echo "开始生成模型数据..."
echo "日志保存到: model_gen_output.log"
echo ""

# 运行生成，最长等待5分钟；如果系统没有 timeout 命令，则直接运行
if command -v timeout >/dev/null 2>&1; then
    # 使用 timeout 限制最长执行时间
    timeout 300 cargo run --bin aios-database --release 2>&1 | tee model_gen_output.log || {
        echo ""
        echo "⚠️ 模型生成超时或出错，但继续检查结果..."
    }
else
    echo "⚠️ 未找到 timeout 命令，直接运行 cargo run --bin aios-database --release（无超时限制）..."
    cargo run --bin aios-database --release 2>&1 | tee model_gen_output.log || {
        echo ""
        echo "⚠️ 模型生成出错，但继续检查结果..."
    }
fi

# 验证结果
echo ""
echo "📋 步骤 5: 验证结果"
echo "=================================="

# 检查日志
echo "检查生成日志..."
if [ -f "model_gen_output.log" ]; then
    echo "日志文件大小: $(wc -l < model_gen_output.log) 行"
    
    if grep -q "update_inst_relate_aabbs" model_gen_output.log; then
        echo "✅ 找到 AABB 更新相关日志:"
        grep -n "update_inst_relate_aabbs" model_gen_output.log | tail -3
    else
        echo "❌ 未找到 AABB 更新日志"
    fi
    
    if grep -q "gen_model.*完成" model_gen_output.log; then
        echo "✅ 找到生成完成日志"
    fi
    
    if grep -q "错误\|失败\|error\|Error" model_gen_output.log; then
        echo "⚠️ 发现错误信息:"
        grep -i "错误\|失败\|error" model_gen_output.log | tail -3
    fi
else
    echo "❌ 未找到日志文件"
fi

# 最终数据库验证
echo ""
echo "查询最终数据库状态..."

FINAL_RESPONSE=$(curl -s -X POST http://127.0.0.1:8009/sql \
  -H "Content-Type: application/json" \
  -H "NS: 1516" \
  -H "DB: 1112" \
  -H "Accept: application/json" \
  -u "root:root" \
  -d '{"sql": "SELECT count() as total FROM inst_relate; SELECT count() as with_aabb FROM inst_relate WHERE aabb != none;"}' || echo "")

echo "最终数据库状态: $FINAL_RESPONSE"

echo ""
echo "=================================="
echo "测试完成"
echo "=================================="
echo ""
echo "📝 详细日志: model_gen_output.log"
echo "🔍 如需查看详细 AABB 数据，运行:"
echo "   surreal sql --endpoint http://127.0.0.1:8009 --namespace 1516 --database 1112 --username root --password root"
echo ""
