#!/bin/bash

# 空间查询服务测试脚本
# 用法: ./test_spatial_query.sh

echo "🚀 空间查询服务测试脚本"
echo "=========================================="

# 检查是否存在必要的文件
if [ ! -f "Cargo.toml" ]; then
    echo "❌ 错误: 请在项目根目录运行此脚本"
    exit 1
fi

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}📋 测试步骤:${NC}"
echo "1. 编译项目"
echo "2. 启动空间查询服务器"
echo "3. 运行客户端测试"
echo "4. 清理进程"
echo ""

# 步骤1: 编译项目
echo -e "${BLUE}🔨 步骤1: 编译项目${NC}"
if cargo build --release; then
    echo -e "${GREEN}✅ 项目编译成功${NC}"
else
    echo -e "${RED}❌ 项目编译失败${NC}"
    exit 1
fi

# 步骤2: 启动服务器
echo -e "\n${BLUE}🚀 步骤2: 启动空间查询服务器${NC}"
echo "启动服务器在后台运行..."

# 启动服务器并获取进程ID
cargo run --bin gen_model -- --spatial-query-server &
SERVER_PID=$!

echo "服务器进程ID: $SERVER_PID"

# 等待服务器启动
echo "等待服务器启动..."
sleep 3

# 检查服务器是否正在运行
if ps -p $SERVER_PID > /dev/null; then
    echo -e "${GREEN}✅ 服务器启动成功 (PID: $SERVER_PID)${NC}"
else
    echo -e "${RED}❌ 服务器启动失败${NC}"
    exit 1
fi

# 检查端口是否监听
if command -v nc >/dev/null 2>&1; then
    if nc -z 127.0.0.1 9090; then
        echo -e "${GREEN}✅ 服务器正在监听端口 9090${NC}"
    else
        echo -e "${YELLOW}⚠️  端口 9090 可能未就绪，继续测试...${NC}"
    fi
else
    echo -e "${YELLOW}⚠️  nc 命令不可用，跳过端口检查${NC}"
fi

# 步骤3: 运行客户端测试
echo -e "\n${BLUE}🧪 步骤3: 运行客户端测试${NC}"

# 设置清理函数
cleanup() {
    echo -e "\n${YELLOW}🧹 清理进程...${NC}"
    if ps -p $SERVER_PID > /dev/null; then
        kill $SERVER_PID
        echo "已终止服务器进程 $SERVER_PID"
    fi
    
    # 确保端口完全释放
    sleep 1
    if command -v lsof >/dev/null 2>&1; then
        lsof -ti:9090 | xargs kill -9 2>/dev/null || true
    fi
    
    exit $1
}

# 设置信号处理
trap 'cleanup 1' INT TERM

# 运行客户端测试
echo "正在执行客户端测试..."
if cargo run --example spatial_query_client; then
    echo -e "${GREEN}✅ 客户端测试完成${NC}"
    TEST_RESULT=0
else
    echo -e "${RED}❌ 客户端测试失败${NC}"
    TEST_RESULT=1
fi

# 步骤4: 性能测试（可选）
echo -e "\n${BLUE}⚡ 步骤4: 性能测试${NC}"
echo "正在进行简单的性能测试..."

# 创建临时性能测试脚本
cat > /tmp/perf_test.py << 'EOF'
#!/usr/bin/env python3
import grpc
import time
import spatial_query_pb2
import spatial_query_pb2_grpc
import statistics

def run_performance_test():
    channel = grpc.insecure_channel('localhost:9090')
    stub = spatial_query_pb2_grpc.SpatialQueryServiceStub(channel)
    
    # 预热
    for _ in range(5):
        request = spatial_query_pb2.SpatialQueryRequest(
            refno=1001,
            include_self=False,
            tolerance=0.001,
            max_results=100
        )
        stub.QueryIntersectingElements(request)
    
    # 性能测试
    times = []
    for i in range(100):
        start = time.time()
        request = spatial_query_pb2.SpatialQueryRequest(
            refno=1001 + (i % 4),  # 轮换查询不同构件
            include_self=False,
            tolerance=0.001,
            max_results=100
        )
        response = stub.QueryIntersectingElements(request)
        end = time.time()
        times.append((end - start) * 1000)  # 转换为毫秒
        
        if i % 20 == 19:
            print(f"已完成 {i+1}/100 次查询")
    
    print(f"\n性能统计 (100次查询):")
    print(f"平均响应时间: {statistics.mean(times):.2f} ms")
    print(f"最小响应时间: {min(times):.2f} ms")
    print(f"最大响应时间: {max(times):.2f} ms")
    print(f"中位数响应时间: {statistics.median(times):.2f} ms")
    print(f"99分位数: {sorted(times)[98]:.2f} ms")

if __name__ == "__main__":
    try:
        run_performance_test()
    except ImportError:
        print("❌ 缺少 grpc 相关的 Python 库，跳过性能测试")
        print("   如需性能测试，请安装: pip install grpcio grpcio-tools")
    except Exception as e:
        print(f"❌ 性能测试失败: {e}")
EOF

# 尝试运行性能测试
if command -v python3 >/dev/null 2>&1; then
    if python3 -c "import grpc" 2>/dev/null; then
        echo "运行 Python 性能测试..."
        python3 /tmp/perf_test.py
    else
        echo -e "${YELLOW}⚠️  Python grpc 库未安装，跳过性能测试${NC}"
    fi
else
    echo -e "${YELLOW}⚠️  Python3 不可用，跳过性能测试${NC}"
fi

# 清理临时文件
rm -f /tmp/perf_test.py

# 最终报告
echo -e "\n${BLUE}📊 测试报告${NC}"
echo "=========================================="
if [ $TEST_RESULT -eq 0 ]; then
    echo -e "${GREEN}✅ 所有测试通过！${NC}"
    echo "空间查询服务运行正常，可以投入使用。"
else
    echo -e "${RED}❌ 部分测试失败${NC}"
    echo "请检查错误信息并修复问题。"
fi

echo -e "\n${BLUE}💡 使用提示:${NC}"
echo "- 服务器地址: http://127.0.0.1:9090"
echo "- 可以使用 grpcurl 进行手动测试"
echo "- 查看服务定义: proto/spatial_query_service.proto"

# 清理并退出
cleanup $TEST_RESULT