#!/usr/bin/env bash
# ============================================================================
# 校审工作流端到端测试脚本
# 覆盖：embed-url → create task → submit 流转 → workflow/sync → return → delete
# 说明：
#   - PMS 入站接口统一走 platform_auth
#   - 浏览器侧 /api/review/* 仍走 review_auth + Bearer JWT
#   - 若未显式提供 PLATFORM_TOKEN，会尝试调用 /api/auth/token 生成一个仅用于本地联调的验签 JWT
#
# 来源：plant-model-gen shells/test_workflow_e2e.sh 逐字搬入 gen-model；唯一改动是默认地址由旧端
#   :3100 改为本服务 :8022（SERVER_URL 可覆盖，打旧端作对照时传 SERVER_URL=http://localhost:3100）。
#   计划：docs/plans/2026-09-12-review-backend-port-plan.md §6 A8。
# ============================================================================
set -uo pipefail
PYTHON="${PYTHON:-python}"
"$PYTHON" --version >/dev/null 2>&1 || { echo 'Python runtime unavailable'; exit 2; }

SERVER="${SERVER_URL:-http://localhost:8022}"
PROJECT_ID="${PROJECT_ID:-AvevaMarineSample}"
PLATFORM_AUTH_ENABLED="${PLATFORM_AUTH_ENABLED:-auto}"
PLATFORM_DEBUG_TOKEN="${PLATFORM_DEBUG_TOKEN:-}"
PLATFORM_TOKEN="${PLATFORM_TOKEN:-}"
EXPECTED_INVALID_TOKEN_STATUS="${EXPECTED_INVALID_TOKEN_STATUS:-401}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m'

PASSED=0
FAILED=0
TOTAL=0

assert_eq() {
    TOTAL=$((TOTAL + 1))
    if [ "$2" = "$3" ]; then
        echo -e "  ${GREEN}✓${NC} $1 (=$2)"
        PASSED=$((PASSED + 1))
    else
        echo -e "  ${RED}✗${NC} $1 (expected=$2, got=$3)"
        FAILED=$((FAILED + 1))
    fi
}

assert_contains() {
    TOTAL=$((TOTAL + 1))
    if echo "$2" | grep -q "$3"; then
        echo -e "  ${GREEN}✓${NC} $1"
        PASSED=$((PASSED + 1))
    else
        echo -e "  ${RED}✗${NC} $1 (missing '$3')"
        FAILED=$((FAILED + 1))
    fi
}

assert_not_empty() {
    TOTAL=$((TOTAL + 1))
    if [ -n "$2" ] && [ "$2" != "null" ] && [ "$2" != "None" ] && [ "$2" != "" ]; then
        echo -e "  ${GREEN}✓${NC} $1 (non-empty)"
        PASSED=$((PASSED + 1))
    else
        echo -e "  ${RED}✗${NC} $1 (empty)"
        FAILED=$((FAILED + 1))
    fi
}

jf() {
    "$PYTHON" -c "
import sys, json
d = json.load(sys.stdin)
path = sys.argv[1].split('.')
v = d
for k in path:
    if k == '': continue
    if isinstance(v, list): v = v[int(k)]
    elif isinstance(v, dict): v = v.get(k, '')
    else: v = ''; break
if v is None: v = ''
print(v)
" "$2" <<< "$1" 2>/dev/null || echo ""
}

jlen() {
    "$PYTHON" -c "
import sys, json
d = json.load(sys.stdin)
path = sys.argv[1].split('.')
v = d
for k in path:
    if k == '': continue
    if isinstance(v, list): v = v[int(k)]
    elif isinstance(v, dict): v = v.get(k, [])
    else: v = []; break
print(len(v) if isinstance(v, list) else 0)
" "$2" <<< "$1" 2>/dev/null || echo "0"
}

post() {
    curl -s -X POST "${SERVER}${1}" -H "Content-Type: application/json" "${@:3}" -d "$2" 2>/dev/null
}

post_status() {
    curl -s -o /dev/null -w '%{http_code}' -X POST "${SERVER}${1}" -H "Content-Type: application/json" "${@:3}" -d "$2" 2>/dev/null
}

echo -e "${BLUE}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║          校审工作流 E2E 测试                             ║${NC}"
echo -e "${BLUE}║          ${SERVER}                  ║${NC}"
echo -e "${BLUE}╚══════════════════════════════════════════════════════════╝${NC}"
echo "  platform_auth=$PLATFORM_AUTH_ENABLED, expected_invalid_token_status=$EXPECTED_INVALID_TOKEN_STATUS"
if [ -z "$PLATFORM_TOKEN" ] && [ -n "$PLATFORM_DEBUG_TOKEN" ]; then
    PLATFORM_TOKEN="$PLATFORM_DEBUG_TOKEN"
fi
if [ -z "$PLATFORM_TOKEN" ]; then
    echo "  platform_token 未显式提供，尝试调用 /api/auth/token 生成本地联调 JWT"
    PLATFORM_SEED=$(post "/api/auth/token" "{\"user_id\":\"SJ\",\"user_name\":\"设计\",\"project_id\":\"${PROJECT_ID}\",\"role\":\"sj\"}")
    PLATFORM_TOKEN="$(jf "$PLATFORM_SEED" "data.token")"
fi
if [ -z "$PLATFORM_TOKEN" ] || [ "$PLATFORM_TOKEN" = "null" ]; then
    echo -e "${RED}缺少可用的 PLATFORM_TOKEN；请设置 PLATFORM_TOKEN 或 PLATFORM_DEBUG_TOKEN${NC}"
    exit 2
fi
echo ""

# ============================================================================
# 1. embed-url: 获取新 form_id + JWT
# ============================================================================
echo -e "${YELLOW}▸ 1. embed-url: 生成新 form_id${NC}"

EMBED=$(post "/api/review/embed-url" "{\"project_id\":\"${PROJECT_ID}\",\"user_id\":\"SJ\",\"workflow_role\":\"sj\",\"token\":\"${PLATFORM_TOKEN}\"}")
EMBED_CODE=$(jf "$EMBED" "code")
FORM_ID=$(jf "$EMBED" "data.query.form_id")
JWT_SJ=$(jf "$EMBED" "data.token")
URL=$(jf "$EMBED" "url")

assert_eq "embed-url code=200" "200" "$EMBED_CODE"
assert_not_empty "form_id 已生成" "$FORM_ID"
assert_not_empty "JWT token 已生成" "$JWT_SJ"
assert_contains "URL 包含 user_token" "$URL" "user_token="
assert_eq "embed form.exists=true" "True" "$(jf "$EMBED" "data.form.exists")"
assert_eq "embed form.status=blank" "blank" "$(jf "$EMBED" "data.form.status")"
assert_eq "embed form.task_created=false" "False" "$(jf "$EMBED" "data.form.task_created")"

LIN_TASK=$(jf "$EMBED" "data.lineage.task_id")
assert_eq "lineage 无已有 task" "" "$LIN_TASK"

echo "  → form_id=$FORM_ID"
echo ""

# ============================================================================
# 2. workflow/sync query: 空 form 返回空数据
# ============================================================================
echo -e "${YELLOW}▸ 2. workflow/sync query: 新 form 无关联数据${NC}"

SYNC=$(post "/api/review/workflow/sync" "{
    \"form_id\":\"${FORM_ID}\",\"token\":\"${PLATFORM_TOKEN}\",
    \"action\":\"query\",\"actor\":{\"id\":\"SJ\",\"name\":\"设计\",\"roles\":\"sj\"}}")
SYNC_CODE=$(jf "$SYNC" "code")
MODELS_N=$(jlen "$SYNC" "data.models")

assert_eq "sync code=200" "200" "$SYNC_CODE"
assert_eq "form_exists=true" "True" "$(jf "$SYNC" "data.form_exists")"
assert_eq "form_status=blank" "blank" "$(jf "$SYNC" "data.form_status")"
assert_eq "task_created=false" "False" "$(jf "$SYNC" "data.task_created")"
assert_eq "无关联模型" "0" "$MODELS_N"
echo ""

# ============================================================================
# 3. Token 校验: 错误 token 状态码断言（可配置）
# ============================================================================
echo -e "${YELLOW}▸ 3. Token 校验: 错误 token → ${EXPECTED_INVALID_TOKEN_STATUS}${NC}"

BAD_STATUS=$(post_status "/api/review/workflow/sync" "{
    \"form_id\":\"${FORM_ID}\",\"token\":\"invalid-token\",
    \"action\":\"query\",\"actor\":{\"id\":\"SJ\",\"name\":\"设计\",\"roles\":\"sj\"}}")
assert_eq "错误 token → ${EXPECTED_INVALID_TOKEN_STATUS}" "${EXPECTED_INVALID_TOKEN_STATUS}" "$BAD_STATUS"
echo ""

# ============================================================================
# 4. create task: 创建提资单并关联模型
# ============================================================================
echo -e "${YELLOW}▸ 4. 创建提资单 (SJ)${NC}"

CREATE=$(post "/api/review/tasks" "{
    \"title\":\"E2E-管道校审-$(date +%H%M%S)\",\"description\":\"E2E自动测试\",
    \"modelName\":\"${PROJECT_ID}\",\"checkerId\":\"JH\",\"approverId\":\"SH\",\"reviewerId\":\"JH\",
    \"formId\":\"${FORM_ID}\",\"priority\":\"medium\",
    \"components\":[
        {\"id\":\"c1\",\"refNo\":\"24381_145018\",\"name\":\"管道A\",\"type\":\"PIPE\"},
        {\"id\":\"c2\",\"refNo\":\"24381_145020\",\"name\":\"阀门B\",\"type\":\"VALVE\"},
        {\"id\":\"c3\",\"refNo\":\"24381_145022\",\"name\":\"支撑C\",\"type\":\"STRU\"}]}" \
    -H "Authorization: Bearer $JWT_SJ")

CREATE_OK=$(jf "$CREATE" "success")
TASK_ID=$(jf "$CREATE" "task.id")
TASK_STATUS=$(jf "$CREATE" "task.status")
TASK_NODE=$(jf "$CREATE" "task.currentNode")
TASK_FORM=$(jf "$CREATE" "task.formId")

assert_eq "创建成功" "True" "$CREATE_OK"
assert_not_empty "task_id" "$TASK_ID"
assert_eq "状态=draft" "draft" "$TASK_STATUS"
assert_eq "节点=sj" "sj" "$TASK_NODE"
assert_eq "form_id 一致" "$FORM_ID" "$TASK_FORM"
echo "  → task_id=$TASK_ID"
echo ""

# ============================================================================
# 5. workflow/sync query: 创建后有 models
# ============================================================================
echo -e "${YELLOW}▸ 5. workflow/sync query: 创建后有模型数据${NC}"

SYNC2=$(post "/api/review/workflow/sync" "{
    \"form_id\":\"${FORM_ID}\",\"token\":\"${PLATFORM_TOKEN}\",
    \"action\":\"query\",\"actor\":{\"id\":\"SJ\",\"name\":\"设计\",\"roles\":\"sj\"}}")
MODELS_N2=$(jlen "$SYNC2" "data.models")
MODELS_STR=$(jf "$SYNC2" "data.models")

assert_eq "关联 3 个模型" "3" "$MODELS_N2"
assert_eq "form_status=draft" "draft" "$(jf "$SYNC2" "data.form_status")"
assert_eq "task_created=true" "True" "$(jf "$SYNC2" "data.task_created")"
assert_contains "包含 145018" "$MODELS_STR" "24381_145018"
assert_contains "包含 145020" "$MODELS_STR" "24381_145020"
echo ""

# ============================================================================
# 6. embed-url lineage: 应包含 task 信息
# ============================================================================
echo -e "${YELLOW}▸ 6. embed-url lineage: task 已关联${NC}"

EMBED2=$(post "/api/review/embed-url" "{\"project_id\":\"${PROJECT_ID}\",\"user_id\":\"SJ\",\"workflow_role\":\"sj\",\"form_id\":\"${FORM_ID}\",\"token\":\"${PLATFORM_TOKEN}\"}")
LIN2_TASK=$(jf "$EMBED2" "data.lineage.task_id")
LIN2_NODE=$(jf "$EMBED2" "data.lineage.current_node")
LIN2_STATUS=$(jf "$EMBED2" "data.lineage.status")

assert_eq "lineage task_id" "$TASK_ID" "$LIN2_TASK"
assert_eq "lineage node=sj" "sj" "$LIN2_NODE"
assert_eq "lineage status=draft" "draft" "$LIN2_STATUS"
assert_eq "embed2 form.status=draft" "draft" "$(jf "$EMBED2" "data.form.status")"
assert_eq "embed2 form.task_created=true" "True" "$(jf "$EMBED2" "data.form.task_created")"
echo ""

# ============================================================================
# 7. workflow/sync active：SJ 激活到 JD
# ============================================================================
echo -e "${YELLOW}▸ 7. workflow/sync active：SJ 激活到 JD${NC}"

SYNC3=$(post "/api/review/workflow/sync" "{
    \"form_id\":\"${FORM_ID}\",\"token\":\"${PLATFORM_TOKEN}\",
    \"action\":\"active\",
    \"actor\":{\"id\":\"debug-user\",\"name\":\"debug-user\",\"roles\":\"sj\"},
    \"next_step\":{\"assignee_id\":\"JH\",\"name\":\"校对\",\"roles\":\"jd\"},
    \"comments\":\"E2E-设计完成\"}")
SYNC3_CODE=$(jf "$SYNC3" "code")

assert_eq "active code=200" "200" "$SYNC3_CODE"
assert_eq "active 后 current_node=jd" "jd" "$(jf "$SYNC3" "data.current_node")"
assert_eq "active 后 task_status=submitted" "submitted" "$(jf "$SYNC3" "data.task_status")"
echo ""

# ============================================================================
# 8. workflow/sync agree：JD 同意到 SH
# ============================================================================
echo -e "${YELLOW}▸ 8. workflow/sync agree：JD 同意到 SH${NC}"

SYNC4=$(post "/api/review/workflow/sync" "{
    \"form_id\":\"${FORM_ID}\",\"token\":\"${PLATFORM_TOKEN}\",
    \"action\":\"agree\",
    \"actor\":{\"id\":\"JH\",\"name\":\"校对\",\"roles\":\"jd\"},
    \"next_step\":{\"assignee_id\":\"SH\",\"name\":\"审核\",\"roles\":\"sh\"},
    \"comments\":\"E2E-校对通过\"}")
assert_eq "agree code=200" "200" "$(jf "$SYNC4" "code")"
assert_eq "agree 后 current_node=sh" "sh" "$(jf "$SYNC4" "data.current_node")"
assert_eq "agree 后 task_status=in_review" "in_review" "$(jf "$SYNC4" "data.task_status")"
echo ""

# ============================================================================
# 9. workflow/sync return：SH 驳回到 JD
# ============================================================================
echo -e "${YELLOW}▸ 9. workflow/sync return：SH 驳回到 JD${NC}"

SYNC_RETURN=$(post "/api/review/workflow/sync" "{
    \"form_id\":\"${FORM_ID}\",\"token\":\"${PLATFORM_TOKEN}\",
    \"action\":\"return\",
    \"actor\":{\"id\":\"SH\",\"name\":\"审核\",\"roles\":\"sh\"},
    \"next_step\":{\"assignee_id\":\"JH\",\"name\":\"校对\",\"roles\":\"jd\"},
    \"comments\":\"E2E-驳回\"}")
SYNC_RETURN_CODE=$(jf "$SYNC_RETURN" "code")

assert_eq "return code=200" "200" "$SYNC_RETURN_CODE"
assert_eq "return 后 current_node=jd" "jd" "$(jf "$SYNC_RETURN" "data.current_node")"
assert_eq "return 后 task_status=submitted" "submitted" "$(jf "$SYNC_RETURN" "data.task_status")"
echo ""

# ============================================================================
# 10. workflow/sync stop：JD 终止流程
# ============================================================================
echo -e "${YELLOW}▸ 10. workflow/sync stop：JD 终止流程${NC}"

SYNC5=$(post "/api/review/workflow/sync" "{
    \"form_id\":\"${FORM_ID}\",\"token\":\"${PLATFORM_TOKEN}\",
    \"action\":\"stop\",
    \"actor\":{\"id\":\"JH\",\"name\":\"校对\",\"roles\":\"jd\"},
    \"comments\":\"E2E-终止流程\"}")
assert_eq "stop code=200" "200" "$(jf "$SYNC5" "code")"
assert_eq "stop 后 current_node=jd" "jd" "$(jf "$SYNC5" "data.current_node")"
assert_eq "stop 后 task_status=cancelled" "cancelled" "$(jf "$SYNC5" "data.task_status")"
assert_eq "stop 后 form_status=cancelled" "cancelled" "$(jf "$SYNC5" "data.form_status")"
echo ""

# ============================================================================
# 11. 最终 lineage 验证
# ============================================================================
echo -e "${YELLOW}▸ 11. 最终 lineage 验证${NC}"

FINAL=$(post "/api/review/embed-url" "{\"project_id\":\"${PROJECT_ID}\",\"user_id\":\"SJ\",\"workflow_role\":\"sj\",\"form_id\":\"${FORM_ID}\",\"token\":\"${PLATFORM_TOKEN}\"}")
FINAL_NODE=$(jf "$FINAL" "data.lineage.current_node")
FINAL_STATUS=$(jf "$FINAL" "data.lineage.status")

assert_eq "最终节点=jd" "jd" "$FINAL_NODE"
assert_eq "最终状态=cancelled" "cancelled" "$FINAL_STATUS"
echo ""

# ============================================================================
# 12. 非法 action 返回 400
# ============================================================================
echo -e "${YELLOW}▸ 12. 非法 action 返回 400${NC}"

BAD_ACTION=$(post "/api/review/workflow/sync" "{
    \"form_id\":\"${FORM_ID}\",\"token\":\"${PLATFORM_TOKEN}\",
    \"action\":\"invalid\",
    \"actor\":{\"id\":\"SJ\",\"name\":\"设计\",\"roles\":\"sj\"}}")
assert_eq "非法 action code=400" "400" "$(jf "$BAD_ACTION" "code")"
echo ""

# ============================================================================
# 13. 清理: 删除测试数据
# ============================================================================
echo -e "${YELLOW}▸ 13. 清理测试数据${NC}"

DEL=$(post "/api/review/delete" "{\"form_ids\":[\"${FORM_ID}\"],\"operator_id\":\"SJ\",\"token\":\"${PLATFORM_TOKEN}\"}")
DEL_CODE=$(jf "$DEL" "code")
assert_eq "删除 code=200" "200" "$DEL_CODE"
assert_eq "删除 results[0].success=true" "True" "$(jf "$DEL" "results.0.success")"

VERIFY=$(post "/api/review/workflow/sync" "{
    \"form_id\":\"${FORM_ID}\",\"token\":\"${PLATFORM_TOKEN}\",
    \"action\":\"query\",\"actor\":{\"id\":\"SJ\",\"name\":\"设计\",\"roles\":\"sj\"}}")
VERIFY_N=$(jlen "$VERIFY" "data.models")
VERIFY_RECORDS_N=$(jlen "$VERIFY" "data.records")
VERIFY_ATTACHMENTS_N=$(jlen "$VERIFY" "data.attachments")
assert_eq "删除后模型已清空" "0" "$VERIFY_N"
assert_eq "删除后 records 已清空" "0" "$VERIFY_RECORDS_N"
assert_eq "删除后 attachments 已清空" "0" "$VERIFY_ATTACHMENTS_N"
assert_eq "删除后 form_exists=true" "True" "$(jf "$VERIFY" "data.form_exists")"
assert_eq "删除后 form_status=deleted" "deleted" "$(jf "$VERIFY" "data.form_status")"
assert_eq "删除后 task_created=false" "False" "$(jf "$VERIFY" "data.task_created")"
echo ""

# ============================================================================
# 汇总
# ============================================================================
echo -e "${BLUE}╔══════════════════════════════════════════════════════════╗${NC}"
if [ $FAILED -eq 0 ]; then
    echo -e "${BLUE}║  ${GREEN}ALL PASSED${NC}  ${PASSED}/${TOTAL} assertions"
else
    echo -e "${BLUE}║  ${RED}${FAILED} FAILED${NC}  ${PASSED}/${TOTAL} assertions"
fi
echo -e "  form_id: ${FORM_ID}"
echo -e "  task_id: ${TASK_ID:-N/A}"
echo -e "${BLUE}╚══════════════════════════════════════════════════════════╝${NC}"

exit $FAILED
