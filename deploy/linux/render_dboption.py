#!/usr/bin/env python3
"""把仓库里那份 DbOption.toml 改写成服务器口径，写到 bundle/DbOption.toml。

部署（deploy-linux-ubuntu.yml）和彩排（rehearse-backend-stack.yml）共用这一份。
两边各抄一段的话，改了一处忘了另一处，彩排就不再代表真部署了。

读这几个环境变量：
  E3D_PROJECT_DIR      project_path 指到哪（服务器上没有 E3D 工程，给个空目录）
  HTTP_PORT            http_api_addr 的端口
  REMOTE_HOST          review_web_base_url 的缺省来源
  REVIEW_WEB_BASE_URL  显式给了就用它
  JWT_SECRET           校审 / PMS 入站 JWT 的密钥，写进 [model_center].token_secret（必填，见下）
  DBOPTION_SRC / DBOPTION_DST  默认 DbOption.toml -> bundle/DbOption.toml

关于 JWT_SECRET 为什么走这里而不是环境变量：review/auth.rs::JwtConfig::from_config_file 的口径是
「配置文件能载入就只读 model_center.token_secret，读不到回落 default-jwt-secret-key」，环境变量
JWT_SECRET 只在找不到任何配置文件时才被读——而单元里 DB_OPTION_FILE 一定指着一份能载入的配置。
2026-09-18 首发时 install_backend.sh 把密钥写进 service.env 走 EnvironmentFile，那条路服务根本不看，
生产就这样在代码默认串上跑到 2026-09-21 00:31（手工补上 [model_center] 才对）。所以密钥和旧 :3100
plant-model-gen 一样写在配置文件里；PMS 不持有它（PMS 的 token 是调 /api/auth/token 领的），值本身
随便一串强随机都行，要求只有「本机签 / 验一致、跨重部署稳定」。没给就拒绝渲染，不再默默回落默认串。
"""

import json
import os
import pathlib
import re
import sys
import textwrap

src = pathlib.Path(os.environ.get("DBOPTION_SRC", "DbOption.toml"))
dst = pathlib.Path(os.environ.get("DBOPTION_DST", "bundle/DbOption.toml"))
text = src.read_text(encoding="utf-8")

# 只改「这台机器是什么样」的那几格，其余一律沿用仓库里那份——服务器配置和开发档
# 之间每多一处自说自话，以后排查就多一处要先对账的地方。
overrides = {
    "project_path": f'"{os.environ["E3D_PROJECT_DIR"]}"',   # Windows 的 D:/AVEVA/... 在这儿不存在
    "v_ip": '"127.0.0.1"',
    "store_mode": '"rocksdb"',                              # mem 档下校审四前缀一律 503
    "http_api_addr": f'"0.0.0.0:{os.environ["HTTP_PORT"]}"',
    "startup_autorun": "false",                             # 首发别让它一上来就消化积压
    "gen_model": "false",
    "gen_mesh": "false",
}

for key, value in overrides.items():
    pattern = rf"(?m)^{re.escape(key)}\s*=.*$"
    if not re.search(pattern, text):
        sys.exit(f"DbOption.toml 里没有顶层键 {key}：模板变了，先对一遍再部署")
    text = re.sub(pattern, f"{key} = {value}", text, count=1)

review_web = os.environ.get("REVIEW_WEB_BASE_URL", "").strip() \
    or f'http://{os.environ["REMOTE_HOST"]}'

# 密钥：必填、不许是代码默认串、不许短得像占位。secrets.REVIEW_JWT_SECRET 末尾常带换行，先 strip。
DEFAULT_JWT_SECRET = "default-jwt-secret-key"
jwt_secret = os.environ.get("JWT_SECRET", "").strip()
if not jwt_secret:
    sys.exit("没有 JWT_SECRET（仓库 secret REVIEW_JWT_SECRET）：拒绝渲染。没有它服务会回落代码默认串 "
             f"{DEFAULT_JWT_SECRET}，谁都能离线签出任意身份的 token；上一次部署留下的服务器密钥也会被这份配置盖掉。"
             "先 `gh secret set REVIEW_JWT_SECRET`（值与服务器上 [model_center].token_secret 相同则已发 token 不失效）。")
if jwt_secret == DEFAULT_JWT_SECRET:
    sys.exit(f"JWT_SECRET 就是代码默认串 {DEFAULT_JWT_SECRET}：等于没配，拒绝渲染")
if len(jwt_secret) < 16:
    sys.exit(f"JWT_SECRET 只有 {len(jwt_secret)} 个字符，短得像占位；至少 16 个（旧 :3100 那把是 22 个）")
# TOML basic string 的转义与 JSON 字符串一致（\" \\ \n \t \uXXXX），用 json.dumps 保证任何字符都能进去。
jwt_secret_toml = json.dumps(jwt_secret, ensure_ascii=False)

# TOML 的表头必须排在所有顶层键之后，所以这几段只能追加在文件末尾。
text += textwrap.dedent(f"""\

    # ── 以下四段由 deploy/linux/render_dboption.py 按服务器口径追加 ──────────
    # 校审接口的 JWT 鉴权。四个键的缺省与旧 :3100 端逐字相同，这里显式写出来
    # 是为了「服务器上到底开没开」一眼看得见。
    [review_auth]
    enabled = true

    # PMS 平台入站（S2S）那六条：embed-url / workflow sync / workflow verify /
    # annotations check / cache preload / delete。review_web_base_url 就是
    # embed-url 回给 PMS 的那个地址（移植计划 Q5）。
    [platform_auth]
    enabled = true
    review_web_base_url = "{review_web}"

    # 附件落盘位置；URL 前缀 /files/review_attachments/ 是前端写死的契约，
    # 不在这儿配。
    [review_attachments]
    dir = "review_attachments"

    # 校审 / PMS 入站 JWT 的密钥与有效期（review/auth.rs::JwtConfig::from_config_file **只**从这里读；
    # 有配置文件时环境变量 JWT_SECRET 不生效）。值来自仓库 secret REVIEW_JWT_SECRET，与旧 :3100 一样写在
    # 配置文件里；PMS 不持有它（PMS 的 token 是调 /api/auth/token 领的），换值 = 已发 token 全部失效、重领即可。
    # 本文件因此装成 600（install_backend.sh）。
    [model_center]
    token_secret = {jwt_secret_toml}
    token_expiration_hours = 24
    """)

dst.parent.mkdir(parents=True, exist_ok=True)
with open(dst, "w", encoding="utf-8", newline="\n") as fh:
    fh.write(text)
# 里面有密钥了：runner 上就收成 600，rsync -a 会把这个权限原样带到服务器的 /tmp。
os.chmod(dst, 0o600)

# 自检：写出去的必须是合法 TOML，且密钥原样进了 model_center.token_secret（转义没出错）。
try:
    import tomllib
except ImportError:  # python < 3.11 的 runner 没有 tomllib，跳过自检
    tomllib = None
if tomllib is not None:
    parsed = tomllib.loads(text)
    got = parsed.get("model_center", {}).get("token_secret")
    if got != jwt_secret:
        sys.exit("自检失败：渲染出的 model_center.token_secret 与 JWT_SECRET 不一致（转义出错）")
    for section in ("review_auth", "platform_auth", "review_attachments", "model_center"):
        if section not in parsed:
            sys.exit(f"自检失败：渲染结果里没有 [{section}]")

# 日志里不出现密钥本身，只报长度。
print(f"渲染完成：{dst}（{len(text)} 字节；project_path={os.environ['E3D_PROJECT_DIR']}，"
      f"http_api_addr=0.0.0.0:{os.environ['HTTP_PORT']}，review_web_base_url={review_web}，"
      f"model_center.token_secret 已写入（{len(jwt_secret)} 个字符，文件 600））")
