#!/usr/bin/env bash
# 使用 JSON 文件以 POST 方式验证 Platform API（PMS 入站接口）。
# 依赖: curl, jq（可选；无 jq 时仅打印说明并跳过需 token 的步骤）
#
# 用法:
#   export BASE_URL=http://localhost:8022
#   ./scripts/review/verify_platform_api_json.sh
#
# 若 review_auth.enabled=false，部分接口可不传 JWT；否则先取 token 再跑 workflow/cache/delete。
#
# 来源：plant-model-gen shells/verify_platform_api_json.sh 搬入 gen-model；改动：默认地址 :3100 → :8022，
#   JSON 夹具目录随脚本搬到同级 platform_api_json/（旧端 shells/platform_api_json/ 在源目录里并不存在，
#   夹具按旧端 jwt_auth.rs::TokenRequest / platform_api/types.rs 的请求体补齐）。
#   计划：docs/plans/2026-09-12-review-backend-port-plan.md §6 A8。

set -euo pipefail

JSON_DIR="$(cd "$(dirname "$0")" && pwd)/platform_api_json"
BASE_URL="${BASE_URL:-http://localhost:8022}"
BASE_URL="${BASE_URL%/}"

echo "== BASE_URL=$BASE_URL"

have_jq() { command -v jq >/dev/null 2>&1; }

if ! have_jq; then
  echo "!! 需要 jq 才能执行这套 JSON 契约验证，请先安装 jq。"
  exit 1
fi

echo ""
echo "== 1) POST /api/auth/token (auth_token.json)"
AUTH_RAW=$(curl -sS -X POST "$BASE_URL/api/auth/token" \
  -H 'Content-Type: application/json' \
  -d @"$JSON_DIR/auth_token.json")
echo "$AUTH_RAW" | jq .

TOKEN=$(echo "$AUTH_RAW" | jq -r '.data.token // empty')
FORM_ID=$(echo "$AUTH_RAW" | jq -r '.data.form_id // empty')

if [[ -z "$TOKEN" || "$TOKEN" == "null" ]]; then
  echo "!! 未取得 token（可能 review_auth 关闭或接口失败）。跳过需鉴权的请求。"
  exit 0
fi

echo "   使用 form_id=$FORM_ID"

echo ""
echo "== 1b) user_token（JWT）payload 不得包含 form_id（仅 URL/query 携带）"
jwt_payload_json() {
  python3 - "$1" <<'PY'
import sys, json, base64
token = sys.argv[1]
part = token.split(".")[1]
pad = (-len(part)) % 4
if pad:
    part += "=" * pad
raw = base64.urlsafe_b64decode(part.encode("ascii"))
sys.stdout.buffer.write(raw)
PY
}
if ! command -v python3 >/dev/null 2>&1; then
  echo "!! 未安装 python3，跳过 JWT payload 检查（可运行: cargo test test_jwt_payload_json_has_no_form_id_key）"
elif PAYLOAD_RAW=$(jwt_payload_json "$TOKEN"); then
  if echo "$PAYLOAD_RAW" | jq -e 'has("form_id")' >/dev/null 2>&1; then
    echo "!! 验证失败：JWT payload 含 form_id（应仅从 .data.form_id / URL 传递）"
    echo "$PAYLOAD_RAW" | jq .
    exit 1
  fi
  echo "   OK：payload 无 form_id 键。keys=$(echo "$PAYLOAD_RAW" | jq -c 'keys')"
else
  echo "!! JWT payload 解码失败"
  exit 1
fi

echo ""
echo "== 2) POST /api/auth/verify（检查 claims.workflow_mode）"
VERIFY_RAW=$(jq -n --arg t "$TOKEN" '{token:$t}' \
  | curl -sS -X POST "$BASE_URL/api/auth/verify" -H 'Content-Type: application/json' -d @-)
echo "$VERIFY_RAW" | jq .

VERIFY_CODE=$(echo "$VERIFY_RAW" | jq -r '.code // empty')
if [[ "$VERIFY_CODE" != "200" ]]; then
  echo "!! 验证失败：/api/auth/verify code 不是 200，实际为: ${VERIFY_CODE:-<empty>}"
  exit 1
fi

# claims.workflow_mode 只在旧端 `review-internal-workflow` feature 构建里出现；该 feature 不移植
# （计划 Q7 / N6），gen-model 与旧端默认构建一样：/api/auth/token 收到 workflow_mode 只告警忽略，
# claims 不携带。因此这条断言改为 opt-in：EXPECT_WORKFLOW_MODE=manual 时才判等，默认只打印实际值。
EXPECT_WORKFLOW_MODE="${EXPECT_WORKFLOW_MODE:-}"
VERIFY_WORKFLOW_MODE=$(echo "$VERIFY_RAW" | jq -r '.data.claims.workflow_mode // .data.claims.workflowMode // empty')
if [[ -n "$EXPECT_WORKFLOW_MODE" ]]; then
  if [[ "$VERIFY_WORKFLOW_MODE" != "$EXPECT_WORKFLOW_MODE" ]]; then
    echo "!! 验证失败：claims.workflow_mode 不是 ${EXPECT_WORKFLOW_MODE}，实际为: ${VERIFY_WORKFLOW_MODE:-<empty>}"
    exit 1
  fi
  echo "   OK：claims.workflow_mode=${VERIFY_WORKFLOW_MODE}"
else
  echo "   claims.workflow_mode=${VERIFY_WORKFLOW_MODE:-<absent>}（外部驱动模式下不携带；要判等请设 EXPECT_WORKFLOW_MODE）"
fi

echo ""
echo "== 3) POST /api/review/embed-url（检查公开 URL 保留 form_id，且不回流用户身份字段）"
EMBED_RAW=$(jq -n \
  --arg p "$(jq -r '.project // .project_id' "$JSON_DIR/auth_token.json")" \
  --arg u "$(jq -r '.username // .user_id' "$JSON_DIR/auth_token.json")" \
  --arg t "$TOKEN" \
  --arg f "$FORM_ID" \
  '{project_id:$p, user_id:$u, form_id:$f, token:$t}' \
  | curl -sS -X POST "$BASE_URL/api/review/embed-url" -H 'Content-Type: application/json' -d @-)
echo "$EMBED_RAW" | jq .

EMBED_URL=$(echo "$EMBED_RAW" | jq -r '.url // empty')
if [[ -z "$EMBED_URL" ]]; then
  echo "!! 验证失败：embed-url 响应缺少 url"
  exit 1
fi
if [[ "$EMBED_URL" != *"user_token="* ]]; then
  echo "!! 验证失败：embed-url 未携带 user_token"
  exit 1
fi
if [[ "$EMBED_URL" != *"form_id="* ]]; then
  echo "!! 验证失败：embed-url 未携带 form_id -> $EMBED_URL"
  exit 1
fi
if [[ "$EMBED_URL" == *"project_id="* || "$EMBED_URL" == *"user_id="* || "$EMBED_URL" == *"output_project="* ]]; then
  echo "!! 验证失败：embed-url 仍回流旧身份 query 参数 -> $EMBED_URL"
  exit 1
fi

EMBED_TOKEN=$(echo "$EMBED_RAW" | jq -r '.data.token // empty')
if [[ -n "$EMBED_TOKEN" && "$EMBED_TOKEN" != "null" && "$EMBED_TOKEN" != "$TOKEN" ]]; then
  echo ""
  echo "== 3b) embed-url 返回的 JWT payload 同样不得包含 form_id"
  if command -v python3 >/dev/null 2>&1; then
    EP=$(jwt_payload_json "$EMBED_TOKEN")
    if echo "$EP" | jq -e 'has("form_id")' >/dev/null 2>&1; then
      echo "!! 验证失败：embed JWT 含 form_id"
      echo "$EP" | jq .
      exit 1
    fi
    echo "   OK：embed token payload 无 form_id"
  fi
fi

TMP=$(mktemp)
jq --arg t "$TOKEN" --arg f "$FORM_ID" \
  '.token = $t | .form_id = $f' \
  "$JSON_DIR/workflow_sync_query.json" >"$TMP"
echo ""
echo "== 4) POST /api/review/workflow/sync (query)"
curl -sS -X POST "$BASE_URL/api/review/workflow/sync" \
  -H 'Content-Type: application/json' \
  -d @"$TMP" | jq .
rm -f "$TMP"

TMP=$(mktemp)
jq --arg t "$TOKEN" --arg f "$FORM_ID" \
  '.token = $t | .form_id = $f' \
  "$JSON_DIR/workflow_sync_active.json" >"$TMP"
echo ""
echo "== 5) POST /api/review/workflow/sync (active)"
curl -sS -X POST "$BASE_URL/api/review/workflow/sync" \
  -H 'Content-Type: application/json' \
  -d @"$TMP" | jq .
rm -f "$TMP"

TMP=$(mktemp)
jq --arg t "$TOKEN" '.token = $t' "$JSON_DIR/cache_preload.json" >"$TMP"
echo ""
echo "== 6) POST /api/review/cache/preload"
curl -sS -X POST "$BASE_URL/api/review/cache/preload" \
  -H 'Content-Type: application/json' \
  -d @"$TMP" | jq .
rm -f "$TMP"

TMP=$(mktemp)
jq --arg t "$TOKEN" \
  --argjson ids "[\"$FORM_ID\"]" \
  '.token = $t | .form_ids = $ids' \
  "$JSON_DIR/review_delete.json" >"$TMP"
echo ""
echo "== 7) POST /api/review/delete（会删除该 form 关联数据，默认跳过）"
read -r -p "   确认执行删除? [y/N] " ans || true
if [[ "${ans:-}" =~ ^[yY]$ ]]; then
  curl -sS -X POST "$BASE_URL/api/review/delete" \
    -H 'Content-Type: application/json' \
    -d @"$TMP" | jq .
else
  echo "   已跳过。"
fi
rm -f "$TMP"

echo ""
echo "== 完成"
