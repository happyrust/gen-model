# ADR-083：存储介质只由环境变量 `AIOS_STORE_MODE` 决定——`store_mode` 退出配置文件与向导

状态：**已接受**（2026-09-18 用户拍板：「调整 store mode 的参数配置，取代 DbOption 里的参数控制，只允许环境变量控制」。
**号到提交时复核**——ADR-060 曾撞号两次。）
**同日修订（2026-09-18，用户拍板「走乙」）**：D6 的默认档由 `mem` 翻成 **`rocksdb`**，同时 D5 的 `Start-AMS.ps1`
在不给 `-StoreMode` 时**显式回填** `AIOS_STORE_MODE=mem`。改的只是「漏设时落到哪一档」，D1–D4 一个字没动。
初版落地当天就翻，是因为真机复验把两种漏法的代价量出来了——见改写后的 D6。
**二次修订（2026-10，用户拍板「向导里可选 external / memory」）**：裁决恢复分层——
**`AIOS_STORE_MODE` > `DbOption.toml` 的 `store_mode` > 默认 `rocksdb`**。
`store_mode` 回到配置文件，但身份变了：不再是没人认领的残留键，而是**配置向导第②步持久化的用户选择**
（两档：external / memory，external 为默认）。向导恢复 radio 并把它写盘；向导触发的重启把文件里那一档
**显式**递给孩子进程的 `AIOS_STORE_MODE`，保证所选档生效；环境变量压过文件时 `store_mode_notices` 仍并排
点名「两边不是同一档」。`in_memory_db` 与 `AIOS_IN_MEMORY_DB` 维持退役，只留点名。`Start-AMS.ps1` 的回填
改为**文件优先**：文件写了 `store_mode` 就按它进环境变量，文件没写才回填 `mem`。离线包出厂
`DbOption.toml` 注入 `store_mode = "rocksdb"`（external 默认走文件层，不再靠 `deploy.env` pin）。
修订点：D1（签名里有配置那一格了）、D2（`store_mode` 恢复生效，`in_memory_db` 仍只点名）、
D3（夹具仍不许带——现在是**生效键**，预置=静默定介质，更不许）、D5（回填改为文件优先）。
日期：2026-09-18
落地：`src/options.rs`（`store_mode()` 只读环境变量；`effective_store_mode(env)` 签名里没有配置那一格；
`store_mode_notices()` 逐条点名文件里残留的 `store_mode` / `in_memory_db` 与旧环境变量 `AIOS_IN_MEMORY_DB`；
`in_memory_db()` 删除；护栏 `every_db_option_fixture_declares_its_store_mode` 反转为 `no_db_option_fixture_still_carries_a_store_mode_key`）、
`src/lib.rs`（启动通告逐条打印；横幅带「这一档是谁定的」；mem 横幅的改法指到环境变量）、
`src/web_service/setup.rs`（`SetupConfigPatch.store_mode` 收下不写盘、不校验；`subset_values` 不含它；
`GET /api/v1/setup/config` 的 `effective` 多出 `store_mode_source` / `store_mode_env` / `store_mode_env_value` / `stale_config_store_mode`；
`ensure_completion_ready` 以 `options::store_mode()` 为准，不再从文件读）、
`web/setup.html`（第②步不再选介质，只展示生效档与出处、提醒文件里的过期键；地址两格随生效档启停）、
**2026-10 修订再落地**：`effective_store_mode` 签名恢复配置那一格（env > file > 默认）；向导第②步恢复两档
radio（external / memory），选中档校验后写盘成规范值 `store_mode`；`spawn_replacement` 把文件档显式递给孩子
进程的 `AIOS_STORE_MODE`；`ensure_completion_ready` 按「下一进程跑哪档」文件优先判；`Start-AMS.ps1` 回填改为
文件优先；`subset_values` 收进 `store_mode`，`effective` 的 `stale_config_store_mode` 换成 `config_store_mode`；
`store_mode_notices` 对文件层的退役写法与「env 压文件」同样点名（2026-10-15 补齐文件层退役别名点名）。
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

### D1　介质三层裁决：`AIOS_STORE_MODE` > `store_mode` > 默认 `rocksdb`（2026-10 修订）

`store_mode()` = `effective_store_mode(env, file)`：`mem` / `rocksdb`（下划线、大小写、退役别名照 ADR-074 D4 认，
两层都在认），两层都没说或都认不出 = **`rocksdb`**（见 D6），认不出 = 当没设并出声。三层各管一件事：

- **`AIOS_STORE_MODE`**：启动命令那一层的显式覆盖——开发隔离与运维 pin（systemd `Environment=`）都走它，
  它在且认得出就以它为准；
- **`DbOption.toml` 的 `store_mode`**：配置向导第②步落盘的用户选择（external / memory 两档）。环境变量没说时
  按它跑；两边都在且不同档，`store_mode_notices` 启动时并排念出来——那是 2026-09-18 现场踩过的静默覆盖形状；
- **默认 `rocksdb`**：兜住「人没说」，落在会喊的那一侧。

介质仍是「这一次进程怎么起」的事；`store_mode` 那一格不是「配置文件替进程挑介质」，是**用户经向导显式存下的
那一档**，环境变量始终能盖它。`v_ip:v_port` 留在文件里：它是「rocksdb 档连哪儿」，不是「要不要 rocksdb」。

（2026-09-18 初版是「介质只由环境变量决定，`effective_store_mode(env)` 签名里没有配置那一格」；2026-10 用户拍板
「向导里可选 external / memory」，配置层恢复——`store_mode` 回到文件，但身份变成向导持久化的选择。）

### D2　`in_memory_db` 与旧环境变量只剩「点名」一个用途（2026-10 修订）

`DbOption.toml` 的 `in_memory_db` 与环境变量 `AIOS_IN_MEMORY_DB` **不参与裁决**：`DbOptionExtFields` 里的字段
留着（旧配置照常解析），还写着就逐条点名「已不生效——本进程按 X 跑（谁定的）」并指到向导 / `AIOS_STORE_MODE`。
`store_mode` 已恢复生效（D1）——它不再是残留键，但文件里的**退役别名**（`external` / `spawned-*` / `kv-mem` /
`embedded-mem` / `memory`）解析得出仍要点名：向导只写规范值，这种行只可能来自手改，而 `spawned-rocksdb`
的含义从「自己拉一台」变成了「只连」，不许静默改道（2026-10-15 补齐：此前退役写法只在环境变量层点名，
文件层静默生效）。`in_memory_db()` 函数删除（此前已无调用者）。

### D3　夹具与模板里不许再有这一行

护栏照旧：`no_db_option_fixture_still_carries_a_store_mode_key` 扫仓根、`db_options/`、`scripts/templates/` 下全部 `DbOption*.toml`，
任何一份出现未注释的 `store_mode =` / `in_memory_db =` 即红。2026-10 起 `store_mode` 是**生效键**——夹具里预置它
等于替每一次夹具启动静默定介质，比退役残留更不许。出厂模板同样不写：`Start-AMS.ps1` 在文件没写时显式回填
`mem`（D5），「解压即跑」不靠文件预置；每份夹具把原值记进注释。

### D4　向导第②步两档可选、落盘 `store_mode`（2026-10 修订）

`web/setup.html` 第②步恢复两个 radio（**external / memory**，external 为默认），预选文件里持久化的档
（没有就按生效档）；保存时 `SetupConfigPatch.store_mode` 校验（认不出的值拦在写盘前）后落盘成规范值
`mem` / `rocksdb`。页面同时展示后端裁决出的生效档与出处（`GET /api/v1/setup/config` 的 `effective.store_mode` /
`store_mode_source` / `store_mode_env` / `store_mode_env_value` / `config_store_mode`），环境变量 pin 了相反档时
照实说「由环境变量决定」。向导触发的重启把文件里那一档**显式**递给孩子进程的 `AIOS_STORE_MODE`
（`spawn_replacement`）——父进程环境里残留相反档也压不掉刚保存的选择；文件没写就靠环境继承，与冷启动同口径。
`v_ip` / `v_port` 与「测试连接」只在 rocksdb 档可用；第②步的闸不变（`rocksdb` 须通过测试连接）。
`ensure_completion_ready` 按「下一进程跑哪档」判：文件里写了 `store_mode` 优先，否则本进程裁决出的那一档。

### D5　发布包与部署脚本把介质递给环境变量

生成的 `Start-AMS.ps1` 新增 `-StoreMode mem|rocksdb`：给了就设 `AIOS_STORE_MODE` 再拉服务（cmd → 服务两层都继承）；
不给时**文件优先**——`DbOption.toml` 写了 `store_mode`（向导持久化的档）就按它进环境变量，文件没写才显式回填 `mem`
（服务默认档 2026-09-18 起是 rocksdb，包不靠默认兜「解压即跑」）；日志打 `STORE_MODE=…` 并标出来源。
冒烟 B 腿本来就设 `$env:AIOS_STORE_MODE`，C 腿显式 `rocksdb`；两腿都不往临时配置里写 `store_mode`；
A 腿有意不设，考默认档下的设置模式逃生门。Linux 部署（`deploy/linux/install_backend.sh`）在 `aios-database`
单元里加 `Environment=AIOS_STORE_MODE=rocksdb`——env pin 压过向导改档是运维显式接管介质，启动 notice 会并排念出两边。

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

- 生效链三层：`AIOS_STORE_MODE` > `DbOption.toml` 的 `store_mode`（向导持久化）> 默认 `rocksdb`；
  `effective_store_mode(env, file)` 两参数，`store_mode_source` 答得出「环境变量 / 配置文件 / 默认」三种出处。
- **凡是想 pin 介质的地方走环境变量**：仓内 `Start-AiosDatabaseManual.ps1` / `Run-*.ps1` /
  `Verify-RocksDb8000.ps1` / `l3_suite/fixture.rs` / `review/acceptance.ps1` 与 systemd 单元都显式带
  `AIOS_STORE_MODE`；它压过向导选的档时启动日志并排点名「两边不是同一档」。
- 向导第②步是持久化的正路：选 external / memory 写 `store_mode`，向导触发的重启把它显式递给孩子进程。
  发布包用户要持久库：向导选 external 并把 `v_ip:v_port` 指到一台在跑的 SurrealDB，或 `Start-AMS.ps1 -StoreMode rocksdb`。
- `/health` 与 `/api/v1/setup/config` 报的介质与进程实际一致；`effective.*` 五格把「生效档 / 谁定的 /
  环境变量名 / 环境变量值 / 文件里那句」分开报，老客户端不受影响。

## 实施约束

1. 裁决：`the_default_store_mode_is_rocksdb`（不设 / 空白 / 拼错都落 rocksdb；`mem` 仍可显式选）、
   `the_store_mode_layers_are_env_then_file_then_default`（env 压文件含相反档；env 缺或认不出落给文件；
   两边都认不出落默认；`ROCKSDB` / ` Mem ` / 退役别名两层都认）、
   `an_unrecognised_store_mode_is_ignored_but_never_silent`。
2. 点名：`a_retired_store_mode_still_parses_but_says_what_it_became`（env 层四种旧字面值逐条说出落到的介质与含义变化）、
   `a_retired_store_mode_in_the_config_file_is_named_too`（文件层同一纪律——向导只写规范值，退役行只可能来自手改；
   2026-10-15 补）、`the_persisted_store_mode_is_quiet_until_the_env_overrides_it`（文件是生效层：env 没说按它跑不吵；
   env 压了它并排念「两边不是同一档」）、`the_legacy_in_memory_db_switch_is_named_not_swallowed`
   （`in_memory_db` / `AIOS_IN_MEMORY_DB` 只点名）、`the_effective_store_mode_can_name_who_decided_it`（三种出处）。
3. 护栏：`no_db_option_fixture_still_carries_a_store_mode_key`。
4. 向导：`the_wizard_offers_two_store_modes_and_persists_the_choice`（页面含 radio 预选持久档 / `effective-store` /
   `store_mode: storeTier()`）、`write_config_persists_store_mode_and_accepts_a_blank_address`（只带 `store_mode`
   的补丁落盘成规范值）、`patch_validation_rejects_malformed_values`（`store_mode` 拼错拦在写盘前）、
   `the_config_endpoint_reports_effective_and_persisted_store_mode`（`effective` 五格与 `options::store_mode()`
   同一答案、`config_store_mode` 念文件里那句原文）。
5. 校审：拒绝启用的 503 正文与横幅给出向导第②步与 `AIOS_STORE_MODE=rocksdb` 两条改法
   （`the_refusal_router_answers_every_review_path_with_503_and_a_reason` /
   `the_refusal_banner_says_which_medium_which_routes_why_and_how_to_fix`）。
6. 服务端源码里不得再有 `options::in_memory_db()`；`effective_store_mode` 保持「env > file > 默认」两参数签名，
   第三层想插队得先改这个签名——那就是在改本文。
