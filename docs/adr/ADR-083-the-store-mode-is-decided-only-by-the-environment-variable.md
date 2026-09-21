# ADR-083：存储介质只由环境变量 `AIOS_STORE_MODE` 决定——`store_mode` 退出配置文件与向导

状态：**已接受**（2026-09-18 用户拍板：「调整 store mode 的参数配置，取代 DbOption 里的参数控制，只允许环境变量控制」。
**号到提交时复核**——ADR-060 曾撞号两次。）
**同日修订（2026-09-18，用户拍板「走乙」）**：D6 的默认档由 `mem` 翻成 **`rocksdb`**，同时 D5 的 `Start-AMS.ps1`
在不给 `-StoreMode` 时**显式回填** `AIOS_STORE_MODE=mem`。改的只是「漏设时落到哪一档」，D1–D4 一个字没动。
初版落地当天就翻，是因为真机复验把两种漏法的代价量出来了——见改写后的 D6。
日期：2026-09-18
落地：`src/options.rs`（`store_mode()` 只读环境变量；`effective_store_mode(env)` 签名里没有配置那一格；
`store_mode_notices()` 逐条点名文件里残留的 `store_mode` / `in_memory_db` 与旧环境变量 `AIOS_IN_MEMORY_DB`；
`in_memory_db()` 删除；护栏 `every_db_option_fixture_declares_its_store_mode` 反转为 `no_db_option_fixture_still_carries_a_store_mode_key`）、
`src/lib.rs`（启动通告逐条打印；横幅带「这一档是谁定的」；mem 横幅的改法指到环境变量）、
`src/web_service/setup.rs`（`SetupConfigPatch.store_mode` 收下不写盘、不校验；`subset_values` 不含它；
`GET /api/v1/setup/config` 的 `effective` 多出 `store_mode_source` / `store_mode_env` / `store_mode_env_value` / `stale_config_store_mode`；
`ensure_completion_ready` 以 `options::store_mode()` 为准，不再从文件读）、
`web/setup.html`（第②步不再选介质，只展示生效档与出处、提醒文件里的过期键；地址两格随生效档启停）、
`src/web_service/review/db.rs`（拒绝启用的三处文案改法指到 `AIOS_STORE_MODE=rocksdb`）、
仓内全部 `DbOption*.toml` 夹具与 `scripts/templates/DbOption.factory.toml`（`store_mode = ...` 换成退役注释、记下原值）、
`scripts/New-LocalRelease.ps1`（生成的 `Start-AMS.ps1` 新增 `-StoreMode mem|rocksdb`、不给时回填 `mem`；冒烟 B / C 腿不再往配置里写 `store_mode`，只递环境变量；A 腿有意不设，考默认档下的设置模式逃生门）、`scripts/start-local-release.ps1`（老启动器同样显式回填 `mem`）、
`scripts/Start-Surreal8009.ps1` / `scripts/strt_plant_ui_runtime.py` / `src/bin/e3d_persist_smoke.rs`（改口）。
关联：
- **修订 ADR-074** D3（**默认由 `mem` 翻成 `rocksdb`**，见本文 D6；「`in_memory_db` 两态映射」作废——配置键不再参与）、D4（退役写法仍认，但只认**环境变量里**的退役写法；
  「向导只写新名」作废——向导一个字都不写）与实施约束 1、3（护栏反向；向导用例换成「只展示、不写」）。
- **修订 ADR-069**「配置真值仍是 `DbOption.toml` 文件」：介质是唯一例外——它不是配置文件里的真值，是启动命令里的真值。
  D3 设置模式判据不变（`rocksdb` × 空地址 = 设置模式，`mem` 不看地址），只是 `rocksdb` 与否由环境变量说。
- **修订 ADR-075** 的改法文案：「先起持久 SurrealDB、再把 `store_mode` 改为 rocksdb」读作「再以 `AIOS_STORE_MODE=rocksdb` 重启」。
- CONTEXT.md「存储形态」「设置模式」「配置向导」「校审持久层裁决」「本地发布包」词条随本文改写。

## 背景

2026-09-13（ADR-074）之后介质只剩 `mem` / `rocksdb` 两档，但**决定它的地方有三层**：环境变量 `AIOS_STORE_MODE` 压过
`DbOption.toml` 的 `store_mode`，`store_mode` 又压过更老的 `in_memory_db`（外加它自己的环境变量 `AIOS_IN_MEMORY_DB`）。
每一层单看都有道理，叠在一起就是一张四行的优先级表，而读配置的人只看得见其中一行。

2026-09-18 现场真踩了：`DbOption.toml` 写着 `store_mode = "rocksdb"`，上一条命令残留在会话里的 `AIOS_STORE_MODE=mem`
把它压成了内存库。服务照常起、模型照常对，只有属性行一直不进库（mem 派生 read-through，不做基线、不开数据窗口），
启动日志里没有一个字提过配置被压。人对着那句 `rocksdb` 反复怀疑增量窗口，查了半天。同一天先补了「环境变量压过配置要出声」
的告警和横幅上的「这一档是谁定的」——那是止血。用户随后拍板把根去掉：**介质只允许环境变量控制，配置文件不再参与。**

同一天的 Linux 部署也印证了这一点：服务器上的介质是 systemd 单元的事（`Environment=`），跟二进制怎么起走；渲染出来的
`DbOption.toml` 再写一遍 `store_mode`，只是给「两处要对账」多添一处。

## 决定

### D1　介质只由 `AIOS_STORE_MODE` 决定

`store_mode()` 只读环境变量 `AIOS_STORE_MODE`：`mem` / `rocksdb`（下划线、大小写、退役别名照 ADR-074 D4 认），不设或空白 = **`rocksdb`**（同日修订，见 D6），
认不出 = 当没设并出声。`effective_store_mode(env)` 的签名里**没有配置那一格**——配置文件想参与裁决，得先改这个签名，那就是在改本文。

介质是「这一次进程怎么起」的事，跟启动命令走；配置文件描述的是「这个工程长什么样」（项目路径、MDB、库地址、监听端口……），
它不该替进程挑介质。`v_ip:v_port` 留在文件里：它是「rocksdb 档连哪儿」，不是「要不要 rocksdb」。

### D2　配置键与旧环境变量只剩「点名」一个用途

`DbOption.toml` 的 `store_mode` / `in_memory_db` 与环境变量 `AIOS_IN_MEMORY_DB` **不再参与裁决**。`DbOptionExtFields` 里的两个字段
留着（旧配置照常解析），但唯一的消费者是 `store_mode_notices()`：文件里还写着，启动时逐条点名「这一行已不生效——介质只由
`AIOS_STORE_MODE` 决定；本进程按 X 跑（谁定的）」，文件里那档与实际跑的那档不一样时并排念出来、说清该设哪个环境变量。
`in_memory_db()` 函数删除（此前已无调用者）。

### D3　夹具与模板里不许再有这一行

护栏反转：`no_db_option_fixture_still_carries_a_store_mode_key` 扫仓根、`db_options/`、`scripts/templates/` 下全部 `DbOption*.toml`，
任何一份出现未注释的 `store_mode =` / `in_memory_db =` 即红。此前那条「每份夹具必须显式写 `store_mode`」（ADR-074 实施约束 1）
护的是「不许依赖会翻的默认」；现在默认是 `rocksdb`（D6 同日修订）且不再翻，而文件里那一行**必定**与某次实际启动相反，留着只会骗人。
每份夹具把原值记进注释（「此前写的是 rocksdb——配套脚本启动服务时要带 `AIOS_STORE_MODE=rocksdb`」），配套脚本本来就带着这个环境变量。

### D4　向导只展示、不选、不写

`web/setup.html` 第②步不再有两个 radio：页面读 `GET /api/v1/setup/config` 的 `effective.store_mode` / `store_mode_source`，
展示「当前介质 X，由 Y 决定」，说明改法是启动前设环境变量（`Start-AMS.ps1 -StoreMode rocksdb` / systemd `Environment=` / `$env:`），
文件里残留 `store_mode` 时用 `effective.stale_config_store_mode` 提醒删掉。`v_ip` / `v_port` 与「测试连接」保留，只在生效档是 `rocksdb`
时可用；第②步的闸不变（`rocksdb` 须通过测试连接）。`SetupConfigPatch.store_mode` 收下但**不写盘、不校验**（与 `included_projects`
同一处理：旧页面带过来不能把保存搞坏），`apply_patch` 里没有它。`ensure_completion_ready` 以 `options::store_mode()` 为准，
不再从文件文本里解析 `store_mode`。

### D5　发布包与部署脚本把介质递给环境变量

生成的 `Start-AMS.ps1` 新增 `-StoreMode mem|rocksdb`：给了就设 `AIOS_STORE_MODE` 再拉服务（cmd → 服务两层都继承）；不给时沿用调用方环境里已有的那一档，调用方也没有就**显式回填 `mem`**（同日修订：服务默认档翻成 rocksdb 之后，包不再靠默认兜「解压即跑」），
两处都没有 = `mem`；日志打一句 `STORE_MODE=…`。冒烟 B 腿本来就设 `$env:AIOS_STORE_MODE`，C 腿改为显式 `rocksdb`；两腿都不再往
临时配置里写 `store_mode`（本机配置若残留这一行，顺手删掉）。Linux 部署（`deploy/linux/install_backend.sh`）在 `aios-database`
单元里加 `Environment=AIOS_STORE_MODE=rocksdb`，`render_dboption.py` 不再渲染 `store_mode`——现役 0.1.27 二进制也认这个环境变量，
两版二进制同一套脚本。

### D6　漏设时落到 `rocksdb`（同日翻的，取代 ADR-074 D3 的 `mem`）

不设、设成空白、或设成认不出的值，一律落到 **`rocksdb`**。`data_face` 仍由介质派生（`rocksdb` → `ingest`，
`mem` → `read-through`，ADR-074 D6），所以漏设换掉的不只是介质，还有整个数据面。

翻的依据不是「哪一档更常用」，是**两种漏法的代价不对等**，这一点在本文落地当天的真机复验里量出来了：

| 漏设时兜底到 | 现象 | 多久发现 |
| --- | --- | --- |
| `mem`（旧） | 服务照常起、模型照常对，只有属性行一直不进库、校审域整域 503、重启即清空 | 2026-09-18 现场查了半天，而且是**先怀疑增量窗口**才找到这儿 |
| `rocksdb`（新） | 地址上没库就当场 `SurrealDB 连接失败（ws://…）` 退出（exit 1） | 一眼 |

默认值的作用就是兜住「人没说」。兜到一个会静默给出错误数据面的档上，等于把最贵的那类沉默设成了出厂值。

代价是「不配任何东西就能起来」（ADR-074 D3 原本的理由）不再由默认提供，而是由 D5 的 `Start-AMS.ps1` 显式回填 `mem` 提供：
发布包是唯一「机器上多半没有库」的场景，它自己说出这一档，不占用默认位。两处残余风险，接受：

1. 从包里**绕过 `Start-AMS.ps1`** 直接双击 `aios-database.exe`：出厂配置 `project_path` 为空 → 仍拐进设置模式、向导打得开
   （冒烟 A 腿有意不设介质，考的就是这条逃生门）；但**向导走完之后**再这样直起，就会去连 `v_ip:v_port` 然后失败。
2. 配置已填好、机器上没库的开发机手工直起：同样是启动失败。改法是 `$env:AIOS_STORE_MODE='mem'`，
   而且失败信息里就带着地址，不需要猜。仓内 `Start-AiosDatabaseManual.ps1` / `Run-*.ps1` / `Verify-*.ps1` / `l3_suite`
   本来就显式带着介质，一个都不受影响。

## 后果

- **凡是「起服务」的地方都得带环境变量**：仓内 `Start-AiosDatabaseManual.ps1` / `Run-*.ps1` / `Verify-RocksDb8000.ps1` /
  `l3_suite/fixture.rs` / `review/acceptance.ps1` 此前就带 `AIOS_STORE_MODE`，一个都不必改；只靠配置文件写 `rocksdb` 起服务的
  手工习惯要改——不改的话服务按 `mem` 起来，启动日志会用一整行点名文件里那句 `rocksdb` 已不生效并给出该设的值。
- 发布包用户要持久库：先起库、向导第②步填地址并测试连接、再 `Start-AMS.ps1 -StoreMode rocksdb`；向导里没有「改选 rocksdb」这一步了。
- `/health` 与 `/api/v1/setup/config` 报的介质与进程实际一致（它们本来就读 `store_mode()`）；`setup/config` 多出的四个 `effective.*`
  字段是加法，老客户端不受影响。
- 少了一张优先级表：`effective_store_mode` 从四参数变成一参数，`store_mode_source` 从五种答案变成两种。

## 实施约束

1. `the_default_store_mode_is_rocksdb`（不设 / 空白 / 拼错都落 rocksdb；`mem` 仍可显式选）、`only_the_environment_variable_decides_the_store_mode`（签名里没有配置那一格；`ROCKSDB` / ` Mem ` 都认）、
   `an_unrecognised_store_mode_is_ignored_but_never_silent`。
2. 点名：`a_stale_store_mode_key_in_the_config_is_named_not_swallowed`（文件说 rocksdb、实际 mem → 「两边不是同一档」+ `AIOS_STORE_MODE=rocksdb`；
   两边一样 → 仍说「不再生效、删掉」）、`the_legacy_in_memory_db_switch_is_named_not_swallowed`（配置键与旧环境变量各一条；四个过期来源同时在场 = 四条）、
   `a_retired_store_mode_still_parses_but_says_what_it_became`（改法指到环境变量）。
3. 护栏：`no_db_option_fixture_still_carries_a_store_mode_key`。
4. 向导：`the_wizard_shows_the_effective_store_mode_and_never_writes_it`（页面含 `effective-store` / `stale_config_store_mode` / `AIOS_STORE_MODE`；
   不含 `store_mode:`、`id="store-mem"`、`id="store-rocksdb"`、`storeTierOf`）；`write_config_never_writes_store_mode_and_accepts_a_blank_address`
   （只带 `store_mode` 的补丁 = 没改、不备份）；`patch_validation_rejects_malformed_values`（`store_mode` 拼错不算问题）。
   页面读哪几格与服务端发哪几格要**两头都钉**：`the_config_endpoint_reports_the_effective_store_mode_and_names_a_stale_key`
   钉住 `GET /api/v1/setup/config` 真发出那四格、与 `options::store_mode()` 同一个答案、过期键念原文且不混进 `values`
   （**已按负例验过**：把 `store_mode_source` 改名即红，失败信息里带整段 `effective`）。只钉页面那一头的话，改名字两条都不红，
   而页面拿到 `undefined`——介质栏画成默认的 `mem`、残留键的提醒整条消失，看上去像「文件里那行已经删了」。
5. 校审：拒绝启用的 503 正文含 `AIOS_STORE_MODE=rocksdb`（`the_refusal_router_answers_every_review_path_with_503_and_a_reason`）。
6. 服务端源码里不得再有 `options::in_memory_db()`；`effective_store_mode` 保持单参数。
