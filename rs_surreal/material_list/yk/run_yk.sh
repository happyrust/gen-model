#!/bin/bash
#将 http 地址 取自环境变量
HTTP_ADDR=${HTTP_ADDR:-http://127.0.0.1:8008}
#database name
DATABASE_NAME=${DATABASE_NAME:-AvevaMarineSample}
#namespace
NAMESPACE=${NAMESPACE:-1516}

surql_files=(
    "yk_common.surql"
    "yk_sb.surql"
    "yk_ybgd.surql"
    "yk_dzcl.surql"
)

for file in "${surql_files[@]}"; do
    surreal import --conn $HTTP_ADDR --namespace $NAMESPACE --database $DATABASE_NAME -u root -p root $file
done
