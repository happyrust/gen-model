#!/bin/bash
#将 http 地址 取自环境变量
HTTP_ADDR=${HTTP_ADDR:-http://127.0.0.1:8008}
#database name
DATABASE_NAME=${DATABASE_NAME:-AvevaMarineSample}
#namespace
NAMESPACE=${NAMESPACE:-1516}

surql_files=(
    "common.surql"
)

for file in "${surql_files[@]}"; do
    surreal import --conn $HTTP_ADDR --namespace $NAMESPACE --database $DATABASE_NAME -u root -p root $file
done

#调用子文件夹的 sh 文件
# sh ./gps/run_gps.sh
# 切进到子目录
cd ./gy
sh ./run_gy.sh

cd ../dq
sh ./run_dq.sh

cd ../gy
sh ./run_gy.sh

cd ../yk
sh ./run_yk.sh

cd ../gps
sh ./run_gps.sh
