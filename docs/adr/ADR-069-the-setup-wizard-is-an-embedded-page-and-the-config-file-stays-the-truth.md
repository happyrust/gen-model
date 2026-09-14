# ADR-069：配置向导是内嵌单页 + 设置模式，配置真值仍是 DbOption.toml 文件

状态：**已接受，2026-09-11 增补 embedded-mem 向导档与 runtime 默认值**（配置真值、回环安全闸和保存重启协议不变。）
**2026-09-13 修订（ADR-074）**：向导第②步只剩 `mem`（默认，不用地址）/ `rocksdb`（连一台已在跑的库，须通过「测试连接」）两档，
`external` / `spawned-rocksdb` / `spawned-mem` / `embedded-mem` 四档退役；写口 `validate_patch` 只认新名，读旧配置时折到两档显示。
D3 的设置模式判据读作「`rocksdb` × 空地址 = 设置模式，`mem` 不看地址」；D5 / D8 与实施约束 2、5、7 里的四档措辞按此读。
「端口预检」按钮随 spawn 两档一起删除——服务不再在那个端口上建库，预检没有对象。
日期：2026-09-09
落地：`src/web_service/setup.rs`（setup API + 设置模式）、`web/setup.html`（向导页）、`src/lib.rs`（启动臂分支 + 单实例锁交接窗）、`scripts/templates/DbOption.factory.toml` + `scripts/New-LocalRelease.ps1`（出厂形态 + 冒烟两腿）。
关联：
- **修订 ADR-068 D3 的一句**：包内 `DbOption.toml` 从「本机快照原样带走、不改写」改为**出厂模板**（见 D5）。ADR-068 的核心边界（资源进包、库数据与源模型不进包）不动。
- **修订 ADR-068 D6 冒烟门**：从一腿改为两腿（见 D8）。
- 计划与 16 问决策表：`docs/plans/2026-09-09-setup-wizard-web-frontend.md`。
- CONTEXT.md 新增词条「配置向导 / 设置模式 / 首跑哨兵 / 引导子集」。

## 背景

-full 发布包解压到新机器，第一件事是改 `DbOption.toml`：`project_path` 是打包机的绝对路径、库地址指着打包机的 8010。而服务的启动顺序是「先连库、先解监控目录，后起 HTTP」——配置不对，进程死在任何网页能被送出去之前。用户要一个网页向导：一步步配置，然后初始化。

难点有三：向导页得在「配置还是坏的」时候就能被服务出来（鸡生蛋）；配置文件里的注释是决策史，网页写盘不能擦掉它；出厂默认监听全 0（局域网要能用应用），配置写口却绝不能顺带暴露给局域网。

## 决定

### D1　向导 = exe 内嵌单文件页 `/setup.html`，不进 plant-ui

与 `/ops.html` 同一套模式：`include_str!` 内嵌兜底、`ui_root` 磁盘副本优先（`x-setup-panel-source` 头说明用的哪份）、零构建、热替换。plant-ui 保持「建模客户端」本分；向导的初始化步骤是既有 API 的薄壳（D7），复杂场景仍指去 plant-ui。

### D2　配置真值只有一处：`DbOption.toml` 文件

HTTP（`GET/PUT /api/v1/setup/config`）只是它的读写口。写盘用 `toml_edit` 外科手术式改键：**注释与未知键原样保留**（注释是决策史；未知键可能属于别的版本），写前同目录落 `DbOption.toml.bak`。改的路径就是本进程在读的那份（`options::active_config_path`，与 `DB_OPTION_FILE` 候选口径逐字相同）——「向导写的」与「服务读的」不许是两个文件。

### D3　两个判据各管一件事（本 ADR 里唯一反直觉、最容易被改坏的地方）

- **设置模式（启动臂选择）= 配置无效**：`project_path` 缺 / 不存在，或 `store_mode=external` 且 `v_ip` 空 / `v_port=0`（哨兵值；这格是 `u16`，空串会毒死整份配置的解析）。命中即走最小启动臂：不连库、不扫描、不起 watcher / 队列，只绑 HTTP 供向导 + `/health`（`mode:"setup"`）。
- **首跑哨兵 `.first-run-pending`（跳转旗）= 向导没走完**：在场时 `/` 302 到 `/setup.html`；**不影响启动臂**。
- 为什么拆开：向导在 ④→⑤ 之间要重启一次，重启后配置已有效必须**正常启动**（⑤ 初始化要全服务）但向导还没完——哨兵若也决定启动臂，就死锁在设置模式里。
- **库地址填了但连不上不算配置无效**：那可能只是库没开机。照旧走正常启动失败退出，不把真故障盖成向导。

### D4　setup 面按 TCP 对端判回环；对外暴露须另加认证（硬边界）

出厂监听全 0（局域网用应用是需求），所以判据**不能**是监听地址（那会把包括本机在内的所有人拒掉），只能是 `ConnectInfo` 里的 TCP 对端：仅 `127.0.0.1` / `::1` 放行 `/api/v1/setup/*`，**不认 `X-Forwarded-For` 之类代理头**（伪造得了的东西不做判据）。`/setup.html` 页面本身不拦（无害），API 全拦；页面对 403 显示「请在服务器本机用 127.0.0.1 打开」。**将来若要让局域网也能进向导，必须另加认证（token / 账号），不许放宽对端判据**——这一条写死。

### D5　出厂形态：模板 + 哨兵（修订 ADR-068 D3）

出厂配置都令 `project_path=""`、`http_api_addr="0.0.0.0:9099"`、gen 全关、watch 空，并带 `.first-run-pending`。runtime 档由组包脚本写成 `store_mode="embedded-mem"`；带 workspace 档保留模板的 `spawned-mem`。向导提供 external / spawned-rocksdb / embedded-mem / spawned-mem 四档，embedded 不要求也不改写有效数据库端点；配置读回时必须原样选中，不能静默翻回 spawned-mem。

### D6　配置生效走「保存并重启」口，交接靠重试窗

`POST /api/v1/setup/restart`：202 → 300ms 后 spawn 同 exe / 同参数 / 同环境的子进程（带 `AIOS_RESTART_HANDOFF=1`）→ 父进程 `exit(0)` 一并交出端口与单实例锁。子进程在 ≤10s 窗口内对「端口被占」「锁被持」重试（`bind_with_handoff` / `open_lock_within_handoff_window`）；窗口只在环境变量在场时存在，平时绑不上就是失败，不悄悄重试。向导页轮询新地址（`0.0.0.0` 映射 `127.0.0.1`）等它回来。失败不静默：子进程拉不起来父进程继续跑并出声；轮询超时给「手动重跑 Start-AMS」文案。

`http_api_addr` 与 `http_public_addr` 分工固定：前者只决定 socket 监听，后者只生成日志、自动打开与分享链接。首跑与 restart 轮询不消费 public 地址，始终把全接口监听映射到回环地址，以满足 D4 的配置安全闸。正常模式可用 `0.0.0.0:9099` 监听并展示 `192.168.1.9:9099`，无需在“局域网可达”和“本机能配置”之间二选一。

### D7　初始化面不加新口

向导第 ⑤ 步全部复用既有 API：`/api/v1/dbnums`（`awaiting_manual_initialization` 名单 + 水位）、`/update/preview`、`/update/execute`（`dbnums[]` 子集，ADR-020）。「只导数据 / 一步到位」两档单选写回 `gen_model` / `gen_mesh` 随 ④ 保存。手动初始化的准入口径仍是 d-103（缺省不自动初始化新库），向导没有旁路。

### D8　冒烟门按发布档位分腿（修订 ADR-068 D6）

A 腿：该档出厂配置 + 哨兵起包内 exe，期待 `/health` 报 `mode:"setup"` 且 `/setup.html` 含向导标记。B 腿：本机真配置全启动；runtime 强制 `embedded-mem`，除 health 与首页外还要求没有 Surreal 子进程；带 workspace 档强制 `spawned-mem`。带 workspace 档另有 ADR-070 的 spawned-rocksdb C 腿。任一必需腿失败照旧标「未冒烟」，`-StrictSmoke` 下不出包。

## 后果

- 新机器解压 → `Start-AMS.cmd` → 浏览器自动落在向导：配置、验证、重启、初始化一条道走完；老部署（配置有效、无哨兵）行为一字不变。
- `Start-AMS.ps1` 的 `process.json` 在向导触发自重启后会过期（记录的还是父进程 pid）——已写进 release-notes，v1 不追。
- 本机经局域网 IP 访问 `/setup.html` 也会撞 403（对端不是回环）——文案引导换 `127.0.0.1`，属可接受边角。
- 「一键用包内 surreal 起持久库」不在本轮（Q5 拍的是指引文案档）；要做是新 store_mode 档或进程管理口，另开一轮拍板。

## 实施约束

1. 回环闸的判据只能是 TCP 对端（源码钉 `the_gate_never_reads_forwarded_headers`：签名只收 `SocketAddr`、模块不读请求头）。
2. 设置模式判据是纯函数 `setup_mode_reason_for`（真值表钉死：project_path 两态 × store 四档 × 地址空满）；embedded-mem 不需要端点。
3. 写盘承诺由 `config_writes_keep_comments_unknown_keys_and_integer_shapes` 与 `write_config_backs_up_before_touching_the_file` 钉死：注释 / 未知键保留、整数键不漂移成字符串、先备份后写、空补丁不碰盘。
4. `v_port` 的「未设置」哨兵是 `"0"`：任何路径（向导写盘、手改模板）都不得把空串写进这一格。
5. `spawned-mem` 必须带真实本机地址（向导默认 127.0.0.1:8009）；`v_port=0` 会让下次正常启动必然失败，写口在合并后拦截。embedded-mem 的 v_ip/v_port 被禁用且不参与判定；external 留空只会安全回设置模式。
