#!/bin/bash

# 配置参数
DB_BIND="${DB_BIND:-0.0.0.0}"  # 服务器监听地址（默认监听所有接口）
DB_PORT="${DB_PORT:-8009}"      # 服务器监听端口
DB_USER="${DB_USER:-root}"
DB_PASS="${DB_PASS:-root}"
DB_FILE="${DB_FILE:-ams-8009-test.db}"

#!/usr/bin/env bash
 
PORT=$DB_PORT
 
PIDS=$(lsof -ti :"$PORT" 2>/dev/null)
if [ -n "$PIDS" ]; then
  echo "Killing process on port $PORT: $PIDS"
  kill -9 $PIDS
fi

# 健康检查地址（用于检查服务是否启动）
CHECK_HOST="${CHECK_HOST:-127.0.0.1}"  # 本地检查地址
CHECK_PORT="${CHECK_PORT:-$DB_PORT}"   # 检查端口（默认与监听端口相同）

MAX_RETRIES=30  # 最多等待30秒
RETRY_INTERVAL=1  # 每秒检查一次

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# 检查数据库是否已经在运行
check_db_running() {
    curl -s -f "http://${CHECK_HOST}:${CHECK_PORT}/health" > /dev/null 2>&1
    return $?
}

# 启动数据库服务
start_database() {
    echo -e "${YELLOW}正在启动 SurrealDB 服务...${NC}"
    
    # 检查是否已经在运行
    if check_db_running; then
        echo -e "${GREEN}✓ SurrealDB 已经在运行 (${DB_BIND}:${DB_PORT})${NC}"
        return 0
    fi
    
    # 启动数据库
    surreal start --log info \
        --user "$DB_USER" \
        --pass "$DB_PASS" \
        --bind "${DB_BIND}:${DB_PORT}" \
        "file:${DB_FILE}" > surreal.log 2>&1 &
    
    local PID=$!
    echo $PID > .surreal.pid
    
    echo -e "${YELLOW}数据库进程已启动 (PID: $PID)${NC}"
    echo -e "${YELLOW}等待数据库就绪...${NC}"
    
    # 等待数据库启动
    local retry_count=0
    while [ $retry_count -lt $MAX_RETRIES ]; do
        if check_db_running; then
            echo -e "${GREEN}✓ 数据库启动成功！${NC}"
            
            # 创建命名空间和数据库
            setup_namespace_and_db
            return 0
        fi
        
        # 显示进度
        echo -n "."
        sleep $RETRY_INTERVAL
        retry_count=$((retry_count + 1))
    done
    
    echo ""
    echo -e "${RED}✗ 数据库启动超时（${MAX_RETRIES}秒）${NC}"
    
    # 检查日志
    if [ -f surreal.log ]; then
        echo -e "${YELLOW}最近的日志：${NC}"
        tail -n 10 surreal.log
    fi
    
    return 1
}

# 设置命名空间和数据库
setup_namespace_and_db() {
    echo -e "${YELLOW}正在设置命名空间和数据库...${NC}"
    
    # 从配置文件读取项目信息（这里使用默认值作为示例）
    PROJECT_CODE="1516"
    PROJECT_NAME="AvevaMarineSample"
    
    # 创建命名空间和数据库
    echo "DEFINE NAMESPACE \`${PROJECT_CODE}\`; USE NS \`${PROJECT_CODE}\`; DEFINE DATABASE ${PROJECT_NAME};" | \
        surreal sql -e "http://${CHECK_HOST}:${CHECK_PORT}" -u "$DB_USER" -p "$DB_PASS" > /dev/null 2>&1
    
    if [ $? -eq 0 ]; then
        echo -e "${GREEN}✓ 命名空间 '${PROJECT_CODE}' 和数据库 '${PROJECT_NAME}' 已就绪${NC}"
    else
        echo -e "${YELLOW}! 命名空间和数据库可能已存在${NC}"
    fi
}

# 停止数据库
stop_database() {
    echo -e "${YELLOW}正在停止 SurrealDB 服务...${NC}"
    
    if [ -f .surreal.pid ]; then
        PID=$(cat .surreal.pid)
        if kill -0 $PID 2>/dev/null; then
            kill $PID
            echo -e "${GREEN}✓ 数据库已停止 (PID: $PID)${NC}"
            rm .surreal.pid
        else
            echo -e "${YELLOW}! 进程 $PID 不存在${NC}"
            rm .surreal.pid
        fi
    else
        echo -e "${YELLOW}! 未找到 PID 文件${NC}"
    fi
}

# 数据库状态检查
check_status() {
    echo -e "${YELLOW}检查数据库状态...${NC}"
    
    if check_db_running; then
        echo -e "${GREEN}✓ SurrealDB 正在运行${NC}"
        echo -e "  监听地址: ${DB_BIND}:${DB_PORT}"
        echo -e "  可访问地址: http://${CHECK_HOST}:${CHECK_PORT}"
        echo -e "  用户: ${DB_USER}"
        
        # 检查命名空间和数据库
        echo -e "${YELLOW}检查连接...${NC}"
        echo "SELECT 1;" | surreal sql -e "http://${CHECK_HOST}:${CHECK_PORT}" \
            -u "$DB_USER" -p "$DB_PASS" \
            --ns "1516" --db "AvevaMarineSample" > /dev/null 2>&1
        
        if [ $? -eq 0 ]; then
            echo -e "${GREEN}✓ 数据库连接正常${NC}"
        else
            echo -e "${RED}✗ 无法连接到数据库${NC}"
        fi
    else
        echo -e "${RED}✗ SurrealDB 未运行${NC}"
        return 1
    fi
}

# 主函数
main() {
    case "${1:-start}" in
        start)
            start_database
            ;;
        stop)
            stop_database
            ;;
        restart)
            stop_database
            sleep 2
            start_database
            ;;
        status)
            check_status
            ;;
        *)
            echo "用法: $0 {start|stop|restart|status}"
            echo "  start   - 启动数据库服务"
            echo "  stop    - 停止数据库服务"
            echo "  restart - 重启数据库服务"
            echo "  status  - 检查数据库状态"
            exit 1
            ;;
    esac
}

main "$@"