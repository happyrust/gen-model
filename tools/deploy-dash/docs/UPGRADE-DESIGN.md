# 部署面板自动升级 · 设计（U0）

2026-11-13，协调者写。计划是用户批准过的《部署面板自动升级：把本机最新程序补丁到所有服务器》（U0–U7）。这份文档把计划里没写死的地方写死，是三位代理共用的约定：线上的字段名、文件怎么判、失败了怎么办，都以这里为准。写的时候发现和计划有出入的地方列在第 9 节。

读法：第 1–3 节人人要读；第 4 节是协议（agent-server 实现，agent-client 调用）；第 5、6 节是代理一侧（agent-server）；第 7 节是面板一侧（agent-client、fleet-ui）；第 8 节是失败处理表；第 10 节是分工、顺序和验收。

哪里写得不对、实现不了、或者两条互相打架：不要自己悄悄改，写到 `UPGRADE-BOARD.md` 的「要协调者定」，我来改这份文档。字段只加不改这条规矩照旧（`schema` 仍是 1）。

## 1. 一次升级从头到尾

1. 本机摆好一套最新的包（一个带 `release.json` 的目录，叫**来源包**）。
2. 面板问每台服务器的代理「你现在装的是什么」，在本机算出每台要换哪些文件、要传多少、要不要停服务，给人看（**预览**）。
3. 人点了升级：面板对每台开一个**会话**，交一份签过名的清单，代理回「我还缺哪些内容」，面板只传这些（分块、可续传、按 sha256 核对）。传的时候服务照常跑。
4. 面板一台一台地下 `upgrade` 任务。代理：等后端空下来 → 停服务 → 把旧文件挪进备份、新文件就位 → 起服务 → 检查。任何一步没过，代理自己换回旧文件、把服务恢复成升级前的样子。
5. 需要的话再下 `upgrade_agent` 任务，换代理自己的 exe。
6. 事后可以点「回到上一版」（`rollback` 任务），把最近一次升级换下来的文件换回去。

升级能力缺省是关的：服务器上生成了**升级密钥**才算开。

## 2. 名词

| 词 | 意思 |
|---|---|
| 根 | 服务器上这套部署的安装目录（`stack.ps1`、`stack.toml` 所在的目录） |
| 已装清单 | `<根>\release.json`，记着这台现在装的是哪个包、每个文件的字节数和 sha256 |
| 来源包 | 面板这台电脑上的一个目录，里面是解开的发布包，带 `release.json` |
| 内容 | 一个文件的字节，用它的 sha256（64 位小写十六进制）指代；两个路径内容相同就只传一次 |
| 会话 | 一次「交清单 + 传内容」，有会话号；应用是另外下的任务 |
| 升级密钥 | 服务器上生成的 32 字节随机数，只用来给清单和任务签名，不在这套协议里传 |
| 本机改过 | 磁盘上的文件和已装清单记的 sha256 不一样（典型的是填了口令的 `config/DbOption.factory.toml`） |

## 3. 不变量

做错了会出事的几条，实现和评审都拿它们对：

1. **清单以外的东西不碰。** 只动「已装清单或新清单里列着的路径」。`stack.toml`、`run\`、`logs\`、`data\`、`work\`、`tmp\` 永远不在清单里，出现了整份拒绝。
2. **本机改过的文件不覆盖、不删除。** 新版也改了它，就留着本机的，把新版另存一份，在报告里点名。服务器上的库口令就写在这样一个文件里，覆盖了这套就起不来。
3. **要么全是旧的，要么全是新的。** 每挪一个文件先记日志再动手；任何时候代理死掉，下次起来都能照日志换回去。
4. **没过就换回，并恢复升级前的样子。** 升级前服务在跑，换回之后也要在跑；升级前增量同步是关的，之后也是关的。
5. **光有令牌推不了程序。** 交清单、下 `upgrade` / `rollback` / `upgrade_agent` 任务都要升级密钥的签名。传内容不用签名，因为每块内容都要对上签过名的清单里的 sha256。
6. **一台服务器同一时刻只有一件会改程序的事在做。** 沿用代理现成的「同一时刻只跑一个任务」。
7. **密钥和令牌不进日志、审计、报告、板、截图。** 审计里只记密钥指纹（第 4.3 节）。

## 4. 协议新增

类型放 `deploy-dash-proto\src\upgrade.rs`，字段名以本节的 JSON 为准；每个应答顶层照旧带 `schema`、`agent_version`（下面的样例里省略）。新枚举照旧带 `Unknown` 兜底。路径常量加进 `paths`，每个端点一份夹具。

### 4.1 能力

`/v1/info` 的 `caps` 加 `upgrade.v1`：表示这个代理有本节的端点（开没开看 4.2 的 `enabled`）。没有这一项的代理，面板显示「这台的代理不支持升级」。做了 4.5 的压缩再加 `upgrade.deflate`。

### 4.2 现状：`GET /v1/upgrade/inventory?files=0|1`

只要令牌，不要签名（预览不该要密钥）。

```json
{
  "enabled": true,
  "key_id": "3fa9c2e1",
  "accept_channels": [],
  "accept_dirty": true,
  "managed": true,
  "release": { "name": "…-rc1-r7", "version": "20261112-rc1-r7", "channel": "rc", "created_at": "2026/11/12 9:37:05" },
  "components": [ { "id": "backend", "title": "后端", "version": "0.1.44", "path": "bin/aios-database.exe", "sha256": "…", "dirty": true, "bytes": 234799616 } ],
  "agent_exe": { "kind": "headless", "version": "0.1.0", "sha256": "…", "bytes": 6100000 },
  "files": [ { "path": "bin/aios-database.exe", "bytes": 234799616, "sha256": "…", "state": "same" } ],
  "digest": "…",
  "disk_free_bytes": 134000000000,
  "busy": { "job_id": null, "backend_running": 0, "backend_queued": 0, "stack_lock": "" },
  "services_running": true,
  "can_rollback": true,
  "last": null
}
```

- `enabled`：有没有升级密钥。`key_id`：密钥指纹，没开是空串。
- `managed`：根下有读得懂的 `release.json`。`false` 时 `release` 是 `null`、`components` 和 `files` 是空的，这台升不了（不是发布包装出来的）。
- `release`、`components`：照抄已装清单。
- `agent_exe`：正在跑的这个代理 exe。`kind` 是 `headless`（`deploy-dash-agent.exe`）或 `panel`（面板 exe 的 `--agent` 身份）；`sha256`、`bytes` 是磁盘上那个文件的。
- `files`：只在 `files=1` 时给。已装清单里的每个路径一项，`sha256` 和 `bytes` 是**磁盘上实际的**；`state` 是 `same`（和已装清单一样）、`modified`（本机改过）、`missing`（不在了，此时 `sha256` 空串、`bytes` 0）。
- `digest`：把所有 `路径 + 磁盘 sha256` 排序后拼起来再算的 sha256。面板拿它判断「和上次看到的一样」，不用每次都要 `files`。
- `busy`：代理自己在跑的任务号、后端在跑和排队的任务数、stack 的锁被哪个命令占着（空串是没占）。
- `services_running`：有服务在跑。
- `can_rollback`：有一份能换回去的备份。`last`：最近一次升级或换回的报告（4.7 的一项），没有是 `null`。

算磁盘上的 sha256 要把整套文件读一遍（约 600 MB）。代理按「路径、大小、改动时间」缓存，第一次可能要十几秒，之后只重算变了的。这个端点同步返回，面板给它 60 秒超时。

### 4.3 签名

会改程序的三种请求要带三个头：

```
X-Upgrade-Key-Id:    <key_id>
X-Upgrade-Nonce:     <一次性随机数>
X-Upgrade-Signature: <签名>
```

- 随机数从 `POST /v1/upgrade/nonce`（只要令牌，空请求体）拿：`{ "nonce": "<64 位十六进制>", "expires_at": "…" }`。两分钟内有效，用一次就作废；同时最多留 16 个没用的，多了丢最老的。
- 签名 = `HMAC-SHA256(密钥的 32 字节, 文本)` 的小写十六进制，文本是四行用 `\n` 连起来（末尾没有换行）：

  ```
  aios-upgrade-v1
  <用途>
  <随机数>
  <请求体字节的 sha256，小写十六进制>
  ```

  用途：交清单是 `manifest:<会话号>`，下任务是 `job`。签的是请求体的原始字节，所以两边不用约定 JSON 怎么排版。
- 密钥的文字形式是 64 位小写十六进制。`key_id` 是「密钥 32 字节的 sha256」的前 8 位十六进制。
- 代理按这个顺序判，哪一步不对就回哪一个错误码（4.8）：没开升级 → `key_id` 对不上 → 随机数不认识或过期 → 签名不对。签名比较用常量时间。签名不对也把那个随机数作废。

### 4.4 会话和清单

`POST /v1/upgrade/sessions`（只要令牌，空请求体）：

```json
{ "session": "u20261113-031200-a1b2", "max_chunk_bytes": 8388608, "expires_at": "…" }
```

没开升级回 403 `upgrade_disabled`；这台不是发布包装的回 409 `not_managed`；有升级类任务正在跑回 409 `busy`。新开一个会话，之前还没应用的会话就作废（状态 `superseded`），但已经传上来的内容留着，新会话接着用。会话 24 小时没动静就过期。

`PUT /v1/upgrade/sessions/<会话号>/manifest`（令牌 + 签名）。请求体：

```json
{
  "release": { "name": "…", "version": "…", "channel": "rc", "created_at": "…" },
  "components": [ { "id": "backend", "title": "后端", "version": "0.1.44", "path": "bin/aios-database.exe", "sha256": "…", "dirty": true, "bytes": 234799616 } ],
  "files": [ { "path": "bin/aios-database.exe", "bytes": 234799616, "sha256": "…" } ],
  "release_json": { "bytes": 71234, "sha256": "…" },
  "agent": { "kind": "headless", "version": "0.1.0", "bytes": 6100000, "sha256": "…" }
}
```

- `release`、`components`、`files` 照抄来源包的 `release.json`；`release_json` 是来源包 `release.json` 文件本身（装好以后原样放到 `<根>\release.json`，它的内容也要传）。
- `agent` 可以是 `null`。只升代理、不升包时，`release` 是 `null`、`components` 和 `files` 是空数组、`release_json` 是 `null`。
- 代理收到后：验签 → 路径把关（5.1）→ 看渠道和 `dirty` 收不收 → 逐个文件判（5.2）→ 算磁盘够不够 → 回会话状态。

应答（`GET /v1/upgrade/sessions/<会话号>` 回同一个东西，续传时用）：

```json
{
  "session": "u20261113-031200-a1b2",
  "state": "open",
  "missing": [ { "sha256": "…", "bytes": 234799616, "have": 0 } ],
  "missing_bytes": 234799616,
  "plan": {
    "add": 2, "replace": 5, "delete": 1,
    "kept_modified": [ "www/config/model-display.config.json" ],
    "conflicts": [ "config/DbOption.factory.toml" ],
    "components_changed": [ "backend", "plant3d-web" ],
    "needs_stop": true,
    "agent_changes": true,
    "disk_needed_bytes": 300000000,
    "disk_free_bytes": 134000000000
  },
  "problems": []
}
```

- `state`：`open`（清单收下了，内容还没齐）、`ready`（内容齐了，可以下任务）、`applying`、`done`、`failed`、`superseded`、`expired`。
- `missing`：还要传的内容，按内容去重；`have` 是已经收到的字节数。**服务器上任何地方已经有的内容不算缺**：磁盘上别的路径有同样 sha256 的文件，或者以前传过。
- `plan`：照 5.2 判出来的结果，面板拿它和自己算的预览对一下，不一样以代理的为准并提示。
- 清单被拒时回 4xx 和错误码，不建 `plan`。

### 4.5 传内容：`PUT /v1/upgrade/sessions/<会话号>/blobs/<sha256>?offset=<n>`

只要令牌。请求体是原始字节，长度不超过 `max_chunk_bytes`；只有这一条路由的请求体上限是 8 MiB，别的照旧 1 MiB。没带对令牌的请求，代理不读它的体就回 401。

```json
{ "sha256": "…", "have": 16777216, "complete": false }
```

- `offset` 必须等于代理已经收到的字节数，否则回 409 `offset_mismatch`，体里带 `have`，客户端从 `have` 接着传。断了以后先 `GET` 会话状态看 `have`，再接着传。
- 收齐（`have` 等于清单里的字节数）时代理算 sha256：对了回 `complete: true`；不对就删掉这份、回 422 `hash_mismatch`（`have` 归零），客户端最多重传 3 次。
- sha256 不在这个会话的清单里回 404 `not_in_manifest`；超过清单里的字节数回 400。
- 可选的压缩：请求带 `Content-Encoding: deflate` 时体是压过的，`offset` 和 `have` 仍按解压后的字节算。U7 量到「传 224 MB 超过 2 分钟」才做；做了加能力 `upgrade.deflate`。

### 4.6 任务

`POST /v1/jobs` 的 `kind` 加三个：`upgrade`、`rollback`、`upgrade_agent`。都算危险操作（要 `confirm: true`），都要 4.3 的签名。`args` 加一个字段 `upgrade`：

```json
{
  "kind": "upgrade",
  "confirm": true,
  "note": "",
  "args": { "upgrade": {
    "session": "u20261113-031200-a1b2",
    "wait_idle_secs": 300,
    "allow_storage_rebuild": false,
    "ignore_unhealthy": false,
    "overwrite_modified": []
  } }
}
```

- `session`：`upgrade`、`upgrade_agent` 必填，会话要在 `ready`；`rollback` 不填。
- `wait_idle_secs`：等后端空下来最多等多久，缺省 300。
- `allow_storage_rebuild`：新后端起来后库版本对不上，也留下新版（之后照现有的「升级数据库」走）。缺省 false = 当作没过、换回。
- `ignore_unhealthy`：升级前就有启用的服务不正常，也照样升。缺省 false = 不动手。
- `overwrite_modified`：点名要覆盖的「本机改过」的路径。缺省不覆盖任何一个。

任务的 `progress.step` 依次是：`等后端空下来`、`停服务`、`换文件`、`启动`、`检查`，换回时是 `换回`；`progress.done / total` 在「换文件」这一步是文件数。任务输出（`/v1/jobs/<id>/output`）里每一步一行人话，带用时。任务的叫法（`label`）：`升级`、`回到上一版`、`升级代理`。

任务结束时 `result.upgrade` 是一份升级报告：

```json
{
  "id": "0003-u20261113-031200-a1b2",
  "kind": "upgrade",
  "session": "u20261113-031200-a1b2",
  "outcome": "ok",
  "from": { "name": "…-r6", "version": "…", "channel": "rc", "created_at": "…" },
  "to":   { "name": "…-r7", "version": "…", "channel": "rc", "created_at": "…" },
  "started_at": "…", "finished_at": "…",
  "stopped_services": true,
  "downtime_ms": 58200,
  "files": { "added": 2, "replaced": 5, "deleted": 1 },
  "kept_modified": [ "www/config/model-display.config.json" ],
  "conflicts": [ "config/DbOption.factory.toml" ],
  "failed_step": "",
  "message": "升级完成：换了 8 个文件，服务停了 58 秒",
  "rollback": null,
  "by": "<计算机名>\\<用户名>",
  "from_ip": "<客户端地址>",
  "key_id": "3fa9c2e1"
}
```

- `outcome`：`ok`、`nothing_to_do`（没有要换的）、`rolled_back`（没过，已经换回并恢复）、`failed`（没过，而且没能恢复成升级前的样子，要人处理）。
- `failed_step`：空串，或 `preflight`、`wait_idle`、`down`、`swap`、`up`、`check`、`storage`、`agent_trial`、`agent_handover`。
- `downtime_ms`：从开始停到检查通过；没停服务是 `null`。
- `rollback`：自动换回过才有，`{ "ok": true, "message": "…" }`。
- 任务的 `exit_code`：`ok`、`nothing_to_do` 是 0，`rolled_back` 是 1，`failed` 是 2。

### 4.7 历史：`GET /v1/upgrade/history?limit=10`

`{ "items": [ 升级报告, … ] }`，新的在前。只要令牌。

### 4.8 错误码（加进 `code`）

| 状态 | 错误码 | 什么时候 |
|---|---|---|
| 403 | `upgrade_disabled` | 这台没生成升级密钥 |
| 403 | `upgrade_key_mismatch` | `X-Upgrade-Key-Id` 不是这台的 |
| 403 | `upgrade_nonce` | 随机数不认识、用过了、过期了 |
| 403 | `upgrade_signature` | 签名不对，或者三个头没带全 |
| 403 | `channel_refused` | 这台不收这个渠道，或不收带 `dirty` 的包 |
| 400 | `bad_manifest` | 路径没过把关、两项路径相同、字段缺 |
| 409 | `not_managed` | 根下没有读得懂的 `release.json` |
| 409 | `session_state` | 会话不在这个请求要的状态（没交清单就传内容、内容没齐就下任务、会话已作废） |
| 404 | `not_in_manifest` | 这份内容不在会话的清单里 |
| 409 | `offset_mismatch` | 偏移和已收到的字节数对不上（体里带 `have`） |
| 422 | `hash_mismatch` | 收齐了但 sha256 不对 |
| 507 | `disk_full` | 磁盘不够（消息里写差多少） |
| 409 | `nothing_to_rollback` | 没有能换回去的备份 |
| 409 | `agent_kind_mismatch` | 清单里代理 exe 的种类和正在跑的不一样 |

## 5. 代理一侧

### 5.1 路径把关

清单里的路径一律是相对路径、用 `/` 分隔。下面任何一条不满足，整份清单拒绝（`bad_manifest`，消息里点名第一个不合格的路径）：

- 不是绝对路径，不带盘符、不带 `:`、不以 `/` 或 `\` 开头，不含 `\`；
- 每一段都不是空的、不是 `.`、不是 `..`，不以空格或点结尾，不含控制字符和 `< > " | ? *`；
- 每一段去掉扩展名后不是设备名（`CON`、`PRN`、`AUX`、`NUL`、`COM1`–`COM9`、`LPT1`–`LPT9`，不分大小写）；
- 整条不超过 240 个字符；
- 不是 `stack.toml`、`release.json`，不在 `run/`、`logs/`、`data/`、`work/`、`tmp/` 下面（不分大小写）；
- 清单里没有两项「不分大小写时相同」的路径。

动手的时候再查一次：目标路径上已经存在的每一级目录都不能是联接点或符号链接（reparse point），是就整个任务失败、不动手。这条防的是有人在根下面放一个指到别处的联接点。

### 5.2 逐个文件怎么判

记号：旧 = 已装清单里这个路径的 sha256；新 = 新清单里的；盘 = 磁盘上实际的。

| 路径在哪 | 磁盘上 | 判成 | 做什么 |
|---|---|---|---|
| 新清单里 | 盘 = 新 | 相同 | 不动 |
| 新清单里 | 没有这个文件 | `add` | 放上去 |
| 新清单里 | 盘 = 旧 | `replace` | 旧的挪进备份，新的放上去 |
| 新清单里 | 盘 ≠ 旧（本机改过），且 新 = 旧 | 留着（`kept_modified`） | 不动。包没改它，是本机改的 |
| 新清单里 | 盘 ≠ 旧（本机改过或已装清单里没有它），且 新 ≠ 旧 | 冲突（`conflicts`） | 留着本机的；新版另存到这次应用的 `conflicts\<路径>` |
| 只在旧清单里 | 盘 = 旧 | `delete` | 挪进备份 |
| 只在旧清单里 | 盘 ≠ 旧 | 留着（`kept_modified`） | 不动 |
| 只在旧清单里 | 没有这个文件 | — | 不动 |

冲突的路径如果在任务的 `overwrite_modified` 里，按 `replace` 办。

`release.json` 不在清单的 `files` 里，单独处理：所有文件换完之后，旧的挪进备份、新的放上去。留着本机版本的那些文件，下次看现状时仍然是 `modified`，这是对的。

### 5.3 要不要停服务

要换（`add` / `replace` / `delete`）的路径全都在 `web/` 或 `www/` 下面，或者是根下的 `.md`、`.txt`：不停服务，直接换。只要有一个在别处（`bin/`、`schema/`、`config/`、`resource/`、`rs-server/`、`stack.ps1`、各个 `.cmd`……）：整套停了再换。

升级前服务本来就全停着：不停也不起，换完就算完，报告里写「服务原来就停着，没有起，也没有检查」。

### 5.4 应用的步骤（`upgrade` 任务）

1. **看一眼**（`preflight`）：会话 `ready`；重新判一遍文件（5.2），和交清单时不一样了（有人又动了文件）就以现在的为准；路径上没有联接点；磁盘够；stack 的锁没人占。服务在跑但有启用的服务不正常：`ignore_unhealthy` 没给就到此为止，什么都不动。没有要换的：`nothing_to_do`。
2. 记下「服务在不在跑」「增量同步开没开」，写日志的第一行。
3. 要停服务时：**先关增量同步，再等后端空下来**（后端在跑和排队的任务都是 0），最多等 `wait_idle_secs`。到点还在忙：把增量同步恢复成原样，任务失败（`wait_idle`），什么都没动。等的这段时间任务可以取消；过了这一步就不能取消。
4. **停**：`down`（不带强制）。退出码不是 0：恢复增量同步，任务失败（`down`），什么都没换。
5. **换文件**：照 5.5 一个一个来。某个文件挪不动（被占用）时隔 2 秒再试，最多 5 次，还不行就走换回。
6. **起**：`up`。退出码不是 0 走换回。
7. **检查**，最多等 `check_secs`（缺省 90）：
   - 每个启用的服务都在跑并通过检查，后端自报健康是 `ok`，连续两份快照都满足；
   - 库版本对得上（后端健康里 `storage_caught_up` 是真，rs-server 没有因为库版本停在等待）。对不上且没给 `allow_storage_rebuild`：走换回，`failed_step` 是 `storage`。
8. 检查通过：日志写 `commit`；把增量同步恢复成升级前的样子；写报告和历史；清理（5.8）。

不停服务的那种（5.3）：没有第 3、4、6 步；第 7 步只看「原来正常的服务现在还正常」。

**换回**（任何一步说「走换回」）：服务有在跑的就先 `down`（带强制）→ 照日志倒着把文件换回去（5.6）→ 升级前服务在跑就 `up` → 再检查一遍 → 恢复增量同步。换回成功是 `rolled_back`；换回本身没成功是 `failed`，发一条错误通知「没能恢复成升级前的样子：<停在哪>」，日志留着不删。

### 5.5 换文件日志

每次应用一个目录 `<根>\run\gui\upgrade\applied\<序号>-<会话号>\`，里面的 `journal.jsonl` 一行一个 JSON。**先写一行并落盘（fsync），再做这一行说的事。**

```
{"n":1,"at":"…","op":"begin","kind":"upgrade","session":"…","from":{…},"to":{…},"services_running":true,"queue_paused":false,"needs_stop":true}
{"n":2,"at":"…","op":"stopped"}
{"n":3,"at":"…","op":"backup","path":"bin/aios-database.exe","sha256":"<旧的>"}
{"n":4,"at":"…","op":"place","path":"bin/aios-database.exe","sha256":"<新的>"}
{"n":5,"at":"…","op":"backup","path":"www/old.js","sha256":"<旧的>"}
{"n":6,"at":"…","op":"conflict","path":"config/DbOption.factory.toml","sha256":"<新的>"}
{"n":7,"at":"…","op":"release_json","old_sha256":"…","new_sha256":"…"}
{"n":8,"at":"…","op":"swapped"}
{"n":9,"at":"…","op":"started"}
{"n":10,"at":"…","op":"commit"}
```

没过时接着写：

```
{"n":…,"op":"rollback_begin","why":"up 退出码 4"}
{"n":…,"op":"rolled_back","ok":true}
```

- `backup`：把 `<根>\<路径>` 改名挪到 `backup\<路径>`。`place`：把内容放到 `<根>\<路径>`（从内容库改名挪过来；同一份内容要放到几个路径时，前几个复制、最后一个挪）。`delete` 就是只有 `backup`、后面没有同路径的 `place`。
- `conflict`：把新版内容存到 `conflicts\<路径>`，不动根下的文件。
- 备份、内容库都在根下面，和根在同一个盘上，所以挪文件是改名，不费时间也不多占空间。
- 一份日志的最后一行是 `commit`，或 `rolled_back` 且 `ok` 为真，这次应用才算有了结果。

### 5.6 照日志换回，和代理中途死掉

**换回的做法**（自动换回、崩溃恢复共用，可以重复做，做几遍结果一样）：把日志从后往前读——

- `release_json`：备份里有旧的 `release.json` 就放回去；
- `place <路径>`：根下那个文件的 sha256 是这一行记的新值，就把它挪回内容库（下次重试不用重传）；不是就不动它；
- `backup <路径>`：`backup\<路径>` 在，就挪回 `<根>\<路径>`；不在说明当时没来得及挪，跳过；
- `conflict`：不用管。

**代理起来时**：扫一遍 `applied\*\journal.jsonl`，发现没有结果的（最后一行既不是 `commit` 也不是成功的 `rolled_back`）：

- 是 `upgrade` 的日志：照上面换回；`begin` 里记着服务原来在跑就 `up`、再检查；恢复增量同步；报告记成 `rolled_back`，消息写「代理中途退出，已换回」；发一条通知。
- 是 `rollback`（5.7）的日志：接着把换回做完，不往回走。

做这件事的时候代理照常应答，但升级类任务回 409 `busy`。

### 5.7 「回到上一版」（`rollback` 任务）

把最近一次成功的 `upgrade` 换下来的文件换回去。步骤和 5.4 一样（等空闲、停、换、起、检查），「换」那一步是照那次的日志倒着做（5.6），它自己另记一份日志（`kind` 是 `rollback`）。

- 那次升级放上去的文件之后又被本机改过：不动它，记进 `kept_modified`。
- 换回之后检查没过：停在旧版，不再换来换去，`outcome` 是 `failed`，发错误通知。
- 换回成功后，那份备份就用掉了；更早还有一份成功的升级，`can_rollback` 才仍然是真。被换下来的新版内容留在内容库里，再升回去不用重传。
- 没有能换回的备份：409 `nothing_to_rollback`。

### 5.8 落盘布局和保留

```
<根>\run\gui\upgrade.key                 升级密钥（文件权限同令牌文件：只给当前账户）
<根>\run\gui\upgrade\
    blobs\<sha256>                       收齐并核对过的内容（跨会话共用）
    blobs\<sha256>.part                  没收完的
    sessions\<会话号>\manifest.json      收下的清单原文
    sessions\<会话号>\state.json
    applied\<序号>-<会话号>\
        journal.jsonl
        backup\<路径>                    换下来的旧文件
        conflicts\<路径>                 因为本机改过而没放上去的新版
        report.json
    history.jsonl                        每次升级、换回一行（就是报告）
    inventory-cache.json                 路径、大小、改动时间 → sha256
```

- 备份留最近 `keep_backups`（缺省 2）次成功的升级，更早的连目录删掉。
- 内容库里超过 7 天、又不属于任何没结束的会话的内容删掉。
- 结束了的会话目录留 7 天。

### 5.9 配置和子命令

`<根>\run\gui\agent.toml` 加一段，都可以不写：

```toml
[upgrade]
accept_channels = []     # 空 = 什么渠道的包都收；正式服务器写 ["release"]
accept_dirty = true      # false = 清单里有部件标着 dirty（没提交的代码编的）就不收
keep_backups = 2
check_secs = 90
```

开没开只看密钥文件在不在。两种代理身份都加一个子命令（写法照 `show-token` / `rotate-token`）：

```
<代理 exe> upgrade-key --root <根>             没有就生成；把密钥打印一次到标准输出
<代理 exe> upgrade-key --root <根> --rotate    换一把（旧的立刻作废）
<代理 exe> upgrade-key --root <根> --disable   删掉密钥文件 = 关掉升级
<代理 exe> upgrade-key --root <根> --id        只打印 key_id（不泄露密钥，核对用）
```

密钥改了不用重启代理（每次验签时读文件，或者看文件改动时间）。审计里：开会话、交清单（收下或拒绝及原因）、每个升级类任务，各一行，带 `key_id`、从哪个包到哪个包、来源地址、`X-Client`；不带密钥、不带签名。

### 5.10 mock 身份

mock 身份下升级全流程都能走：文件是真换的（在 mock 的根目录下），起停是模拟的。给面板自检和单测用的开关，写在 `agent.toml`（只在 mock 下有效，运行中改了下一个任务就生效）：

```toml
mock_upgrade_fail = ""     # 或 "down" / "swap" / "up" / "check" / "storage"：让这一步没过
mock_upgrade_busy_secs = 0 # 假装后端要忙这么多秒才空下来
```

mock 根目录下要先有一份小的假包（十几个文件、带 `release.json`），`deploy-agent-dev` 给一个参数生成它。

## 6. 代理自更新（`upgrade_agent` 任务）

换的是「正在跑的这个代理 exe 文件」。会话的清单里要有 `agent`，`kind` 要和正在跑的一样。

1. 把内容复制到 exe 所在目录，叫 `<exe 名>.new`，再算一遍 sha256。
2. **试跑**：`<exe 名>.new --version`，10 秒内退出码 0、输出里的版本和清单一致。不过：删掉 `.new`，任务失败（`agent_trial`），什么都没变。
3. 写交接单 `<根>\run\gui\upgrade\handover.json`（任务号、旧 exe 的 sha256、新版本、监听地址、时刻）。
4. 把正在跑的 exe 改名成 `<exe 名>.old`（Windows 允许给正在跑的 exe 改名），把 `.new` 改名成原来的名字。
5. 用**和自己一样的命令行参数**再加 `--takeover <自己的进程号>` 起新 exe。新进程不能挂在会跟着旧进程一起结束的作业上，优先级跟旧进程一样。
6. 旧进程：不再接新连接，放开监听端口和单实例互斥量，但先不退出；每秒问一次新进程的 `/v1/ping`，最多 20 秒。
   - 应答了、`agent_version` 和实例号都是新的：旧进程退出。
   - 没等到：结束新进程，把两个文件名换回来，重新占互斥量和端口，接着干；任务失败（`agent_handover`），`outcome` 是 `rolled_back`，发通知。
7. 新进程带着 `--takeover` 起来：等旧进程放开端口和互斥量（最多 10 秒），开始应答；读交接单，把那个任务记成成功（报告里 `from` / `to` 是代理的版本，放在 `message` 里也行），删交接单。`<exe 名>.old` 在下次起来时删。

服务不受影响：代理退出时服务一律留着，这是现成的行为。面板那头会看到断一下再连上、实例号和版本变了。

通用的部分（复制、试跑、改名、起新进程、看护、换回、交接单）放在 `deploy-agent` 库里；两个 exe 的入口各自接上 `--takeover` 参数。

## 7. 面板一侧

### 7.1 来源包和摆包

面板配置（`panel.toml`）加一段：

```toml
[upgrade]
source_dir = ""          # 来源包目录（里面有 release.json）
stage_spec = ""          # 摆包说明文件（New-StackRelease.ps1 的 -Spec）
pre_command = ""         # 摆包前先跑的一条命令（比如你自己的编译脚本），可空
parallel_uploads = 2
```

「重新摆包」：有 `pre_command` 先跑它；再调 `scripts\stack\New-StackRelease.ps1 -Spec <stage_spec> -OutDir <source_dir 的上一级> -NoZip -Force`（这个脚本属于主线，只调用、不改；参数以脚本现在的为准）。两步的输出都显示在升级抽屉里，退出码不是 0 就停下并显示最后几行。脚本的确切用法由 agent-client 读脚本定，写进 `UPGRADE-API.md`。

**配套的代理 exe**：面板 exe 旁边的 `deploy-dash-agent.exe`。清单里的 `agent` 就是它（种类 `headless`）。服务器上跑的是 `panel` 种类的代理（面板 exe 自己的 `--agent` 身份）时，这一版不给它升代理，预览里写「这台的代理就是面板程序，跟面板一起换」。

### 7.2 一轮升级怎么编排（数据层）

每台服务器一个状态：`没看过 → 预览好了 → 传内容中(已传/总共) → 等轮到它 → 应用中(哪一步) → 结果(成功 / 已换回 / 失败 / 跳过及原因)`。

- **预览**：拿现状（4.2，带 `files=1`）和来源包的清单在本机算。每台给出：要换的部件、要传的字节数（按内容去重）、要不要停服务、本机改过的文件、以及这些标记里中了哪些：在忙、代理不支持升级、没开升级、没配升级密钥、密钥对不上（`key_id` 不一样）、不收这个渠道、不是发布包装的、来源包比这台装的还旧（按 `created_at`）。带标记的那几台缺省不勾。
- **传**：最多 `parallel_uploads` 台同时传。块的大小从 1 MiB 起，一块用时不到 1 秒就加倍，最大到代理给的 `max_chunk_bytes`；超时就减半。断了自动接着传。
- **应用**：按列表顺序一台一台来，同一时刻只有一台在应用。一台结果不是成功：后面的都不动，记成「跳过：前面那台没成」。
- **先升第一台**：勾着时，第一台成功以后停下来等人点「继续」。命令行的 `--canary` 是第一台成功后等 60 秒、再看一眼它的现状仍然正常才继续。
- **代理**：包升完以后，这台的代理 exe 和配套的不一样、又是 `headless` 种类，就接着下 `upgrade_agent`。代理没升成不算这台失败，但结果里要写出来。
- 面板中途关了或断了：代理那头照样做完。面板再连上时读任务和历史，把结果补上。

数据层给界面用的类型和函数由 agent-client 先出一份骨架（能编过，函数体可以先空着），写成 `remote\agent-client\UPGRADE-API.md`，fleet-ui 照它接。

### 7.3 存档

`servers.toml` 每台多两项：`upgrade_key`（和令牌一样按当前 Windows 用户加密存）、`auto_upgrade`（缺省 false）。老存档没有这两项照样读得了；新版写出的存档老面板也读得了。

### 7.4 界面

- 汇总条上「升级…」。服务器行上的版本徽标：`最新` / `落后 N 个部件` / `不支持`（代理太老）/ `没开升级` / `不是发布包`；本机改过文件时加一个小标「改过 M 个文件」。还没配来源包时不显示徽标。
- 升级抽屉：
  - 来源包（目录、包名、摆包时刻、「重新摆包」、摆包输出）；
  - 每台一行：勾选框、现在装的 → 要装的、要换的部件、要传多少、要不要停服务、标记；点开看要换的文件和本机改过的文件；
  - 选项：先升第一台（缺省勾着）、新版要重建库也照装（缺省不勾）；
  - 「开始升级」要二次确认，确认框里写清「N 台、其中 M 台要停服务约 1 分钟」；
  - 进行中：每台的进度（传了多少、应用到哪一步）；
  - 结果：每台一句话（成功 / 已换回，为什么 / 失败，停在哪），冲突的文件点名；
  - 每台的「回到上一版」（`can_rollback` 为真才有，要二次确认）。
- 服务器抽屉里：填升级密钥的地方（只显示 `key_id`，不回显密钥）、「自动升级」开关。
- 升级类任务在主界面的任务区照常显示，叫法见 4.6。

### 7.5 命令行

```
deploy-dash.exe upgrade --source <目录> (--all | --servers <名字,名字>)
                        [--canary] [--yes] [--json] [--no-agent]
                        [--allow-storage-rebuild] [--watch] [--store <存档目录>]
```

- 不带 `--yes`：打印预览，问一句 y/N。带 `--json`：结果是一份 JSON（每台一项，带升级报告），别的不往标准输出写。
- 退出码：0 都成功或没有要升的；1 有没成功的（已换回或失败）；2 用法不对或来源包读不了；3 没有失败，但有跳过的（在忙、没开升级等）。
- `--watch`：见 7.6。`--store`：用另一个存档目录（测试用，不碰用户自己的存档）。

### 7.6 自动

面板开着、或者 `upgrade --watch` 在跑时：每 5 秒看一眼来源包的 `release.json`。它变了，而且 30 秒内没再变、清单里的文件都在并且大小都对，就给标了「自动」的服务器升级：

- 一台一台来，不等人点「继续」；一台没成，这一版对后面的都不再自动试，发通知。
- 在忙的那台跳过，2 分钟后再试，最多试 30 分钟。
- 这一版在某台上没装成（已换回或失败）：不再对它自动试这一版，等人来看。面板记住每台「最近自动试过的是哪一版、结果如何」。
- 没标「自动」的、没配升级密钥的不动。

## 8. 失败处理表

| 情形 | 代理 / 面板怎么办 | 之后这台是什么样 | 用户看到什么 |
|---|---|---|---|
| 传到一半网断了 | 面板重连后问 `have`，接着传 | 没变 | 进度停一下再接着走 |
| 收齐了 sha256 不对 | 代理删掉这份；面板重传，最多 3 次 | 没变 | 第 3 次还不对：这台「跳过：内容校验不过」 |
| 没开升级、密钥没配或对不上、签名不对 | 代理拒绝 | 没变 | 预览里的标记；硬下任务时报错误码对应的一句话 |
| 这台不收这个渠道 / 不收 dirty | 代理拒绝清单 | 没变 | 「这台只收 release 渠道的包」 |
| 清单路径越界 | 代理整份拒绝 | 没变 | 报出第一个不合格的路径 |
| 磁盘不够 | 代理拒绝清单 | 没变 | 「差多少」 |
| 后端一直在忙 | 等到 `wait_idle_secs`，恢复增量同步，任务失败 | 没变 | 「后端在忙，没动手」；自动模式过一会儿再试 |
| stack 的锁被别人占着 | 同上 | 没变 | 「有人在这台上跑 stack <命令>」 |
| 升级前就有服务不正常 | 不动手（除非 `ignore_unhealthy`） | 没变 | 点名哪个服务 |
| `down` 没成 | 恢复增量同步，任务失败 | 没变（没换任何文件） | 「停不下来：<原因>」 |
| 某个文件挪不动 | 重试 5 次，不行就换回、起服务 | 旧版，服务恢复 | 「已换回：<路径> 被占用」 |
| `up` 没成、检查超时 | 换回、起服务、再检查 | 旧版，服务恢复 | 「已换回：<哪个服务> 没起来」 |
| 库版本对不上 | 没给 `allow_storage_rebuild` 就换回 | 旧版，服务恢复 | 「这一版要重建库，没装；要装请勾上」 |
| 换回本身没成 | 停手，日志留着，发错误通知；代理下次起来再试一次 | 可能停着，要人处理 | 「没能恢复成升级前的样子：<停在哪>」 |
| 代理中途被结束、机器重启 | 代理下次起来照日志换回、恢复服务 | 旧版，服务恢复 | 历史里一条「代理中途退出，已换回」 |
| 面板中途关了、断了 | 代理照样做完 | 看任务结果 | 面板再连上时补上结果 |
| 两个面板同时来升 | 后来的回 409 `busy` | 先来的那个做完的样子 | 「另一个任务在跑」 |
| 代理新 exe 试跑不过 | 删掉 `.new` | 没变 | 「新代理起不来，没换」 |
| 新代理 20 秒内没应答 | 旧代理把文件名换回、接着干 | 旧代理 | 断一下再连上，「代理没换成」 |

## 9. 和计划的出入

1. **检查不另跑 `stack smoke`。** 计划写的是「`stack smoke` 0 级」。那一级查的是「每个服务在跑并通过检查、后端自报健康」，这些代理的快照里都有，所以直接读快照、连续两份都满足才算过。少跑一条命令，mock 也能走同一条路。
2. **下任务也要签名。** 计划只说清单签名。只给清单签名的话，光有令牌的人可以挑一个时刻触发已经传好的升级或换回，所以三种任务也要签。
3. **本机改过的文件不覆盖。** 计划只说预览里标出来。清单里有 `config/DbOption.factory.toml`，服务器上的库口令就写在它里面，所以定成缺省保留、点名才覆盖（5.2）。
4. **服务器上已经有的内容不重传。** 计划说「只传服务器上没有的内容」，这里把「有」定得更宽：磁盘上任何路径有同样的内容都算（4.4）。
5. **升级前服务不正常就不动手**（5.4 第 1 步），计划没写。不然升完检查不过、换回之后还是不过，分不清是谁的问题。
6. **debug 版里给 sha2 开优化。** 交付的 exe 一直是 debug 版，不开的话 600 MB 要算半分钟。在 `spike\Cargo.toml` 的 `[profile.dev.package]` 加 `sha2 = { opt-level = 3 }`（由 agent-server 加；会让依赖它的包重编一次）。

## 10. 分工、顺序、验收

板：`remote\UPGRADE-BOARD.md`（规矩、端口、进度都在那里）。老板 `BOARD.md` 不再往里写升级的事。

| 代号 | 活 | 能改的文件 |
|---|---|---|
| `agent-server` | U0 的协议类型和夹具；U1 传输；U2 应用与换回；U5 自更新的库里那部分；mock 支持（5.10） | `deploy-dash-proto\**`、`deploy-agent\**`、`spike\Cargo.toml` 里 sha2 那一行 |
| `agent-client` | 把写了一半的「只看」驱动关到特性开关后面；U3 数据层和命令行；U5 两个入口接 `--takeover` 和 `upgrade-key`；U6 的数据层；U7 的本机两套沙盒和云服务器实测、安装说明 | `deploy-dash\src\data\upgrade.rs`（新）、`data\mod.rs`、`data\remote.rs`、`data\stack.rs`、`agent_adapter.rs`、`headless_agent.rs`、`watch.rs`、`main.rs`、`http.rs`、`deploy-dash\Cargo.toml` |
| `fleet-ui` | U4 界面；存档两项；U6 的开关和触发；界面自检场景和截图 | `client_store.rs`、`fleet_state.rs`、`ui\upgrade.rs`（新）、`ui\fleet.rs`、`ui\sheets.rs`、`ui\header.rs`、`ui\mod.rs`、`app.rs`、`state.rs`、`selftest.rs` |

顺序：agent-server 先交「协议类型可编」（板上记一行），agent-client 和 fleet-ui 等这一行再用类型；agent-client 先交数据层骨架和 `UPGRADE-API.md`（板上记一行），fleet-ui 等这一行再接数据层。等的时候做不依赖别人的部分，不要空转。

验收（每条都要有实际跑出来的证据，写进各自的汇报）：

- **U1**：220 MB 的文件分块传完；中途断开能接着传；内容改一个字节被拒；没有密钥被拒；令牌对、签名不对被拒；用过的随机数被拒；路径把关的每一条各有一个单测。
- **U2**：沙盒里从旧包升到新包，服务都正常；放一个起不来的后端 exe，自动换回、服务恢复；换文件中途结束代理，重起后自己换回；本机改过的文件原样留着；「回到上一版」成功。
- **U3**：命令行对两台沙盒，第一台成功才升第二台；第一台失败就停，第二台没动。
- **U4**：用 agent-desktop 操作真实界面走完预览、升级、换回，截图逐张看过。
- **U5**：换成新版后面板自己重连、版本和实例号变了；放一个起不来的 exe，自动换回，断连不超过 30 秒。
- **U6**：重新摆包后 1 分钟内，标了「自动」的那台开始升级；没标的不动；在忙的跳过并稍后重试。
- **U7**：云服务器经隧道实测上传速度、停机时长、隧道中途断开后的续传；安装说明加「升级」一节。云服务器那一步等协调者发话再做。
