#!/bin/bash

# 启动同步系统脚本
# 包括 MQTT 服务器和 Web UI

set -e

echo "==================================="
echo "  Gen-Model 同步系统启动器"
echo "==================================="

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# 检查命令是否存在
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# 启动 MQTT 服务器
start_mqtt_server() {
    echo -e "${YELLOW}[1/3] 启动 MQTT 服务器...${NC}"

    if [ -d "rumqttd-server" ]; then
        cd rumqttd-server

        # 检查是否已编译
        if [ ! -f "target/release/mqtt-server" ]; then
            echo "编译 MQTT 服务器..."
            cargo build --release
        fi

        # 后台启动 MQTT 服务器
        ./target/release/mqtt-server &
        MQTT_PID=$!
        echo -e "${GREEN}✓ MQTT 服务器已启动 (PID: $MQTT_PID)${NC}"
        cd ..
    else
        echo -e "${YELLOW}⚠ 未找到 rumqttd-server 目录，跳过 MQTT 服务器${NC}"
        echo "  你可以使用外部 MQTT 服务器，例如 Mosquitto"

        # 检查是否有 mosquitto
        if command_exists mosquitto; then
            echo -e "${YELLOW}  检测到 mosquitto，是否启动？(y/n)${NC}"
            read -r response
            if [[ "$response" == "y" ]]; then
                mosquitto -d
                echo -e "${GREEN}✓ Mosquitto 已在后台启动${NC}"
            fi
        fi
    fi
}

# 启动 Web UI
start_web_server() {
    echo -e "${YELLOW}[2/3] 启动 Web UI 服务器...${NC}"

    # 检查是否已编译
    if [ ! -f "target/release/web_server" ]; then
        echo "编译 Web UI..."
        cargo build --release --features web_server
    fi

    # 启动 Web UI
    ./target/release/web_server &
    web_server_PID=$!
    echo -e "${GREEN}✓ Web UI 已启动 (PID: $web_server_PID)${NC}"
}

# 显示访问信息
show_access_info() {
    echo -e "${YELLOW}[3/3] 系统已就绪！${NC}"
    echo ""
    echo "==================================="
    echo -e "${GREEN}访问地址：${NC}"
    echo ""
    echo "  📊 Web UI 控制面板:"
    echo "     http://localhost:8888"
    echo ""
    echo "  🎮 同步控制中心:"
    echo "     http://localhost:8888/sync-control"
    echo ""
    echo "  🔧 远程同步配置:"
    echo "     http://localhost:8888/remote-sync"
    echo ""
    if [ ! -z "$MQTT_PID" ]; then
        echo "  📡 MQTT 服务器:"
        echo "     端口: 1883 (MQTT)"
        echo "     端口: 8080 (WebSocket)"
    fi
    echo ""
    echo "==================================="
    echo ""
    echo -e "${YELLOW}按 Ctrl+C 停止所有服务${NC}"
}

# 清理函数
cleanup() {
    echo ""
    echo -e "${YELLOW}正在停止服务...${NC}"

    if [ ! -z "$web_server_PID" ]; then
        kill $web_server_PID 2>/dev/null || true
        echo -e "${GREEN}✓ Web UI 已停止${NC}"
    fi

    if [ ! -z "$MQTT_PID" ]; then
        kill $MQTT_PID 2>/dev/null || true
        echo -e "${GREEN}✓ MQTT 服务器已停止${NC}"
    fi

    # 停止 mosquitto（如果启动了）
    if command_exists mosquitto; then
        pkill mosquitto 2>/dev/null || true
    fi

    echo -e "${GREEN}所有服务已停止${NC}"
    exit 0
}

# 设置信号处理
trap cleanup INT TERM

# 主流程
main() {
    # 检查是否在项目根目录
    if [ ! -f "Cargo.toml" ]; then
        echo -e "${RED}错误：请在 gen-model 项目根目录下运行此脚本${NC}"
        exit 1
    fi

    # 启动服务
    start_mqtt_server
    sleep 2
    start_web_server
    sleep 2
    show_access_info

    # 等待用户中断
    while true; do
        sleep 1
    done
}

# 运行主流程
main