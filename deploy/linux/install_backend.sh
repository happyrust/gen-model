#!/usr/bin/env bash
# 在 Ubuntu 服务器上装 / 换 aios-database（校审后端，:8022）与它依赖的外部 SurrealDB（:8009）。
#
# 由 .github/workflows/deploy-linux-ubuntu.yml 传整包上来之后调用；包在手里也可以自己
# scp 上去 `sudo INSTALL_DIR=... ./install_backend.sh` 跑，两条路跑的是同一段逻辑。
#
# 幂等：重复跑只是把二进制与配置换成包里那份再重启；上一版二进制留在 bin/aios-database.prev，
# ROLLBACK=1 再跑一次就换回去。
#
# 一件必须知道的事：store_mode = "rocksdb" 的部署里，数据库是**外部进程**，服务只连不拉
# （ADR-074）。所以这里装两个 systemd 单元，aios-database 依赖 surrealdb 先起来；
# 客户端 SDK 是 fork 的 2.1.4，服务端二进制必须是 2.1.x——3.x 打不开 2.x 的存储，
# 连内存后端都会在 WS 子协议上对不上。

set -euo pipefail

BUNDLE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

INSTALL_DIR="${INSTALL_DIR:-/opt/aios-database}"
DATA_DIR="${DATA_DIR:-/var/lib/aios-database}"
SURREAL_BIN="${SURREAL_BIN:-/usr/local/bin/surreal}"
SURREAL_DATA_DIR="${SURREAL_DATA_DIR:-/var/lib/surrealdb/ams-8009}"
SURREAL_BIND="${SURREAL_BIND:-127.0.0.1:8009}"
SURREAL_USER="${SURREAL_USER:-root}"
SURREAL_PASS="${SURREAL_PASS:-root}"
HTTP_PORT="${HTTP_PORT:-8022}"
PUBLIC_HOST="${PUBLIC_HOST:-}"
JWT_SECRET="${JWT_SECRET:-}"
RUST_LOG_LEVEL="${RUST_LOG_LEVEL:-info}"
SKIP_NGINX="${SKIP_NGINX:-0}"
ROLLBACK="${ROLLBACK:-0}"
HEALTH_TIMEOUT="${HEALTH_TIMEOUT:-240}"
NGINX_SITE="${NGINX_SITE:-}"

log() { printf '[deploy-backend] %s\n' "$*"; }
die() { printf '[deploy-backend] %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" = "0" ] || die "要 root：本脚本写 /opt、/etc/systemd/system 与 nginx 配置"

wait_http() {
  local url="$1" timeout="$2" label="$3" waited=0
  while [ "$waited" -lt "$timeout" ]; do
    if curl -fsS --max-time 5 "$url" >/dev/null 2>&1; then
      log "${label} 就绪（${waited}s）"
      return 0
    fi
    sleep 3
    waited=$((waited + 3))
  done
  return 1
}

dump_service_failure() {
  local unit="$1"
  printf '\n===== systemctl status %s =====\n' "$unit" >&2
  systemctl status "$unit" --no-pager --lines 20 >&2 || true
  printf '\n===== journalctl -u %s -n 200 =====\n' "$unit" >&2
  journalctl -u "$unit" -n 200 --no-pager >&2 || true
}

# ── 回滚：换回上一版二进制就走 ────────────────────────────────────────────────
if [ "$ROLLBACK" = "1" ]; then
  [ -f "${INSTALL_DIR}/bin/aios-database.prev" ] || die "没有 ${INSTALL_DIR}/bin/aios-database.prev，回滚无从谈起"
  log "回滚到上一版二进制"
  systemctl stop aios-database || true
  mv -f "${INSTALL_DIR}/bin/aios-database" "${INSTALL_DIR}/bin/aios-database.rolledback"
  mv -f "${INSTALL_DIR}/bin/aios-database.prev" "${INSTALL_DIR}/bin/aios-database"
  systemctl start aios-database
  wait_http "http://127.0.0.1:${HTTP_PORT}/api/v1/health" "$HEALTH_TIMEOUT" "回滚后的 /api/v1/health" \
    || { dump_service_failure aios-database; die "回滚后服务仍然起不来"; }
  exit 0
fi

[ -f "${BUNDLE_DIR}/aios-database" ] || die "包里没有 aios-database"
[ -f "${BUNDLE_DIR}/DbOption.toml" ] || die "包里没有 DbOption.toml"

# ── 目录 ────────────────────────────────────────────────────────────────────
mkdir -p "${INSTALL_DIR}/bin" "${DATA_DIR}/assets" "${DATA_DIR}/e3d" "${SURREAL_DATA_DIR%/*}"
chmod 755 "${INSTALL_DIR}" "${INSTALL_DIR}/bin"

# ── SurrealDB 2.1.x ─────────────────────────────────────────────────────────
have_surreal=""
[ -x "$SURREAL_BIN" ] && have_surreal="$("$SURREAL_BIN" version 2>/dev/null || true)"
case "$have_surreal" in
  *2.1.*) log "SurrealDB 已就位：${have_surreal}" ;;
  *)
    tgz="$(find "$BUNDLE_DIR" -maxdepth 1 -name 'surreal-*.tgz' -print -quit)"
    [ -n "$tgz" ] || die "机器上没有 2.1.x 的 surreal（现有：${have_surreal:-无}），包里也没带 surreal-*.tgz"
    log "从包里装 SurrealDB：$(basename "$tgz")"
    tar -xzf "$tgz" -C /usr/local/bin surreal
    chmod 755 /usr/local/bin/surreal
    log "SurrealDB 版本：$(/usr/local/bin/surreal version)"
    ;;
esac

# ── 二进制：先让它自报身份，再落位 ──────────────────────────────────────────
chmod 755 "${BUNDLE_DIR}/aios-database"
log "新二进制自报：$("${BUNDLE_DIR}/aios-database" version 2>&1 | head -1)"

systemctl stop aios-database 2>/dev/null || true
if [ -f "${INSTALL_DIR}/bin/aios-database" ]; then
  cp -f "${INSTALL_DIR}/bin/aios-database" "${INSTALL_DIR}/bin/aios-database.prev"
  log "上一版留在 bin/aios-database.prev（ROLLBACK=1 可换回去）"
fi
install -m 755 "${BUNDLE_DIR}/aios-database" "${INSTALL_DIR}/bin/aios-database"

if [ -f "${INSTALL_DIR}/DbOption.toml" ]; then
  cp -f "${INSTALL_DIR}/DbOption.toml" "${INSTALL_DIR}/DbOption.toml.bak-$(date -u '+%Y%m%dT%H%M%SZ')"
fi
install -m 644 "${BUNDLE_DIR}/DbOption.toml" "${INSTALL_DIR}/DbOption.toml"

# ── systemd ─────────────────────────────────────────────────────────────────
cat > /etc/systemd/system/surrealdb.service <<UNIT
[Unit]
Description=SurrealDB 2.1.x (aios-database 的持久层，rocksdb)
Documentation=https://surrealdb.com/docs
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
# 顺序发布要求 RPC 立刻回应，别攒在响应缓冲里（scripts/Start-Surreal8009.ps1 同款）。
Environment=SURREAL_WEBSOCKET_RESPONSE_BUFFER_SIZE=0
ExecStart=${SURREAL_BIN} start --user ${SURREAL_USER} --pass ${SURREAL_PASS} --bind ${SURREAL_BIND} rocksdb:${SURREAL_DATA_DIR}
Restart=on-failure
RestartSec=5
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
UNIT

# JWT 密钥不进单元文件（那是 644，谁都读得到）：单独一个 600 的 env 文件，
# 单元用 EnvironmentFile=- 引它，没有也能起。
[ -z "$JWT_SECRET" ] && [ -f "${BUNDLE_DIR}/jwt_secret" ] && JWT_SECRET="$(cat "${BUNDLE_DIR}/jwt_secret")"
if [ -n "$JWT_SECRET" ]; then
  umask 177
  printf 'JWT_SECRET=%s\n' "$JWT_SECRET" > "${INSTALL_DIR}/service.env"
  umask 022
  log "JWT_SECRET 写进 ${INSTALL_DIR}/service.env（600）"
fi

{
  cat <<UNIT
[Unit]
Description=aios-database (gen-model 增量服务 / 三维校审后端 :${HTTP_PORT})
After=network-online.target surrealdb.service
Requires=surrealdb.service

[Service]
Type=simple
WorkingDirectory=${INSTALL_DIR}
EnvironmentFile=-${INSTALL_DIR}/service.env
Environment=DB_OPTION_FILE=${INSTALL_DIR}/DbOption.toml
Environment=PLANT_ASSET_ROOT=${DATA_DIR}/assets
Environment=RUST_LOG=${RUST_LOG_LEVEL}
UNIT
  # 监听地址是 0.0.0.0，embed-url 回给 PMS 的地址得靠这一句推出公网 IP，
  # 否则它会拿 UDP 探出来的内网地址去拼（platform/config.rs）。
  [ -n "$PUBLIC_HOST" ] && echo "Environment=AIOS_PUBLIC_HOST=${PUBLIC_HOST}"
  cat <<UNIT
ExecStart=${INSTALL_DIR}/bin/aios-database
Restart=on-failure
RestartSec=10
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
UNIT
} > /etc/systemd/system/aios-database.service

systemctl daemon-reload
systemctl enable surrealdb aios-database >/dev/null

# ── 起库，再起服务 ──────────────────────────────────────────────────────────
systemctl restart surrealdb
wait_http "http://${SURREAL_BIND}/health" 60 "SurrealDB ${SURREAL_BIND}" \
  || { dump_service_failure surrealdb; die "SurrealDB 起不来，服务就不用起了（store_mode=rocksdb 只连不拉）"; }

systemctl restart aios-database
wait_http "http://127.0.0.1:${HTTP_PORT}/api/v1/health" "$HEALTH_TIMEOUT" "aios-database /api/v1/health" \
  || { dump_service_failure aios-database; die "服务 ${HEALTH_TIMEOUT}s 内没起来"; }

wait_http "http://127.0.0.1:${HTTP_PORT}/api/review/health" 30 "校审域 /api/review/health" \
  || { dump_service_failure aios-database; die "服务起来了但校审域不健康——先看启动横幅那句 review persistence"; }

# ── nginx：把校审四前缀分流到 :8022 ─────────────────────────────────────────
if [ "$SKIP_NGINX" = "1" ]; then
  log "按要求跳过 nginx"
else
  snippet=/etc/nginx/snippets/aios-review-split.conf
  mkdir -p /etc/nginx/snippets
  # 出处：gen-model scripts/review/nginx-review-split.conf。那一份连 /api/ → :3100 的
  # 兜底一起给，这里只留 :8022 那几条——站点文件本来就有 /api 与 /files/，重复写会打架。
  cat > "$snippet" <<SNIPPET
# 由 deploy/linux/install_backend.sh 生成，勿手改（下次部署会覆盖）。
# 校审域（含 PMS 入站那六条）走新后端 aios-database；其余 /api 留在站点文件里原来的上游。
location ~ ^/(api/review(?:/|\$)|api/users(?:/|\$)|api/auth(?:/|\$)|api/v1(?:/|\$)) {
    proxy_pass http://127.0.0.1:${HTTP_PORT};
    proxy_http_version 1.1;
    proxy_set_header Host \$host;
    proxy_set_header X-Real-IP \$remote_addr;
    proxy_set_header X-Forwarded-Host \$host;
    proxy_set_header X-Forwarded-Proto \$scheme;
    proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
    proxy_set_header Upgrade \$http_upgrade;
    proxy_set_header Connection "upgrade";
    client_max_body_size 50m;
    proxy_read_timeout 120s;
}

# 附件下载：前端契约写死的前缀（reviewApi.ts），落库的 download_url 里也带着它。
# 用 ^~ 且前缀更长，才压得过站点文件里那条 ^~ /files/ → 旧上游。
location ^~ /files/review_attachments/ {
    proxy_pass http://127.0.0.1:${HTTP_PORT};
    proxy_http_version 1.1;
    proxy_set_header Host \$host;
    proxy_set_header X-Real-IP \$remote_addr;
    proxy_set_header X-Forwarded-Proto \$scheme;
    proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
    client_max_body_size 50m;
}
SNIPPET

  site="$NGINX_SITE"
  if [ -z "$site" ]; then
    for candidate in /etc/nginx/sites-available/plant3d-web /etc/nginx/conf.d/plant3d-web.conf /etc/nginx/sites-available/default; do
      [ -f "$candidate" ] && { site="$candidate"; break; }
    done
  fi
  [ -n "$site" ] || die "找不到站点配置文件，nginx 这一步没法自动做（可用 NGINX_SITE=... 指定，或 SKIP_NGINX=1 跳过）"

  # 宽一点的匹配：前端那份模板里写的是通配 include（aios-review-split*.conf），
  # 按全名找会漏掉它，于是又插一行，两条 include 撞出重复 location。
  if grep -q 'aios-review-split' "$site"; then
    log "站点 ${site} 里已经 include 了分流片段"
  else
    backup="${site}.bak-$(date -u '+%Y%m%dT%H%M%SZ')"
    cp -f "$site" "$backup"
    # 插在 server_name 之后：那一行每个 server 块都有且只有一行，位置也在所有 location 之前。
    # 前端部署脚本会把整个站点文件覆写掉（deploy_frontend_bundle.sh），所以这一步每次部署都要重来。
    awk -v inc="    include ${snippet};" '
      { print }
      /^[[:space:]]*server_name[[:space:]]/ && !inserted { print inc; inserted = 1 }
    ' "$backup" > "$site"
    log "已把 include 插进 ${site}"
  fi

  nginx -t
  systemctl reload nginx || nginx -s reload
fi

# ── 交代清楚现状 ────────────────────────────────────────────────────────────
log "本机 /api/v1/health：$(curl -fsS --max-time 5 "http://127.0.0.1:${HTTP_PORT}/api/v1/health" | head -c 600)"
log "本机 /api/review/health：$(curl -fsS --max-time 5 "http://127.0.0.1:${HTTP_PORT}/api/review/health" | head -c 600)"
log "surrealdb：$(systemctl is-active surrealdb)  aios-database：$(systemctl is-active aios-database)"
log "装完了：${INSTALL_DIR}/bin/aios-database"
