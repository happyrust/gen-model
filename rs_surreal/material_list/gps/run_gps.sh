#!/bin/bash
#将 http 地址 取自环境变量
HTTP_ADDR=${HTTP_ADDR:-http://127.0.0.1:8008}
#database name
DATABASE_NAME=${DATABASE_NAME:-AvevaMarineSample}
#namespace
NAMESPACE=${NAMESPACE:-1516}

surql_files=(
     "gps_flan.surql"
     "gps_tee.surql"
     "gps_redu.surql"
     "gps_tubi.surql"
     "gps_bend.surql"
     "gps_elbo.surql"
)

for file in "${surql_files[@]}"; do
    surreal import --conn $HTTP_ADDR --namespace $NAMESPACE --database $DATABASE_NAME -u root -p root $file
done
