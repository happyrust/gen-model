#!/bin/bash

# ==============================================================================
# 三维校审域 e2e：plant-model-gen shells/test_review_api.sh 移植到 gen-model（:8022）
#
# 用法:
#   ./scripts/review/test_review_api.sh                 # 打 http://localhost:8022，跑业务流
#   ./scripts/review/test_review_api.sh http://host:port
#   ./scripts/review/test_review_api.sh legacy          # 打旧端 http://localhost:3100 作对照
#   ./scripts/review/test_review_api.sh <base> routes   # 只跑「契约路由非 404」探针（计划 §6 A3）
#   ./scripts/review/test_review_api.sh <base> auth     # 只跑 401/403 文本两端对照（计划 §6 A5）
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
                PASSED=$((PASSED + 1))
                return 0
            else
                echo -e "${RED}❌ FAIL${NC} (missing field: $expected_field)"
                FAILED=$((FAILED + 1))
                return 1
            fi
        else
            echo -e "${GREEN}✅ PASS${NC} (HTTP $http_code)"
            PASSED=$((PASSED + 1))
            return 0
        fi
    elif [ "$http_code" = "000" ]; then
        echo -e "${RED}❌ FAIL${NC} (连接失败)"
        FAILED=$((FAILED + 1))
        return 1
    else
        echo -e "${YELLOW}⚠️  WARN${NC} (HTTP $http_code)"
        FAILED=$((FAILED + 1))
        return 1
    fi
}

# 路由存在性探针：断言「这个方法在这条路径上真的注册了」（A3 的 curl 半边；
# 语义正确性由业务流与单测负责）。
#
# 404 / 000 不用说；405 也算失败：它表示路径被别的 handler（比如前端 SPA 的 GET 兜底）
# 接住了、但本方法没注册——只按「非 404」判会把这种情况判成绿。
probe_route() {
    local method="$1"
    local endpoint="$2"
    echo -n "  路由: $method $endpoint ... "
    local output body registered=1
    output=$(curl --max-time 20 -s -w '\n%{http_code}' -X "$method" \
        -H "Content-Type: application/json" "${AUTH_HEADER[@]}" -d '{}' \
        "${BASE_URL}${endpoint}" 2>/dev/null || printf '\n000')
    http_code=$(printf '%s' "$output" | tail -n1)
    body=$(printf '%s' "$output" | sed '$d')
    if [ "$http_code" = '404' ]; then
        printf '%s' "$body" | python -c 'import json,sys; v=json.load(sys.stdin); sys.exit(0 if isinstance(v,dict) and (v.get("success") is False or "error_message" in v) else 1)' 2>/dev/null || registered=0
    fi
    case "$http_code" in
        405|000)
            registered=0
            ;;
    esac
    case "$registered" in
        0)
            echo -e "${RED}❌ FAIL${NC} (HTTP $http_code)"
            FAILED=$((FAILED + 1))
            ;;
        *)
            echo -e "${GREEN}✅ 已注册${NC} (HTTP $http_code)"
            PASSED=$((PASSED + 1))
            ;;
    esac
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
# routes 模式：计划 §1.2「鉴权/用户/任务/记录/评论·批注状态/附件/同步/健康」八行
#
# 逐条数下来是 37 条方法·路径（含前端未调用但旧端一直注册的 returned / batch-reactivate）
# 外加 1 条静态挂载，再加 PMS 6 条与集成 2 条，共 46 个探针；计划 §6 A3 与本脚本旧注释里的「42 条」是计数笔误，
# 清单以 `src/web_service/review/common.rs` 的 `REVIEW_CONTRACT_ROUTES` 护栏为准（两处一一对应）。
# ==================================================
if [ "$MODE" = "routes" ]; then
    echo -e "${YELLOW}📋 契约方法·路径非 404 探针（37 条核心 + 1 条静态 + 6 条 PMS + 2 条集成）${NC}"
    # 鉴权 2
    probe_route POST   /api/auth/token
    probe_route POST   /api/auth/verify
    # 用户 3
    probe_route GET    /api/users
    probe_route GET    /api/users/me
    probe_route GET    /api/users/reviewers
    # 任务 15
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
    if REVIEW_TOKEN="$REVIEW_TOKEN" node "$(dirname "$0")/probe-attachment.mjs" "$BASE_URL"; then
        PASSED=$((PASSED + 1))
    else
        FAILED=$((FAILED + 1))
    fi
    # 同步 2
    probe_route POST   /api/review/sync/export
    probe_route POST   /api/review/sync/import
    # 健康 1
    probe_route GET    /api/review/health
    # PMS 六接口 + 模型侧集成两接口；总计 46。
    probe_route POST   /api/review/workflow/sync
    probe_route POST   /api/review/workflow/verify
    probe_route POST   /api/review/delete
    probe_route POST   /api/review/embed-url
    probe_route POST   /api/review/annotations/check
    probe_route POST   /api/review/cache/preload
    probe_route POST   /api/review/aux-data
    probe_route GET    /api/review/collision-data
    summarize
fi

# ==================================================
# auth 模式：401 / 403 错误文本两端逐字对照（计划 §6 A5）
#
# 旧端出处：jwt_auth.rs:513（缺 Bearer）、:517（token 无效）、review_api.rs:1458（任务写授权）。
# 输出只含「标签 | HTTP 码 | error_message」，不带 ID / 时间戳，两端可直接 diff：
#   ./scripts/review/test_review_api.sh legacy auth > /tmp/a5-legacy.txt
#   ./scripts/review/test_review_api.sh local  auth > /tmp/a5-gen-model.txt
#   diff /tmp/a5-legacy.txt /tmp/a5-gen-model.txt        # 应为空
# 前提：两端都要 review_auth.enabled=true，否则中间件注入调试身份，压根不会回 401。
# ==================================================
error_message_of() {
    echo "$1" | grep -o '"error_message":"[^"]*"' | head -1 | cut -d'"' -f4
}

# expect_error <标签> <期望HTTP码> <期望文本|前缀匹配写 "prefix:xxx"> <方法> <路径> [curl 附加参数...]
expect_error() {
    local label="$1" want_code="$2" want_message="$3" method="$4" endpoint="$5"
    shift 5
    local out code body message
    out=$(curl -s -w "\n%{http_code}" -X "$method" -H "Content-Type: application/json" \
        "$@" -d '{}' "${BASE_URL}${endpoint}" 2>/dev/null || printf '\n000')
    code=$(printf '%s' "$out" | tail -n1)
    body=$(printf '%s' "$out" | sed '$d')
    message=$(error_message_of "$body")

    local ok=0
    case "$want_message" in
        prefix:*) case "$message" in "${want_message#prefix:}"*) ok=1 ;; esac ;;
        *) [ "$message" = "$want_message" ] && ok=1 ;;
    esac
    [ "$code" = "$want_code" ] || ok=0

    echo -n "  $label | HTTP $code | $message ... "
    if [ "$ok" = "1" ]; then
        echo -e "${GREEN}✅${NC}"
        PASSED=$((PASSED + 1))
    else
        echo -e "${RED}❌ FAIL${NC}（期望 HTTP $want_code / ${want_message}）"
        FAILED=$((FAILED + 1))
    fi
}

if [ "$MODE" = "auth" ]; then
    echo -e "${YELLOW}📋 401 / 403 错误文本逐字对照${NC}"

    expect_error "匿名读任务列表" 401 "Missing or invalid Authorization header" \
        GET /api/review/tasks
    expect_error "匿名传附件" 401 "Missing or invalid Authorization header" \
        POST /api/review/attachments
    expect_error "非 Bearer 头" 401 "Missing or invalid Authorization header" \
        GET /api/review/tasks -H "Authorization: Basic dXNlcjpwYXNz"
    expect_error "坏 token" 401 "prefix:Invalid token: " \
        GET /api/review/tasks -H "Authorization: Bearer not-a-jwt"

    # 403：本域没有把 jwt_auth::require_roles 挂到任何路由上（两端都没有），
    # 浏览器侧真正会撞到的角色不足是任务写授权 authorize_task_write（旧端 review_api.rs:1458）。
    if [ -n "$REVIEW_TOKEN" ]; then
        created=$(curl -s -X POST -H "Content-Type: application/json" "${AUTH_HEADER[@]}" \
            -d '{"title":"A5 授权探针","description":"auth mode probe","modelName":"probe","priority":"low"}' \
            "${BASE_URL}/api/review/tasks" 2>/dev/null || echo "")
        probe_task=$(echo "$created" | grep -o '"id":"[^"]*"' | head -1 | cut -d'"' -f4)
        other_token=$(curl -s -X POST -H "Content-Type: application/json" \
            -d '{"project":"debug-project","username":"outsider_999","name":"局外人","role":"sj"}' \
            "${BASE_URL}/api/auth/token" 2>/dev/null | grep -o '"token":"[^"]*"' | head -1 | cut -d'"' -f4)
        if [ -n "$probe_task" ] && [ -n "$other_token" ]; then
            expect_error "他人改任务" 403 \
                "prefix:权限不足：用户 outsider_999 " \
                PATCH "/api/review/tasks/${probe_task}" -H "Authorization: Bearer ${other_token}"
            curl -s -o /dev/null -X DELETE "${AUTH_HEADER[@]}" \
                "${BASE_URL}/api/review/tasks/${probe_task}" 2>/dev/null || true
        else
            echo -e "  他人改任务 ... ${YELLOW}跳过${NC}（建探针任务或换第二个 token 失败）"
            FAILED=$((FAILED + 1))
        fi
    else
        echo -e "  他人改任务 ... ${YELLOW}跳过${NC}（没有 Bearer；review_auth.enabled=true 才跑得了）"
        FAILED=$((FAILED + 1))
    fi

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
        PASSED=$((PASSED + 1))
    else
        echo -e "${YELLOW}⚠️  PASS${NC} (无法提取 ID)"
        PASSED=$((PASSED + 1))
    fi
else
    echo -e "${RED}❌ FAIL${NC} (HTTP $http_code)"
    FAILED=$((FAILED + 1))
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
        PASSED=$((PASSED + 1))
    else
        echo -e "${RED}❌ FAIL${NC} (HTTP $http_code)"
        FAILED=$((FAILED + 1))
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
    PASSED=$((PASSED + 1))
else
    echo -e "${RED}❌ FAIL${NC} (HTTP $http_code)"
    FAILED=$((FAILED + 1))
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
