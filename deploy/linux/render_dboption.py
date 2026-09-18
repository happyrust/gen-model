#!/usr/bin/env python3
"""把仓库里那份 DbOption.toml 改写成服务器口径，写到 bundle/DbOption.toml。

部署（deploy-linux-ubuntu.yml）和彩排（rehearse-backend-stack.yml）共用这一份。
两边各抄一段的话，改了一处忘了另一处，彩排就不再代表真部署了。

读这几个环境变量：
  E3D_PROJECT_DIR      project_path 指到哪（服务器上没有 E3D 工程，给个空目录）
  HTTP_PORT            http_api_addr 的端口
  REMOTE_HOST          review_web_base_url 的缺省来源
  REVIEW_WEB_BASE_URL  显式给了就用它
  DBOPTION_SRC / DBOPTION_DST  默认 DbOption.toml -> bundle/DbOption.toml
"""

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

# TOML 的表头必须排在所有顶层键之后，所以这三段只能追加在文件末尾。
text += textwrap.dedent(f"""\

    # ── 以下三段由 deploy/linux/render_dboption.py 按服务器口径追加 ──────────
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
    """)

dst.parent.mkdir(parents=True, exist_ok=True)
with open(dst, "w", encoding="utf-8", newline="\n") as fh:
    fh.write(text)

print(f"渲染完成：{dst}（{len(text)} 字节；project_path={os.environ['E3D_PROJECT_DIR']}，"
      f"http_api_addr=0.0.0.0:{os.environ['HTTP_PORT']}，review_web_base_url={review_web}）")
