#!/bin/bash
# LiteFS 安装脚本

set -e

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${GREEN}╔════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║   LiteFS 自动安装脚本                  ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════╝${NC}"
echo ""

# 检查 root 权限
if [ "$EUID" -ne 0 ]; then
    echo -e "${RED}✗ 错误: 请使用 root 或 sudo 运行此脚本${NC}"
    exit 1
fi

# 检测操作系统
echo -e "${BLUE}→ 检测操作系统...${NC}"
if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    OS="linux"
    echo -e "${GREEN}✓ Linux 系统${NC}"
elif [[ "$OSTYPE" == "darwin"* ]]; then
    OS="darwin"
    echo -e "${GREEN}✓ macOS 系统${NC}"
else
    echo -e "${RED}✗ 不支持的操作系统: $OSTYPE${NC}"
    exit 1
fi

# 检测架构
echo -e "${BLUE}→ 检测系统架构...${NC}"
ARCH=$(uname -m)
case $ARCH in
    x86_64)
        ARCH="amd64"
        ;;
    arm64|aarch64)
        ARCH="arm64"
        ;;
    *)
        echo -e "${RED}✗ 不支持的架构: $ARCH${NC}"
        exit 1
        ;;
esac
echo -e "${GREEN}✓ $ARCH${NC}"

# LiteFS 版本
LITEFS_VERSION="0.5.11"
DOWNLOAD_URL="https://github.com/superfly/litefs/releases/download/v${LITEFS_VERSION}/litefs-v${LITEFS_VERSION}-${OS}-${ARCH}.tar.gz"

# 检查 LiteFS 是否已安装
if command -v litefs &> /dev/null; then
    INSTALLED_VERSION=$(litefs version 2>/dev/null | grep -oP 'v\K[0-9.]+' || echo "unknown")
    echo -e "${YELLOW}⚠ LiteFS 已安装 (版本: $INSTALLED_VERSION)${NC}"
    read -p "是否重新安装? (y/n) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${BLUE}跳过安装${NC}"
        SKIP_INSTALL=true
    fi
fi

# 下载和安装 LiteFS
if [ "$SKIP_INSTALL" != true ]; then
    echo -e "${BLUE}→ 下载 LiteFS v${LITEFS_VERSION}...${NC}"
    cd /tmp
    if curl -fsSL "$DOWNLOAD_URL" -o litefs.tar.gz; then
        echo -e "${GREEN}✓ 下载完成${NC}"
    else
        echo -e "${RED}✗ 下载失败${NC}"
        echo "URL: $DOWNLOAD_URL"
        exit 1
    fi

    echo -e "${BLUE}→ 安装 LiteFS...${NC}"
    tar -xzf litefs.tar.gz -C /usr/local/bin
    chmod +x /usr/local/bin/litefs
    rm litefs.tar.gz
    echo -e "${GREEN}✓ LiteFS 已安装到 /usr/local/bin/litefs${NC}"
fi

# 验证安装
echo -e "${BLUE}→ 验证安装...${NC}"
if litefs version; then
    echo -e "${GREEN}✓ LiteFS 安装成功${NC}"
else
    echo -e "${RED}✗ LiteFS 验证失败${NC}"
    exit 1
fi

# 询问节点类型
echo ""
echo -e "${YELLOW}请选择节点类型:${NC}"
echo "  1) 主节点 (Primary) - 可读写，用于第一台服务器"
echo "  2) 副本节点 (Replica) - 只读，用于其他服务器"
read -p "请输入选择 (1/2): " NODE_CHOICE

if [ "$NODE_CHOICE" = "1" ]; then
    NODE_TYPE="primary"
    CONFIG_FILE="litefs-primary.yml"
    echo -e "${GREEN}→ 配置为主节点${NC}"
elif [ "$NODE_CHOICE" = "2" ]; then
    NODE_TYPE="replica"
    CONFIG_FILE="litefs-replica.yml"
    echo -e "${GREEN}→ 配置为副本节点${NC}"
else
    echo -e "${RED}✗ 无效选择${NC}"
    exit 1
fi

# 复制配置文件
echo -e "${BLUE}→ 配置 LiteFS...${NC}"
if [ ! -f "$CONFIG_FILE" ]; then
    echo -e "${RED}✗ 配置文件 $CONFIG_FILE 不存在${NC}"
    echo "请确保在项目目录下运行此脚本"
    exit 1
fi

cp "$CONFIG_FILE" /etc/litefs.yml
echo -e "${GREEN}✓ 配置文件已复制到 /etc/litefs.yml${NC}"

# 如果是副本节点，提示输入主节点 IP
if [ "$NODE_TYPE" = "replica" ]; then
    echo ""
    echo -e "${YELLOW}请输入主节点 IP 地址:${NC}"
    read -p "主节点 IP: " PRIMARY_IP
    if [ -z "$PRIMARY_IP" ]; then
        echo -e "${RED}✗ 主节点 IP 不能为空${NC}"
        exit 1
    fi
    # 替换配置文件中的 PRIMARY_IP
    sed -i.bak "s/PRIMARY_IP/$PRIMARY_IP/g" /etc/litefs.yml
    echo -e "${GREEN}✓ 主节点 IP 已设置为: $PRIMARY_IP${NC}"
fi

# 创建目录
echo -e "${BLUE}→ 创建目录...${NC}"
mkdir -p /litefs
mkdir -p /var/lib/litefs
echo -e "${GREEN}✓ 目录创建完成${NC}"

# 安装 systemd 服务
echo -e "${BLUE}→ 安装 systemd 服务...${NC}"
if [ -f "litefs.service" ]; then
    cp litefs.service /etc/systemd/system/
    systemctl daemon-reload
    echo -e "${GREEN}✓ systemd 服务已安装${NC}"
else
    echo -e "${YELLOW}⚠ litefs.service 文件不存在，跳过 systemd 配置${NC}"
fi

# 完成
echo ""
echo -e "${GREEN}╔════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║   LiteFS 安装完成！                    ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════╝${NC}"
echo ""
echo -e "${BLUE}下一步操作:${NC}"
echo ""
echo "1. 启动 LiteFS 服务:"
echo -e "   ${YELLOW}sudo systemctl start litefs${NC}"
echo ""
echo "2. 查看 LiteFS 状态:"
echo -e "   ${YELLOW}sudo systemctl status litefs${NC}"
echo -e "   ${YELLOW}curl http://localhost:20203/status${NC}"
echo ""
echo "3. 设置开机自启:"
echo -e "   ${YELLOW}sudo systemctl enable litefs${NC}"
echo ""
echo "4. 修改 DbOption.toml 数据库路径:"
echo -e "   ${YELLOW}deployment_sites_sqlite_path = \"/litefs/deployment_sites.sqlite\"${NC}"
echo ""
echo "5. 启动 web_server 服务"
echo ""
echo -e "${BLUE}管理命令:${NC}"
echo "  - 查看日志: sudo journalctl -u litefs -f"
echo "  - 重启服务: sudo systemctl restart litefs"
echo "  - 停止服务: sudo systemctl stop litefs"
echo ""