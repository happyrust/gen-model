#!/bin/bash

# 启动内存KV数据库实例 (端口 8011)
# 用于额外备份 PE 数据

DB_PORT=8011
DB_USER="root"
DB_PASS="root"
DB_FILE="surreal-8011.db"

echo "🚀 正在启动内存KV数据库实例..."
echo "   端口: $DB_PORT"
echo "   用户: $DB_USER"
echo "   数据文件: $DB_FILE"

surreal start \
    --log info \
    --user "$DB_USER" \
    --pass "$DB_PASS" \
    --bind "0.0.0.0:$DB_PORT" \
    "file:$DB_FILE"

