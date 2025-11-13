#!/bin/bash
set -e

# AIOS Database - Ubuntu 部署脚本
# 自动部署到 Ubuntu 22.04 LTS 服务器

echo "=========================================="
echo "AIOS Database - Ubuntu 部署"
echo "=========================================="

# 颜色定义
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 配置
SERVER_HOST="101.42.162.129"
SERVER_USER="ubuntu"
SERVER_PASSWORD="Happytest123_"
DEPLOY_PATH="/opt/aios-database"

# 项目根目录
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEPLOY_DIR="$PROJECT_ROOT/deploy"

echo -e "${YELLOW}项目根目录: $PROJECT_ROOT${NC}"
echo -e "${YELLOW}部署目标: ${SERVER_USER}@${SERVER_HOST}:${DEPLOY_PATH}${NC}"

# ==========================================
# 检查部署文件是否存在
# ==========================================
if [ ! -d "$DEPLOY_DIR" ]; then
    echo -e "${RED}✗ 部署文件夹不存在: $DEPLOY_DIR${NC}"
    echo -e "${YELLOW}请先运行编译脚本: ./scripts/build_for_ubuntu.sh${NC}"
    exit 1
fi

# ==========================================
# 使用 sshpass 自动输入密码
# ==========================================
echo -e "\n${YELLOW}[1/7] 检查 SSH 连接工具...${NC}"

if ! command -v sshpass &> /dev/null; then
    echo -e "${YELLOW}sshpass 未安装，正在安装...${NC}"
    if [[ "$OSTYPE" == "darwin"* ]]; then
        # macOS
        brew install hudochenkov/sshpass/sshpass
    else
        # Linux
        sudo apt-get update && sudo apt-get install -y sshpass
    fi
fi

SSH_CMD="sshpass -p '$SERVER_PASSWORD' ssh -o StrictHostKeyChecking=no ${SERVER_USER}@${SERVER_HOST}"
SCP_CMD="sshpass -p '$SERVER_PASSWORD' scp -o StrictHostKeyChecking=no -r"
RSYNC_CMD="sshpass -p '$SERVER_PASSWORD' rsync -avz --progress -e 'ssh -o StrictHostKeyChecking=no'"

# ==========================================
# 测试 SSH 连接
# ==========================================
echo -e "\n${YELLOW}[2/7] 测试服务器连接...${NC}"

if ! eval "$SSH_CMD 'echo Connected'" &> /dev/null; then
    echo -e "${RED}✗ 无法连接到服务器${NC}"
    echo -e "${YELLOW}请检查:${NC}"
    echo -e "  1. 服务器地址: $SERVER_HOST"
    echo -e "  2. 用户名: $SERVER_USER"
    echo -e "  3. 密码是否正确"
    echo -e "  4. 网络连接"
    exit 1
fi

echo -e "${GREEN}✓ 服务器连接成功${NC}"

# ==========================================
# 停止现有服务
# ==========================================
echo -e "\n${YELLOW}[3/7] 停止现有服务...${NC}"

eval "$SSH_CMD" << 'ENDSSH'
# 停止 systemd 服务
if systemctl is-active --quiet aios-web-server; then
    echo "停止后端服务..."
    sudo systemctl stop aios-web-server || true
fi

# 停止 PM2 前端服务
if command -v pm2 &> /dev/null; then
    if pm2 list | grep -q "aios-frontend"; then
        echo "停止前端服务..."
        pm2 stop aios-frontend || true
        pm2 delete aios-frontend || true
    fi
fi

echo "服务已停止"
ENDSSH

echo -e "${GREEN}✓ 现有服务已停止${NC}"

# ==========================================
# 创建部署目录
# ==========================================
echo -e "\n${YELLOW}[4/7] 创建部署目录...${NC}"

eval "$SSH_CMD" << ENDSSH
sudo mkdir -p ${DEPLOY_PATH}/backend
sudo mkdir -p ${DEPLOY_PATH}/frontend
sudo mkdir -p ${DEPLOY_PATH}/logs
sudo chown -R ${SERVER_USER}:${SERVER_USER} ${DEPLOY_PATH}
echo "部署目录已创建"
ENDSSH

echo -e "${GREEN}✓ 部署目录已创建${NC}"

# ==========================================
# 上传文件
# ==========================================
echo -e "\n${YELLOW}[5/7] 上传文件到服务器...${NC}"

# 上传后端
echo -e "${BLUE}上传后端文件...${NC}"
eval "$RSYNC_CMD $DEPLOY_DIR/backend/ ${SERVER_USER}@${SERVER_HOST}:${DEPLOY_PATH}/backend/"

# 上传前端
echo -e "${BLUE}上传前端文件...${NC}"
eval "$RSYNC_CMD $DEPLOY_DIR/frontend/ ${SERVER_USER}@${SERVER_HOST}:${DEPLOY_PATH}/frontend/"

# 上传配置文件
echo -e "${BLUE}上传配置文件...${NC}"
eval "$SCP_CMD $DEPLOY_DIR/aios-web-server.service ${SERVER_USER}@${SERVER_HOST}:${DEPLOY_PATH}/"
eval "$SCP_CMD $DEPLOY_DIR/ecosystem.config.js ${SERVER_USER}@${SERVER_HOST}:${DEPLOY_PATH}/"

echo -e "${GREEN}✓ 文件上传完成${NC}"

# ==========================================
# 在服务器上安装依赖并配置服务
# ==========================================
echo -e "\n${YELLOW}[6/7] 配置服务器环境...${NC}"

eval "$SSH_CMD" << 'ENDSSH'
set -e

echo "设置文件权限..."
chmod +x /opt/aios-database/backend/web_server

echo "配置 systemd 服务..."
sudo cp /opt/aios-database/aios-web-server.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable aios-web-server

echo "检查 Node.js..."
if ! command -v node &> /dev/null; then
    echo "安装 Node.js 18..."
    curl -fsSL https://deb.nodesource.com/setup_18.x | sudo -E bash -
    sudo apt-get install -y nodejs
fi
echo "Node 版本: $(node --version)"
echo "NPM 版本: $(npm --version)"

echo "检查 PM2..."
if ! command -v pm2 &> /dev/null; then
    echo "安装 PM2..."
    sudo npm install -g pm2
    pm2 startup systemd -u ubuntu --hp /home/ubuntu
fi

echo "安装前端依赖..."
cd /opt/aios-database/frontend
npm install --production

echo "配置完成"
ENDSSH

echo -e "${GREEN}✓ 服务器环境配置完成${NC}"

# ==========================================
# 启动服务
# ==========================================
echo -e "\n${YELLOW}[7/7] 启动服务...${NC}"

eval "$SSH_CMD" << 'ENDSSH'
set -e

echo "启动后端服务..."
sudo systemctl start aios-web-server
sudo systemctl status aios-web-server --no-pager || true

echo "启动前端服务..."
cd /opt/aios-database
pm2 start ecosystem.config.js
pm2 save

echo "服务启动完成"
ENDSSH

echo -e "${GREEN}✓ 服务已启动${NC}"

# ==========================================
# 显示部署信息
# ==========================================
echo -e "\n${GREEN}=========================================="
echo -e "部署完成！"
echo -e "==========================================${NC}"

echo -e "\n${BLUE}服务信息:${NC}"
echo -e "  服务器: ${GREEN}http://${SERVER_HOST}${NC}"
echo -e "  前端: ${GREEN}http://${SERVER_HOST}:3000${NC}"
echo -e "  后端 API: ${GREEN}http://${SERVER_HOST}:8080${NC}"

echo -e "\n${BLUE}管理命令:${NC}"
echo -e "  查看后端状态: ${YELLOW}ssh ${SERVER_USER}@${SERVER_HOST} 'sudo systemctl status aios-web-server'${NC}"
echo -e "  查看后端日志: ${YELLOW}ssh ${SERVER_USER}@${SERVER_HOST} 'sudo journalctl -u aios-web-server -f'${NC}"
echo -e "  查看前端状态: ${YELLOW}ssh ${SERVER_USER}@${SERVER_HOST} 'pm2 status'${NC}"
echo -e "  查看前端日志: ${YELLOW}ssh ${SERVER_USER}@${SERVER_HOST} 'pm2 logs aios-frontend'${NC}"

echo -e "\n${BLUE}重启服务:${NC}"
echo -e "  重启后端: ${YELLOW}ssh ${SERVER_USER}@${SERVER_HOST} 'sudo systemctl restart aios-web-server'${NC}"
echo -e "  重启前端: ${YELLOW}ssh ${SERVER_USER}@${SERVER_HOST} 'pm2 restart aios-frontend'${NC}"

# 检查服务状态
echo -e "\n${YELLOW}检查服务状态...${NC}"
sleep 2

echo -e "\n${BLUE}后端服务状态:${NC}"
eval "$SSH_CMD 'sudo systemctl is-active aios-web-server'" || echo -e "${RED}后端服务未运行${NC}"

echo -e "\n${BLUE}前端服务状态:${NC}"
eval "$SSH_CMD 'pm2 list | grep aios-frontend'" || echo -e "${RED}前端服务未运行${NC}"

echo -e "\n${GREEN}部署流程完成！${NC}"

