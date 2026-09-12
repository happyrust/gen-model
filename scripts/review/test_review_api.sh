#!/bin/bash

# ==============================================================================
# 三维校审域 e2e：plant-model-gen shells/test_review_api.sh 移植到 gen-model（:8022）
#
# 用法:
#   ./scripts/review/test_review_api.sh                 # 打 http://localhost:8022，跑业务流
#   ./scripts/review/test_review_api.sh http://host:port
#   ./scripts/review/test_review_api.sh legacy          # 打旧端 http://localhost:3100 作对照
#   ./scripts/review/test_review_api.sh <base> routes   # 只跑「42 条路由非 404」探针（计划 §6 A3）
#
# 环境变量:
#   REVIEW_TOKEN   直接给 Bearer；不给则先 POST /api/auth/token（debug 角色 sj）换一个。
#                  review_auth.enabled=false 时服务端会注入调试身份，没有 token 也能通。
#   REVIEW_ROLE    换 token 时的角色，默认 sj。
#
# 与旧脚本的差异：旧脚本默认 :8080 且不带 Authorization；这里默认 :8022、自动换 token，
# 断言方式（HTTP 2xx + 响应体含字段名）逐字沿用，便于两端对照。
# 计划：docs/plans/2026-09-12-review-backend-port-plan.md §6 A3 / A4 / A5。
# ==============================================================================

set -e

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# 默认地址
LOCAL_URL="http://localhost:8022"
LEGACY_URL="http://localhost:3100"

# 解析参数
case "${1:-local}" in
    local)
        BASE_URL="$LOCAL_URL"
        ;;
    legacy)
        BASE_URL="$LEGACY_URL"
        ;;
    *)
        BASE_URL="$1"
        ;;
esac
MODE="${2:-flow}"

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}   Review API 测试${NC}"
echo -e "${BLUE}   目标: ${BASE_URL}  模式: ${MODE}${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# 统计
PASSED=0
FAILED=0

# ==================================================
# 0. 鉴权：拿一个 Bearer（review_auth.enabled=true 时必需）
# ==================================================
if [ -z "$REVIEW_TOKEN" ]; then
    token_body=$(curl -s -X POST -H "Content-Type: application/json" \
        -d '{"project":"debug-project","username":"designer_001","name":"设计师","role":"'"${REVIEW_ROLE:-sj}"'"}' \
        "${BASE_URL}/api/auth/token" 2>/dev/null || echo "")
    REVIEW_TOKEN=$(echo "$token_body" | grep -o '"token":"[^"]*"' | head -1 | cut -d'"' -f4)
    if [ -n "$REVIEW_TOKEN" ]; then
        echo -e "  鉴权: ${GREEN}已从 /api/auth/token 换取 Bearer${NC}"
    else
        echo -e "  鉴权: ${YELLOW}未取到 token（review_auth.enabled=false 时可忽略）${NC}"
    fi
fi
AUTH_HEADER=()
if [ -n "$REVIEW_TOKEN" ]; then
    AUTH_HEADER=(-H "Authorization: Bearer ${REVIEW_TOKEN}")
fi
echo ""

# 测试函数
test_api() {
    local method="$1"
    local endpoint="$2"
    local data="$3"
    local expected_field="$4"
    local description="$5"

    echo -n "  测试: $description ... "

    if [ "$method" = "GET" ]; then
        response=$(curl -s -w "\n%{http_code}" "${AUTH_HEADER[@]}" "${BASE_URL}${endpoint}" 2>/dev/null || echo -e "\n000")
    else
        response=$(curl -s -w "\n%{http_code}" -X "$method" \
            -H "Content-Type: application/json" "${AUTH_HEADER[@]}" \
            -d "$data" \
            "${BASE_URL}${endpoint}" 2>/dev/null || echo -e "\n000")
    fi

    http_code=$(echo "$response" | tail -n1)
    body=$(echo "$response" | sed '$d')

    if [ "$http_code" = "200" ] || [ "$http_code" = "201" ]; then
        if [ -n "$expected_field" ]; then
            if echo "$body" | grep -q "\"$expected_field\""; then
                echo -e "${GREEN}✅ PASS${NC} (HTTP $http_code)"
                ((PASSED++))
                return 0
            else
                echo -e "${RED}❌ FAIL${NC} (missing field: $expected_field)"
                ((FAILED++))
                return 1
            fi
        else
            echo -e "${GREEN}✅ PASS${NC} (HTTP $http_code)"
            ((PASSED++))
            return 0
        fi
    elif [ "$http_code" = "000" ]; then
        echo -e "${RED}❌ FAIL${NC} (连接失败)"
        ((FAILED++))
        return 1
    else
        echo -e "${YELLOW}⚠️  WARN${NC} (HTTP $http_code)"
        ((FAILED++))
        return 1
    fi
}

# 路由存在性探针：只断言「不是 404」（A3 的 curl 半边；语义正确性由业务流与单测负责）
probe_route() {
    local method="$1"
    local endpoint="$2"
    echo -n "  路由: $method $endpoint ... "
    http_code=$(curl -s -o /dev/null -w "%{http_code}" -X "$method" \
        -H "Content-Type: application/json" "${AUTH_HEADER[@]}" -d '{}' \
        "${BASE_URL}${endpoint}" 2>/dev/null || echo "000")
    if [ "$http_code" = "404" ] || [ "$http_code" = "000" ]; then
        echo -e "${RED}❌ FAIL${NC} (HTTP $http_code)"
        ((FAILED++))
    else
        echo -e "${GREEN}✅ 非404${NC} (HTTP $http_code)"
        ((PASSED++))
    fi
}

summarize() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}   测试结果汇总${NC}"
    echo -e "${BLUE}========================================${NC}"
    echo -e "  通过: ${GREEN}$PASSED${NC}"
    echo -e "  失败: ${RED}$FAILED${NC}"
    TOTAL=$((PASSED + FAILED))
    echo -e "  总计: $TOTAL"
    echo ""

    if [ $FAILED -eq 0 ]; then
        echo -e "${GREEN}🎉 全部测试通过！${NC}"
        exit 0
    else
        echo -e "${YELLOW}⚠️ 部分测试失败，请检查服务器状态${NC}"
        exit 1
    fi
}

# ==================================================
# routes 模式：计划 §1.2「鉴权/用户/任务/记录/评论·批注状态/附件/同步/健康」八行 42 条
# ==================================================
if [ "$MODE" = "routes" ]; then
    echo -e "${YELLOW}📋 42 条方法·路径非 404 探针${NC}"
    # 鉴权 2
    probe_route POST   /api/auth/token
    probe_route POST   /api/auth/verify
    # 用户 3
    probe_route GET    /api/users
    probe_route GET    /api/users/me
    probe_route GET    /api/users/reviewers
    # 任务 16
    probe_route POST   /api/review/tasks
    probe_route GET    /api/review/tasks
    probe_route GET    /api/review/tasks/probe-id
    probe_route PATCH  /api/review/tasks/probe-id
    probe_route DELETE /api/review/tasks/probe-id
    probe_route POST   /api/review/tasks/probe-id/start-review
    probe_route POST   /api/review/tasks/probe-id/approve
    probe_route POST   /api/review/tasks/probe-id/reject
    probe_route POST   /api/review/tasks/probe-id/cancel
    probe_route POST   /api/review/tasks/probe-id/submit
    probe_route POST   /api/review/tasks/probe-id/return
    probe_route GET    /api/review/tasks/probe-id/history
    probe_route GET    /api/review/tasks/probe-id/workflow
    probe_route GET    /api/review/tasks/returned
    probe_route POST   /api/review/tasks/batch-reactivate
    # 记录 4
    probe_route POST   /api/review/records
    probe_route GET    /api/review/records/by-task/probe-id
    probe_route DELETE /api/review/records/item/probe-id
    probe_route DELETE /api/review/records/clear-task/probe-id
    # 评论 / 批注状态 8
    probe_route POST   /api/review/comments
    probe_route GET    "/api/review/comments/by-annotation/probe-id?type=text"
    probe_route DELETE /api/review/comments/item/probe-id
    probe_route PATCH  /api/review/comments/item/probe-id
    probe_route PATCH  /api/review/annotations/probe-id
    probe_route PATCH  /api/review/annotations/probe-id/severity
    probe_route POST   /api/review/annotation-states/apply
    probe_route GET    "/api/review/annotation-states?formId=probe&taskId=probe"
    # 附件 3（静态文件由 /files/review_attachments 挂载，缺文件时 404 属正常，这里只探目录前缀是否被路由接住）
    probe_route POST   /api/review/attachments
    probe_route DELETE /api/review/attachments/probe-id
    echo -n "  静态: GET /files/review_attachments/ ... "
    http_code=$(curl -s -o /dev/null -w "%{http_code}" "${BASE_URL}/files/review_attachments/__probe__.bin" 2>/dev/null || echo "000")
    if [ "$http_code" = "000" ]; then echo -e "${RED}❌ FAIL${NC} (连接失败)"; ((FAILED++)); else echo -e "${GREEN}✅ 已应答${NC} (HTTP $http_code，缺文件 404 属正常)"; ((PASSED++)); fi
    # 同步 2
    probe_route POST   /api/review/sync/export
    probe_route POST   /api/review/sync/import
    # 健康 1
    probe_route GET    /api/review/health
    summarize
fi

# 保存创建的资源 ID
TASK_ID=""
RECORD_ID=""
COMMENT_ID=""

# ==================================================
# 1. 用户 API 测试
# ==================================================
echo -e "${YELLOW}📋 1. 用户 API${NC}"

test_api "GET" "/api/users" "" "success" "获取用户列表"
test_api "GET" "/api/users/me" "" "user" "获取当前用户"
test_api "GET" "/api/users/reviewers" "" "users" "获取审核人员"

echo ""

# ==================================================
# 2. 提资单 API 测试
# ==================================================
echo -e "${YELLOW}📋 2. 提资单 API${NC}"

# 创建提资单
echo -n "  测试: 创建提资单 ... "
response=$(curl -s -w "\n%{http_code}" -X POST \
    -H "Content-Type: application/json" "${AUTH_HEADER[@]}" \
    -d '{
        "title": "测试提资单-'$(date +%s)'",
        "modelName": "test-model",
        "reviewerId": "user-002",
        "priority": "medium",
        "components": [{"id": "c1", "name": "/TEST", "refNo": "123_456", "type": "pipe"}]
    }' \
    "${BASE_URL}/api/review/tasks" 2>/dev/null || echo -e "\n000")

http_code=$(echo "$response" | tail -n1)
body=$(echo "$response" | sed '$d')

if [ "$http_code" = "200" ]; then
    TASK_ID=$(echo "$body" | grep -o '"id":"[^"]*"' | head -1 | cut -d'"' -f4)
    if [ -n "$TASK_ID" ]; then
        echo -e "${GREEN}✅ PASS${NC} (ID: $TASK_ID)"
        ((PASSED++))
    else
        echo -e "${YELLOW}⚠️  PASS${NC} (无法提取 ID)"
        ((PASSED++))
    fi
else
    echo -e "${RED}❌ FAIL${NC} (HTTP $http_code)"
    ((FAILED++))
fi

test_api "GET" "/api/review/tasks" "" "tasks" "获取任务列表"

if [ -n "$TASK_ID" ]; then
    test_api "GET" "/api/review/tasks/$TASK_ID" "" "task" "获取任务详情"
    test_api "PATCH" "/api/review/tasks/$TASK_ID" '{"title":"更新后的标题"}' "success" "更新任务"
    test_api "POST" "/api/review/tasks/$TASK_ID/start-review" "" "success" "开始审核"
    test_api "GET" "/api/review/tasks/$TASK_ID/history" "" "history" "获取审核历史"
    test_api "GET" "/api/review/tasks/$TASK_ID/workflow" "" "success" "获取工作流历史"
fi

echo ""

# ==================================================
# 3. 确认记录 API 测试
# ==================================================
echo -e "${YELLOW}📋 3. 确认记录 API${NC}"

if [ -n "$TASK_ID" ]; then
    # 创建确认记录
    echo -n "  测试: 创建确认记录 ... "
    response=$(curl -s -w "\n%{http_code}" -X POST \
        -H "Content-Type: application/json" "${AUTH_HEADER[@]}" \
        -d '{
            "taskId": "'$TASK_ID'",
            "type": "batch",
            "annotations": [{"id": "a1", "text": "测试批注"}],
            "cloudAnnotations": [],
            "rectAnnotations": [],
            "obbAnnotations": [],
            "measurements": [],
            "note": "测试备注"
        }' \
        "${BASE_URL}/api/review/records" 2>/dev/null || echo -e "\n000")

    http_code=$(echo "$response" | tail -n1)
    body=$(echo "$response" | sed '$d')

    if [ "$http_code" = "200" ]; then
        RECORD_ID=$(echo "$body" | grep -o '"id":"[^"]*"' | head -1 | cut -d'"' -f4)
        echo -e "${GREEN}✅ PASS${NC} (ID: $RECORD_ID)"
        ((PASSED++))
    else
        echo -e "${RED}❌ FAIL${NC} (HTTP $http_code)"
        ((FAILED++))
    fi

    test_api "GET" "/api/review/records/by-task/$TASK_ID" "" "records" "获取任务记录"
fi

echo ""

# ==================================================
# 4. 评论 API 测试
# ==================================================
echo -e "${YELLOW}📋 4. 评论 API${NC}"

# 创建评论
echo -n "  测试: 创建评论 ... "
response=$(curl -s -w "\n%{http_code}" -X POST \
    -H "Content-Type: application/json" "${AUTH_HEADER[@]}" \
    -d '{
        "annotationId": "anno-test-1",
        "annotationType": "text",
        "authorId": "user-001",
        "authorName": "测试用户",
        "authorRole": "designer",
        "content": "这是一条测试评论"
    }' \
    "${BASE_URL}/api/review/comments" 2>/dev/null || echo -e "\n000")

http_code=$(echo "$response" | tail -n1)
body=$(echo "$response" | sed '$d')

if [ "$http_code" = "200" ]; then
    COMMENT_ID=$(echo "$body" | grep -o '"id":"[^"]*"' | head -1 | cut -d'"' -f4)
    echo -e "${GREEN}✅ PASS${NC} (ID: $COMMENT_ID)"
    ((PASSED++))
else
    echo -e "${RED}❌ FAIL${NC} (HTTP $http_code)"
    ((FAILED++))
fi

test_api "GET" "/api/review/comments/by-annotation/anno-test-1?type=text" "" "comments" "获取评论"

echo ""

# ==================================================
# 5. 同步 / 健康
# ==================================================
echo -e "${YELLOW}📋 5. 同步与健康${NC}"

if [ -n "$TASK_ID" ]; then
    test_api "POST" "/api/review/sync/export" '{"taskIds":["'$TASK_ID'"]}' "success" "导出校审数据"
fi
test_api "GET" "/api/review/health" "" "status" "校审健康探针"

echo ""

# ==================================================
# 6. 清理测试数据
# ==================================================
echo -e "${YELLOW}📋 6. 清理测试数据${NC}"

if [ -n "$COMMENT_ID" ]; then
    test_api "DELETE" "/api/review/comments/item/$COMMENT_ID" "" "success" "删除评论"
fi

if [ -n "$RECORD_ID" ]; then
    test_api "DELETE" "/api/review/records/item/$RECORD_ID" "" "success" "删除记录"
fi

if [ -n "$TASK_ID" ]; then
    test_api "DELETE" "/api/review/tasks/$TASK_ID" "" "success" "删除任务"
fi

summarize
