# ADR-074：本进程从不拉起数据库——`store_mode` 只剩 `mem`（进程内）与 `rocksdb`（连配置指的那台）

状态：**已接受**（2026-09-13 用户拍板：「mem 进程内，rocksdb 连配置里的远端库，不再自己拉子进程」；
字面值用新名 `mem` / `rocksdb`、默认 `mem`、旧写法当别名认但启动时大声告警。**号到提交时复核**——ADR-060 曾撞号两次。）
**2026-09-18 修订（ADR-083）**：介质**只由环境变量 `AIOS_STORE_MODE` 决定**，`DbOption.toml` 的 `store_mode` / `in_memory_db`
退出配置文件、向导第②步只展示不选。本文 D3 **整条被取代**：「`in_memory_db` 两态映射」作废，且**默认档由 `mem` 翻成 `rocksdb`**（ADR-083 D6，同日修订——漏设的代价要落在「当场报连不上」而不是「悄悄跑在内存库上」；「解压即跑」改由 `Start-AMS.ps1` 显式回填 `mem` 保住）；D4 的别名与告警只对
**环境变量里**的退役写法生效，「向导只写新名」读作「向导一个字都不写」；实施约束 1 的护栏反转为
`no_db_option_fixture_still_carries_a_store_mode_key`，约束 3 的向导用例换成 `the_wizard_shows_the_effective_store_mode_and_never_writes_it`。
两档本身、只连不拉、durable 谓词与 `data_face` 派生（D1 / D2 / D5 / D6）不变。
**2026-10 再修订（ADR-083 二次修订）**：裁决恢复分层——`AIOS_STORE_MODE` > `DbOption.toml` 的 `store_mode` > 默认 `rocksdb`；
`store_mode` 回到配置文件（向导第②步 external / memory 落盘的选择），「向导一个字都不写」作废，`in_memory_db` 仍只点名；
「D4 的别名与告警只对环境变量生效」作废——文件层的退役写法同样点名（2026-10-15 补齐）。
日期：2026-09-13
落地：`src/options.rs`（`StoreMode { Mem, RocksDb }`、别名解析、`retired_store_mode_meaning` 告警、`has_endpoint()` 删除）、
`src/lib.rs`（`ensure_store_connected` 两分支：`connect_in_memory_store` / `connect_configured_store`；`ensure_own_store` 删除）、
**`src/store_process.rs` 整个删除**（约 1800 行：端口探针 / 署名 / 心跳 / 四格裁决 / 孤儿回收 / 收尸）、
`src/web_service/setup.rs`（判据两档、写口只认新名、`ensure_completion_ready` 复用 `options::parse_store_mode`）、
`web/setup.html`（第②步两档）、`src/fast_model/model_db_adapter.rs`（`PersistMedium::is_reachable()` 并入 `is_durable()`）、
仓内全部 `DbOption*.toml` 夹具与 `scripts/templates/DbOption.factory.toml`、`scripts/New-LocalRelease.ps1`（冒烟 B / C 腿改由脚本起库）、
`scripts/Verify-OutOfBox.ps1`（改考「一台库都不拉」）、`scripts/Run-KvMemRoomBootstrap.ps1`（退役，见后果）。
关联：
- **作废 ADR-070**（`spawned-rocksdb` 一档与「创建实例 = 一档配置」）；其 D1–D6 随本文全部失效。
- **作废** `docs/plans/2026-09-04-spawned-memory-store-out-of-box.md` 的机制部分（署名 / 四格裁决 / 收尸 / 版本闸在启动侧）；
  「开箱即用」这个目标本身由 `mem` 接手——它比 `spawned-mem` 更开箱：一个进程、零端口。
- **修订 ADR-068** D6 / **ADR-069** D3、D5、D8：凡提到 `embedded-mem` / `spawned-mem` / `spawned-rocksdb` / `external` 的判据与
  冒烟腿，按本文 D1–D5 读。两份 ADR 的正文不逐句改，各自状态行加一句指向这里。
- CONTEXT.md「本地发布包」「冒烟门」「设置模式」词条随本文改写；「自带持久库」词条作废。
- 共识 **d-338**（写入标记不加 phase）不受影响：本文只改介质从哪里来，不改写入标记与恢复策略。

## 背景

`store_mode` 到 2026-09-12 为止有四档：`embedded-mem` / `spawned-mem` / `spawned-rocksdb` / `external`。后三档的连接语句
一个字不差——都是 `aios_core::init_surreal()` 走 ws 连 `v_ip:v_port`；它们的区别只在「那台 surreal 是谁拉起的」以及后端是
memory 还是 rocksdb。为了「本进程负责把那台库拉起来并收尸」，`store_process.rs` 背着约 1800 行：端口探针、库内署名、心跳、
四格裁决（空闲拉起 / 自家附着 / 孤儿收尸 / 别人的库拒启）、版本闸、退出收尸——**全在管一个子进程的命，与「数据放哪儿」
一个字的关系都没有**。默认值在两周里翻了三次（`external` → `spawned-mem` → `external` → `spawned-mem`），每翻一次都让没写这一项
的现场配置静默换介质、连带换掉整个数据面。

用户 2026-09-13 问「两个数据库现在是如何访问的」，听完四档之后定下：**内存就在进程里，落盘就连配置指的那台服务器；本进程不再
拉起任何数据库。**

## 决定

### D1　只有两档，命名就叫介质

`store_mode = "mem"`：`SUL_DB.connect("mem://")`，库在进程里，没有端口、活不过重启，只有本进程读得到。
`store_mode = "rocksdb"`：`aios_core::init_surreal()` 连配置里 `v_ip:v_port` 上那台 SurrealDB（rocksdb 后端）。

字面值就是介质名。不再有「谁拉起的」这一维——因为答案永远是「不是本进程」。

### D2　本进程只连，不拉起

`rocksdb` 档端口上没东西就是连不上，**报错退出**。不自己起一台空库然后在上面作业：那种失败一声不响，现象是「查了半天什么都
没有」，最难查的那一类。`store_process.rs` 整个删除；`AIOS_SURREAL_EXE` 环境变量服务端不再读（只有 `Start-Surreal8009.ps1` 与
兼容性测试还认它）。

### D3　默认 `mem`；旧开关 `in_memory_db` 的两态原样映射（**整条已被 ADR-083 取代，2026-09-18**：默认翻成 `rocksdb`，旧开关退役）

两个键都不写 → `mem`（开箱即用：不配任何东西就能起来）。没写 `store_mode` 但写了 `in_memory_db = true` → `mem`；写了
`in_memory_db = false` → `rocksdb`——那句显式的 `false` 是人的意图，不能被默认覆盖。两个键都写且矛盾时以 `store_mode` 为准并由
`store_mode_notice` 出声。仓内夹具一律显式写 `store_mode`，由 `every_db_option_fixture_declares_its_store_mode` 钉住，不押在默认上。

### D4　旧写法认、但当场说清换成了什么

`embedded-mem` / `kv-mem` / `spawned-mem` / `memory` → `mem`；`external` / `spawned-rocksdb` → `rocksdb`。**不许静默改道**：
现场配置照抄了好几年，`retired_store_mode_meaning` 在启动时把「你写的是退役写法，已按 X 生效，含义变成了什么」一句打出来
——最危险的是 `spawned-rocksdb`：它以前会自己把库拉起来，现在只连，端口上没库就起不来。**向导只写新名**：
`validate_patch` 只认 `mem` / `rocksdb`，退役值认得出不等于该继续产出。

### D5　「持久」与「别的进程连得上」合并成同一个问题

四档时 `is_durable()`（活得过重启）与 `has_endpoint()` / `is_reachable()`（别的进程连不连得上）是两个问题——`spawned-mem` 不
durable 却有真端口。两档之后进程内嵌那档既不持久也没有端口，配置指的那台既持久又连得上：两个谓词恒等，删掉一个
（`StoreMode::has_endpoint()`、`PersistMedium::is_reachable()`），留着只会让下一个人以为它们还能分开。`/health.sul_db`：
`medium` 报 `mem` / `rocksdb`，`endpoint` 在 `mem` 下报 `embedded:mem`（不是配置里那对没被连过的地址），`durable` 只对
`rocksdb` 为真；`in_memory_db` 这一栏为老客户端保留。

### D6　`data_face` 仍由介质派生

`rocksdb` → `ingest`；`mem` → `read-through`。与 2026-09-05 的口径一致，不因两档而变。

## 后果

- **mem 档没有端口。** `/sql` 探针、`rvm_verify`、`Capture-*Evidence.ps1` 与 plant-ui 的「库供数」读面在这一档全都够不着。
  plant-ui 的主启动序列不受影响——它早就不直连库（`plant-ui-app/src/data.rs`），能力缺失那一路按 `mirror.endpoint = "embedded:mem"`
  自己降级。**要外部诊断就用 rocksdb 档**，没有第三条路。
- `Run-KvMemRoomBootstrap.ps1` **退役**：它对着 `spawned-mem` 的端口发 SQL 挑正例 / 反例，读透形态只剩没端口的 `mem`，
  那两条 SQL 无处可发。脚本保留、开头直接报错说明原因；要救回来须把挑选改走服务 HTTP 面。
- 发布包：**两种包出厂都是 `mem`**（用户 2026-09-13 追加拍板：「两档都真正解压即跑」），出厂模板原样进包、组包脚本不再按档位
  外科改写 `store_mode`。要数据活得过重启的现场：先跑包内 `Start-Surreal.ps1` / `bin\surreal.exe` 起一台 rocksdb 库，再在向导
  第②步改选 `rocksdb`。冒烟 B 腿默认 `mem`；C 腿（带 workspace）改为脚本用包内 `surreal.exe` 起库、服务连它，验的是
  「包内二进制起得来、服务连得上、服务自己没拉子进程」。
- 单实例锁：`mem` 不拿（两个实例各连各的库，锁没有保护对象；`meshes_path` 仍是共享的，并排跑前先分家）；`rocksdb` 照拿
  ——那台库别人也连得上。机器级写者锁同理。
- 少了约 1800 行与「谁拉起的」有关的代码；`options.rs` 的档位真值表从 4×N 缩成 2×N。

## 实施约束

1. 两档真值表：`only_the_disk_backed_store_is_durable`、`the_default_store_mode_is_mem`、
   `data_face_follows_storage_backup_scope`（rocksdb → ingest，mem → read-through）；仓内夹具由
   `every_db_option_fixture_declares_its_store_mode` 钉住显式写 `store_mode`。
2. 别名与告警：`a_retired_store_mode_still_parses_but_says_what_it_became`——`parse_store_mode` 认全部退役写法；
   `store_mode_notice_for` 对退役写法必出声、对 `spawned-rocksdb` 必点名「不再拉起」、对 `spawned-mem` 必点名「没有端口」；
   `a_contradicting_legacy_flag_is_named_not_swallowed` 管 `in_memory_db` 打架。
3. 向导：`the_wizard_offers_exactly_the_two_store_modes_and_writes_only_new_names` 钉住 `web/setup.html` 只有 `mem` / `rocksdb`
   两个 radio、退役 value / return / 比较一处不留；`validate_patch` 拒收退役值（`write_config_accepts_a_blank_address_in_either_mode`
   的尾段）。
4. 设置模式判据：`setup_mode_reason_for(store, project_path, v_ip, v_port)` 两档真值表
   （`setup_mode_triggers_on_missing_project_path_or_blank_endpoint_address`）——`rocksdb` × 空地址 = 设置模式，`mem` 不看地址。
5. 服务端源码里不得再出现 `store_process` 模块名与 `SpawnDatastore`；`AIOS_SURREAL_EXE` 只允许出现在起库脚本与
   `src/test/fork_surreal_compat.rs`。
