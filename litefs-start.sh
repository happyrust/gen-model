#!/bin/bash
# LiteFS 启动脚本

set -e

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}=== LiteFS 启动脚本 ===${NC}"

# 检查是否为 root
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}错误: 请使用 root 或 sudo 运行此脚本${NC}"
    exit 1
fi

# 检查 LiteFS 是否已安装
if ! command -v litefs &> /dev/null; then
    echo -e "${RED}错误: 未找到 LiteFS${NC}"
    echo "请先安装 LiteFS:"
    echo "  curl -L https://github.com/superfly/litefs/releases/download/v0.5.11/litefs-v0.5.11-linux-amd64.tar.gz | tar xz -C /usr/local/bin"
    exit 1
fi

# 检查配置文件
NODE_TYPE="${1:-replica}"
if [ "$NODE_TYPE" = "primary" ]; then
    CONFIG_FILE="litefs-primary.yml"
else
    CONFIG_FILE="litefs-replica.yml"
fi

if [ ! -f "$CONFIG_FILE" ]; then
    echo -e "${RED}错误: 配置文件 $CONFIG_FILE 不存在${NC}"
    exit 1
fi

# 创建必要的目录
echo -e "${YELLOW}创建目录...${NC}"
mkdir -p /litefs
mkdir -p /var/lib/litefs

# 设置权限
chown -R $SUDO_USER:$SUDO_USER /var/lib/litefs 2>/dev/null || true

# 检查端口是否被占用
if netstat -tuln | grep -q ":20203 "; then
    echo -e "${YELLOW}警告: 端口 20203 已被占用，LiteFS 可能已在运行${NC}"
    echo "如需重启，请先停止: sudo pkill litefs"
    exit 1
fi

# 复制配置到标准位置
echo -e "${YELLOW}复制配置文件...${NC}"
cp $CONFIG_FILE /etc/litefs.yml

# 提示用户修改配置
if [ "$NODE_TYPE" = "replica" ]; then
    echo -e "${YELLOW}请先修改 /etc/litefs.yml 中的 PRIMARY_IP 为主节点的实际 IP 地址${NC}"
    read -p "是否已修改配置文件? (y/n) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${RED}请先修改配置文件后再运行此脚本${NC}"
        exit 1
    fi
fi

# 启动 LiteFS
echo -e "${GREEN}启动 LiteFS ($NODE_TYPE)...${NC}"
litefs mount &

# 等待挂载完成
echo -e "${YELLOW}等待 LiteFS 挂载...${NC}"
for i in {1..30}; do
    if mountpoint -q /litefs; then
        echo -e "${GREEN}✓ LiteFS 挂载成功${NC}"
        break
    fi
    if [ $i -eq 30 ]; then
        echo -e "${RED}✗ LiteFS 挂载超时${NC}"
        exit 1
    fi
    sleep 1
done

# 检查状态
echo -e "${YELLOW}检查 LiteFS 状态...${NC}"
sleep 2
if curl -s http://localhost:20203/status > /dev/null; then
    echo -e "${GREEN}✓ LiteFS API 可访问${NC}"
    curl -s http://localhost:20203/status | python3 -m json.tool || true
else
    echo -e "${RED}✗ LiteFS API 不可访问${NC}"
fi

echo ""
echo -e "${GREEN}=== LiteFS 启动完成 ===${NC}"
echo ""
echo "LiteFS 信息:"
echo "  - 挂载点: /litefs"
echo "  - 数据目录: /var/lib/litefs"
echo "  - HTTP API: http://localhost:20203"
echo "  - 节点类型: $NODE_TYPE"
echo ""
echo "下一步:"
echo "  1. 修改 DbOption.toml 中的数据库路径为: /litefs/deployment_sites.sqlite"
echo "  2. 启动 web_server 服务: ./target/release/web_server"
echo ""
echo "查看状态: curl http://localhost:20203/status"
echo "停止服务: sudo pkill litefs"