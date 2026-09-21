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
# （ADR-074）。所以这里装两个 systemd 单元：aios-surrealdb 与 aios-database，后者依赖前者。
# 两个名字都带 aios- 前缀、surreal 二进制也落在 ${INSTALL_DIR}/bin 下，都是为了跟机器上
# 已有的东西错开——123.57.182.243 上既有一个别人的 surrealdb.service，/usr/local/bin/surreal
# 又是活着的 surreal-8020 在用的 3.2.0-nightly。
# 服务端二进制必须是 fork 的那条 2.1.4：上游 2.1.4 会在第一条 query 上无声挂住，
# 3.x 更是连 2.x 的存储都打不开。

set -euo pipefail

BUNDLE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

INSTALL_DIR="${INSTALL_DIR:-/opt/aios-database}"
DATA_DIR="${DATA_DIR:-/var/lib/aios-database}"
# 刻意不落 /usr/local/bin：123.57.182.243 上那里已经躺着一个 3.2.0-nightly，
# 活着的 surreal-8020.service 正用它开一份存储版本 3 的库。拿 2.1.4 盖掉它，
# 下次那台库重启就再也打不开自己的数据了。我们这一台只归自己用，放自己家里。
SURREAL_BIN="${SURREAL_BIN:-${INSTALL_DIR}/bin/surreal}"
# 单元名也带 aios- 前缀：那台机器上已经有一个（disabled 的）surrealdb.service，
# 指向 /root/shells/run_surrealdb_kv_8020.sh，不是我们的东西。
SURREAL_UNIT="${SURREAL_UNIT:-aios-surrealdb}"
SURREAL_DATA_DIR="${SURREAL_DATA_DIR:-/var/lib/surrealdb/ams-8009}"
SURREAL_BIND="${SURREAL_BIND:-127.0.0.1:8009}"
SURREAL_USER="${SURREAL_USER:-root}"
SURREAL_PASS="${SURREAL_PASS:-root}"
HTTP_PORT="${HTTP_PORT:-8022}"
PUBLIC_HOST="${PUBLIC_HOST:-}"
# 没有 JWT_SECRET 这个入参了：校审 JWT 的密钥在包里 DbOption.toml 的 [model_center].token_secret
# （render_dboption.py 从 secrets.REVIEW_JWT_SECRET 渲染），与旧 :3100 一样写在配置文件里。
# 服务的口径（review/auth.rs::JwtConfig::from_config_file）是有配置文件就只读那里，环境变量
# JWT_SECRET 根本不看——2026-09-18 首发走 service.env + EnvironmentFile 那条路就是这么落空的，
# 生产在代码默认串上跑到 2026-09-21 00:31 手工补上才对。
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

# 这台机器上别人的东西不许动。要覆盖的文件如果存在、又没有我们的印记，就停下来说清楚，
# 而不是备份一下继续——备份救不回一个被我们改跑偏的线上服务。
MANAGED_MARK='# managed-by: gen-model deploy/linux/install_backend.sh'
assert_ours() {
  local path="$1"
  [ -e "$path" ] || return 0
  grep -qF "$MANAGED_MARK" "$path" && return 0
  die "${path} 已经存在，而且不是这个脚本写的——先确认它归谁管（要接管就在文件头加一行 '${MANAGED_MARK}'）"
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
[ -d "${BUNDLE_DIR}/resource/surreal" ] || die "包里没有 resource/surreal——服务启动时按 CWD 相对路径读它，缺了连库那一步就退出"

# ── 目录 ────────────────────────────────────────────────────────────────────
mkdir -p "${INSTALL_DIR}/bin" "${DATA_DIR}/assets" "${DATA_DIR}/e3d" "${SURREAL_DATA_DIR%/*}"
chmod 755 "${INSTALL_DIR}" "${INSTALL_DIR}/bin"

# ── SurrealDB：必须是 fork 的那条 2.1.4 ─────────────────────────────────────
# 上游 2.1.4 不是替代品。2026-09-18 的彩排（run 35314163615）拿官方 2.1.4 试过：握手、
# signin、use 全过，第一条 query（common.surql）发出去之后再没有回音，两边都不报错，
# 最后是健康检查把服务判死的。客户端 SDK 是 gitee happydpc/surrealdb 那条线
# （Cargo.lock rev 45013fc9），服务端就得是同一条线的构建——它自报的版本里带 rev 后缀。
SURREAL_REQUIRE_REV="${SURREAL_REQUIRE_REV:-45013fc9}"
have_surreal=""
[ -x "$SURREAL_BIN" ] && have_surreal="$("$SURREAL_BIN" version 2>/dev/null || true)"
case "$have_surreal" in
  *"$SURREAL_REQUIRE_REV"*)
    log "SurrealDB 已就位：${have_surreal}"
    ;;
  *)
    if [ -f "${BUNDLE_DIR}/surreal" ]; then
      log "从包里装 SurrealDB（fork 构建）"
      install -m 755 "${BUNDLE_DIR}/surreal" "$SURREAL_BIN"
    else
      tgz="$(find "$BUNDLE_DIR" -maxdepth 1 -name 'surreal-*.tgz' -print -quit)"
      [ -n "$tgz" ] || die "包里既没有 surreal 二进制也没有 surreal-*.tgz（机器上现有：${have_surreal:-无}）"
      log "从包里装 SurrealDB：$(basename "$tgz")"
      tar -xzf "$tgz" -C "$(dirname "$SURREAL_BIN")" surreal
      chmod 755 "$SURREAL_BIN"
    fi
    new_surreal="$("$SURREAL_BIN" version)"
    log "SurrealDB 版本：${new_surreal}"
    case "$new_surreal" in
      *"$SURREAL_REQUIRE_REV"*) ;;
      *) die "装上的是 ${new_surreal}，不是 fork 的那条（rev ${SURREAL_REQUIRE_REV}）——宁可现在停，也别留一台会在第一条 query 上无声挂住的库" ;;
    esac
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

# 配置里必须带校审 JWT 密钥，且不是代码默认串。这一步整份覆盖服务器上的 DbOption.toml，
# 包里那份没有 [model_center] 就等于把线上密钥打回 default-jwt-secret-key（已发 token 全失效、
# 谁都能离线伪造）——宁可停在这儿。手工 scp 上来的包也过这道门。
bundle_secret="$(awk '
  /^\[model_center\]/ { f = 1; next }
  /^\[/               { f = 0 }
  f && /^token_secret[ \t]*=/ { sub(/^token_secret[ \t]*=[ \t]*/, ""); print; exit }
' "${BUNDLE_DIR}/DbOption.toml")"
case "$bundle_secret" in
  "")                          die "包里的 DbOption.toml 没有 [model_center].token_secret——render_dboption.py 没拿到 JWT_SECRET（仓库 secret REVIEW_JWT_SECRET）就渲染了？" ;;
  '""'|'"default-jwt-secret-key"') die "包里的 [model_center].token_secret 是空串 / 代码默认串，等于没配" ;;
esac

if [ -f "${INSTALL_DIR}/DbOption.toml" ]; then
  backup="${INSTALL_DIR}/DbOption.toml.bak-$(date -u '+%Y%m%dT%H%M%SZ')"
  cp -pf "${INSTALL_DIR}/DbOption.toml" "$backup"
  chmod 600 "$backup"   # 旧配置里可能也有密钥（2026-09-21 起就有）
fi
# 600：里面有密钥。服务以 root 跑（单元没有 User=），读得到。
install -m 600 "${BUNDLE_DIR}/DbOption.toml" "${INSTALL_DIR}/DbOption.toml"
log "DbOption.toml 已就位（600，含 [model_center].token_secret）"

# 首发那条不生效的路留下的文件：里面只有一行 JWT_SECRET，服务从来没读过。留着只会让人误以为密钥在这儿。
if [ -f "${INSTALL_DIR}/service.env" ]; then
  rm -f "${INSTALL_DIR}/service.env"
  log "移除旧的 ${INSTALL_DIR}/service.env（EnvironmentFile 那条路服务不读，密钥现在在 DbOption.toml [model_center]）"
fi

# aios_core::function::define_common_functions 用 std::fs::read_dir("resource/surreal")
# 读这一目录——**相对 CWD**，也就是单元里的 WorkingDirectory；整目录 sort 后逐个 query。
# 缺了它服务在连库那一步就退出，报的是「SurrealDB 连接失败（ws://…）: Failed to define
# common functions: No such file or directory (os error 2)」，长得跟网络不通一模一样。
rm -rf "${INSTALL_DIR}/resource/surreal"
mkdir -p "${INSTALL_DIR}/resource"
cp -r "${BUNDLE_DIR}/resource/surreal" "${INSTALL_DIR}/resource/"
chmod 644 "${INSTALL_DIR}"/resource/surreal/*.surql
log "resource/surreal：$(ls -1 "${INSTALL_DIR}/resource/surreal" | wc -l) 个 .surql 就位"

# ── systemd ─────────────────────────────────────────────────────────────────
assert_ours "/etc/systemd/system/${SURREAL_UNIT}.service"
assert_ours /etc/systemd/system/aios-database.service

cat > "/etc/systemd/system/${SURREAL_UNIT}.service" <<UNIT
${MANAGED_MARK}
[Unit]
Description=SurrealDB 2.1.4 fork (aios-database 的持久层，rocksdb ${SURREAL_BIND})
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

# 单元文件是 644，密钥不在这里：它在 600 的 DbOption.toml 里，DB_OPTION_FILE 指过去。
{
  cat <<UNIT
${MANAGED_MARK}
[Unit]
Description=aios-database (gen-model 增量服务 / 三维校审后端 :${HTTP_PORT})
After=network-online.target ${SURREAL_UNIT}.service
Requires=${SURREAL_UNIT}.service

[Service]
Type=simple
WorkingDirectory=${INSTALL_DIR}
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
systemctl enable "$SURREAL_UNIT" aios-database >/dev/null

# ── 起库，再起服务 ──────────────────────────────────────────────────────────
systemctl restart "$SURREAL_UNIT"
wait_http "http://${SURREAL_BIND}/health" 60 "SurrealDB ${SURREAL_BIND}" \
  || { dump_service_failure "$SURREAL_UNIT"; die "SurrealDB 起不来，服务就不用起了（store_mode=rocksdb 只连不拉）"; }

systemctl restart aios-database
wait_http "http://127.0.0.1:${HTTP_PORT}/api/v1/health" "$HEALTH_TIMEOUT" "aios-database /api/v1/health" \
  || { dump_service_failure aios-database; dump_service_failure "$SURREAL_UNIT"; die "服务 ${HEALTH_TIMEOUT}s 内没起来"; }

wait_http "http://127.0.0.1:${HTTP_PORT}/api/review/health" 30 "校审域 /api/review/health" \
  || { dump_service_failure aios-database; die "服务起来了但校审域不健康——先看启动横幅那句 review persistence"; }

# ── nginx：把校审四前缀分流到 :8022 ─────────────────────────────────────────
if [ "$SKIP_NGINX" = "1" ]; then
  log "按要求跳过 nginx"
else
  snippet=/etc/nginx/snippets/aios-review-split.conf
  mkdir -p /etc/nginx/snippets

  # 切哪几个前缀是个要拿主意的事，所以做成参数。默认只切这两个，是 2026-09-18 跟用户对过的
  # 口径：123.57.182.243 上 /api/review 是 504、/api/auth 是 404，都没人在用，切过来不动
  # 任何现有流量；而 /api/v1 与 /api/users 此刻由 :3100 那个旧 web_server 实供着
  # （/api/v1/health 回的是带 litefs 字段的那份，200），把它们一起切等于把现网前端的数据面
  # 也换了后端——新后端的 core 还在 initializing、机器上没有 E3D 工程，那一排会是空的。
  # 要切全四个：REVIEW_SPLIT_PREFIXES="api/review api/users api/auth api/v1"。
  REVIEW_SPLIT_PREFIXES="${REVIEW_SPLIT_PREFIXES:-api/review api/auth}"
  dollar='$'
  split_re=""
  for prefix in $REVIEW_SPLIT_PREFIXES; do
    split_re="${split_re}|${prefix}(?:/|${dollar})"
  done
  split_re="${split_re#|}"
  log "分流前缀：${REVIEW_SPLIT_PREFIXES}"

  # 出处：gen-model scripts/review/nginx-review-split.conf。那一份连 /api/ → :3100 的
  # 兜底一起给，这里只留 :8022 那几条——站点文件本来就有 /api 与 /files/，重复写会打架。
  cat > "$snippet" <<SNIPPET
# 由 deploy/linux/install_backend.sh 生成，勿手改（下次部署会覆盖）。
# 校审域（含 PMS 入站那六条）走新后端 aios-database；其余 /api 留在站点文件里原来的上游。
location ~ ^/(${split_re}) {
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

  # reload 是异步的：SIGHUP 之后老 worker 还会拿旧配置再服务一小会儿。2026-09-18 的彩排
  # （run 35317065250）就栽在这里——脚本 reload 完，外面 38 毫秒后去探，探到的还是旧配置下
  # 的 /api → :3100，一个干净的 502。所以这一步自己盯到分流真的生效为止。
  # 不只看状态码：服务器上 :3100 那个旧后端也可能回 200，认的是校审 health 的响应体。
  nginx_host_header=()
  [ -n "$PUBLIC_HOST" ] && nginx_host_header=(-H "Host: ${PUBLIC_HOST}")
  split_ok=""
  split_last=""
  for _ in $(seq 1 20); do
    split_last="$(curl -s --max-time 5 "${nginx_host_header[@]}" \
      "http://127.0.0.1/api/review/health" 2>/dev/null || true)"
    case "$split_last" in
      *'"database"'*) split_ok=1; break ;;
    esac
    sleep 1
  done
  [ -n "$split_ok" ] || die "nginx reload 过了，但从 :80 走 /api/review/health 拿到的不是校审后端的响应（最后一次：${split_last:-空}）——分流没生效，先看站点里还有没有别的 location 压在前面"
  log "nginx 分流生效：:80 /api/review/health => ${split_last}"
fi

# ── 交代清楚现状 ────────────────────────────────────────────────────────────
log "本机 /api/v1/health：$(curl -fsS --max-time 5 "http://127.0.0.1:${HTTP_PORT}/api/v1/health" | head -c 600)"
log "本机 /api/review/health：$(curl -fsS --max-time 5 "http://127.0.0.1:${HTTP_PORT}/api/review/health" | head -c 600)"
log "${SURREAL_UNIT}：$(systemctl is-active "$SURREAL_UNIT")  aios-database：$(systemctl is-active aios-database)"
log "装完了：${INSTALL_DIR}/bin/aios-database（surreal 在 ${SURREAL_BIN}，没碰 /usr/local/bin）"
