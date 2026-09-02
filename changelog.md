# 变更记录

## 2026-09-02

### 新增

- **ADR-057（提议）：e3d-model 是模型面的纯函数层，模型面的状态与副作用归 gen-model。**
  起因是用户问「模型生成和模型增量不是都应该发生在 e3d-model 吗」——算法层面两件事确已在
  e3d-model（`pipeline` 全量生成 + `increment` L0–L4），漂的是三处：生产选根还没接 e3d-model
  （legacy 走 `model_impact`，direct 无窗口差分即审核 S8）、e3d-model 的 `execute_plan` 在 spec 035 P1
  删 `apply_window` 臂后将没有生产消费者、纯文件函数 `enumerate_generation_roots` 却在 gen-model。
  ADR 把边界写成一句口径「对 `(库文件, 会话)` 是纯函数的进 e3d-model；碰 SurrealDB / 凭证 / 队列 /
  锁 / 房间 / 空间树 / MQTT 的留 gen-model」，D3 定 `execute_plan` 为参考实现 + 离线工具而非生产
  执行器（不删、不接线），D4 / D5 把根枚举与 `touches_roots` 划归 e3d-model（与 spec 035 P2-1 同批），
  D6 钉依赖方向单向、e3d-model 不得依赖 `surrealdb` / `aios_core` / `pdms_io` / `tokio`。
  同批落地：`vendor/e3d-model/src/increment.rs` 模块文档新增「生产定位」一节（只改 `//!` 注释）；
  `CONTEXT.md` 新增「模型面 / 数据面 / 模型面状态」词条。状态**提议**，待拍板后 `record_decision`。

### 变更

- **未指定时点的模型生成一律取文件最新会话（ADR-054，取代 ADR-053 Q3）。**
  用户拍板原话「如果没有指定时间，默认就是要使用最新的数据去生成模型」。当前投影的时点
  与库文件权威换源：`E3dModelService::from_current` 的 pin 来自 MDB 成员、时点留空，生成时
  按目标库解一次文件最新会话（新模块 `data_interface/model_source.rs`，复用历史投影
  `historical_model::resolve_session(Latest)` 这一把尺子，按文件长度 + 修改时刻缓存）；
  生成根所在库不再 `SELECT dbnum FROM pe`，改在 MDB 的 DESI 文件里按会话索引点查
  （零解析项目从此能点看即生成；命中零个或多个都报错，不猜）。完成凭证判据由等值改
  单调（`gen_root.source_end_sesno >= 要求时点`，`0` 永不算覆盖，时刻列只记不比），
  `generation_root_cache_current` / `gen_root_credential_is_current` / ADR-025 模型门
  `model_coverage_current` 同批改；`apply_window` 不再断言 `pin.sesno == target`，窗口右端
  按显式时点开库，已发布的更新版本覆盖旧窗口时收口为成功而不是失败。`DirectStore` 的
  「没钉 pin 即报错」翻成「定位器认识的库开库解最新、不认识的才报 `NoFileForDbnum`」，
  `NotPinned` 变体随之退役；`pins_from_watermark` 保留为对拍探针显式钉水位的工具。
  已知代价：`ModelTarget` 带 `(file, session)` 摘要，切换后存量凭证全部失效一次，首显整片
  重生成。验证：`cargo check --lib --bins` 绿；lib 单测 1297 绿，8 红均为本变更之外的在飞
  工作（`aabb_tree` 白名单、`pdms_inst` t041 段数分行），与本条无关。

## 2026-08-30

### 新增

- **legacy↔v2 读取对拍探针（P1 尺子）落地并批跑全绿**
  （`src/bin/legacy_v2_read_parity.rs`；判读 `docs/evidence/2026-08-30-legacy-v2-read-parity.md`，
  逐文件原始数据与自动汇总同名 `-raw.jsonl` / `-raw-summary.md`）。431 个 ams000 真库、
  文件级错误 0、探针自检 0：活叶页尾部多读 **779,558** 条（168 库，D3-1 陈旧槽位坐实，
  **P3 门实测开**）、纯幽灵键 8,807 个、抽样幽灵 35.6% 可被旧栈**点查**够到
  （`ams8000_0001` 上 64 个——幽灵不止污染穷举，点查也漏）；两栈同中的点查位置不一致
  **0**（D3-4 文件态位拆一致再证）；v2 活键数与 e3d-io 429 库硬门 789,831 **逐键吻合**，
  两条独立 core.dll 对齐实现互证通过；活条目 flag 全 1、陈旧槽位 98.9% flag=0，
  「认领扫描 flag==1 vs 点查无视 flag」口径分裂拿到第一份定量证据（D3-5/V2）。
  会话链侧顺带钉住 D2-2 的具体形态：旧栈 `cur_ses_pgno > 4` 系统性看不见落在第 2–4 页
  的创世会话（429/431 库）。顺带：`old-parse-pdms-db` 整体再导出 `pdmsdb_engine_v2`，
  下游做对拍不必二次钉 rev。
- **dabacon 索引页布局裁定：AoS + `free_dwords` + 子树最小键**
  （`docs/evidence/2026-08-30-dabacon-index-page-layout-adjudication.md`，原始输出
  `docs/evidence/2026-08-30-dabacon-index-layout-probe-raw.txt`）。仓里先后四份索引解析
  对同一批字节有三处互不相容的读法，且没有任何现存测试能区分它们——`pdmsdb_engine_v2`
  那条真库 round-trip 是「同一套解码 + 同一套编码」，错的读法照样逐字节相等。用纯文件
  探针（`old/vendor/e3d-io/examples/index_layout_probe.rs`，不依赖 IDA、不调用任何现存
  索引代码）把 2×2×2 八种组合在 429 个真库文件上按七个结构不变量打分，唯一解是
  **AoS 条目 + 页头 `0x18` 反推条目数 + 非叶键为子树最小键**（429/429 全零；SoA 三种
  组合 0/429）。三条结论：① **推翻** `2026-08-13_reverse-core-dll-index-leaf-report.md`
  标「事实」级的 C2 条（结点不是 SoA）；② `0x18` 是 `free_dwords` 不是 `pfno`，「扫到
  全零槽为止」在 ams8000 上凭空多出 7025 条叶条目、6790 个重复 refno、52 个重复子指针、
  7 处层级异常、53 个读不动的子页——`session_index_diff.rs` 那一整套异常计数**是计数
  bug 的产物，不是文件格式的性质**；③ 非叶键是子树最小键，仓内所有注释都写反了，但路由
  代码碰巧实现的是最小键语义（所以点查一直对，写下来的模型一直错）。
- **e3d-io L0/L1 重写：页 I/O 与会话链按 core.dll 的真实寻址落地**
  （`old/vendor/e3d-io`，`aaf14a5`）。`PageSize` 改 newtype，`words()`/`bytes()` 分开——
  文件头 `0x34` 按 4 字节字计、被当字节读，490 个真库里 17 个中招（`ams7329_0001` 读出
  `sesno=0`，权威值 221），这类混淆现在编译期就不成立；页大小不合法即 `Err`，删掉
  `else { DEFAULT_PAGE_SIZE }`（那个默认值是 4096，真实页大小 2048，正好差一倍）；
  `PageId{ext,page}` 的 `ext` 真的用来选文件句柄，取不到 extent 报 `MissingExtent`，
  不再把 extent 2 的第 N 页安静地读成 extent 1 的第 N 页；`read_into(&mut [u8])` 是唯一
  原语（旧实现每读一页分配一次，一次树下降就是树高次堆分配）；positioned read；预读页
  与按需读分开计数，否则「点查读了几页」量不准。L1 侧 `open()` 只是 `open_at(Latest)`，
  没有第二套 latest 逻辑；会话不存在是报错不是 `None`；链走有环保护与页数上限。顺带删掉
  `engine::sessions()` 里那套**错的**第二份会话解析——它从 `0x30`（最旧会话）起步、只读
  一页、不跟 `last_ses_pgno`，在 264 个会话的库上返回单元素 vec 且报成功。
- **e3d-io L2 索引页解码器 + B+ 树游标**（`6be8847`、`a9ade97`）。解码按上条裁定实现且
  不提供任何回退读法；畸形页一律报错命名失败字段，绝不降级为空条目列表。游标从**会话页
  携带的索引根**下降，点查与顺序枚举共用同一个结点读取与同一条路由规则——因此二者对
  「什么算存在」给出同一个集合，这条已设为**硬门**：429 个真库、枚举出的 **789831** 个键
  逐个点查可达且落到同一条记录，枚举没给出的邻近键点查一律 `None`（旧的「扫到零为止」
  计数下这道门根本不可能通过——它凭空造出的条目没有任何下降路径能路由到）。冷缓存点查
  代价用**单页缓存**度量（读两次就计两次），实测恰好等于树高，语料里最深的树是 3。
  `ReadOnlyEngine::search_index` 那份自带的下降已删除、改为委派——它既不校验子层级递减
  也无防环，指回自己的树要靠深度计数器兜底。新增源码顺序断言：索引模块之外再出现第二条
  路由规则就红（这个栈继承过来的是四份）。
- **e3d-io L3 记录层重写：按记录自己写下的地址取，不再靠找**（`522a252`）。记录头第 6/7
  字是**显式属性流地址**、第 8/9 字是**成员表地址**，都用索引叶值那套 `(page_no, packed)`
  编码；每个块尾同样写着自己的续接地址。438 个真库、2 923 428 条记录、这些地址够到的
  **3 195 015** 个块里，指向空处的 **0** 个、类型对不上槽位的 **0** 个。旧读法三处全错：
  ① 把 `0x00000007` 当结束标记往后找，可它只出现在 **29 027 / 789 831（3.7%）** 条记录的
  真实末尾，其余 96% 多吃了下一条记录的一截，另有 1 033 条一路吃到 64 KiB 上限、把完好的
  记录报成截断；② 块之间靠「跳过填充继续找」搭桥，这一步在 **36 106** 条记录上触发且**无一
  例外**越过了本页边界——它把同一元素的**后来副本**接到了正在读的记录尾巴上；③ 相邻性根本
  够不到 **46 566** 个块（分布在 40 263 条记录上）：`ams5100_0001` 的 ROOM_NO 定义
  13292/122 离页尾只剩 24 字节，塞不下一个块，整条属性流在下一页，按相邻读它就是个没名字、
  没属性、没有「适用于哪些 noun」列表的空壳，**BRAN 因此丢了 Dictionary 给它定义的 22 个
  UDA 里的 2 个**。块头那两个「保留字」是续接地址（BRAN 24383/85432 的成员表里那个
  18010/8193 就是它被当成员读出来的），所以块 payload 恒从 +20 起，显式解码器里「0 和 8
  两个起点都试一遍」的兜底也一并删掉。硬门：429 个真库 **789 831 / 789 831** 全部读出且
  自报 RefNo 一致（旧读法直接失败 1033 条）；交出去的字节数与解析器消费的字节数逐字相等。
- **e3d-io：查元素只从会话根下降一次**（`357512d`）。`find_element` 原先叠了三层兜底——
  搜遍文件里所有树、取最靠后的副本、都不认就把整个文件倒着扫一遍找 RefNo 字节模式——
  三层都在给「根找错了」擦屁股。根来自「读每一页、留下没被别的索引页指过的索引页」，索引
  是 copy-on-write，于是历次会话遗留的树全被当成根。现在是一次下降加一次读记录；记录自报
  别的名字是**报错**而不是没找到（429 个库 789 831 个键里 0 例）。`ams1112_0001` 的
  17496/950 那「十一个叶子、十个指向别人」是十棵死树加一棵活树，而那棵活树属于会话 12——
  它自那以后就没再被任何会话索引过。`scan_all_indexed_refnos` 换成 `indexed_refnos`：
  旧的取全文件所有索引页的叶条目共 940 694 条，活树只有 789 831 条，**16% 是够不到的树**。
  连带发现：本仓 TTY 采样清单 `sample.tsv` 正是从旧清单里挑的，**211 行里有 77 行是此后
  再没被任何会话索引过的元素**（光 `ams8000_0001` 就贡献 26 个止于会话 8 / 共 264 会话的
  图元），另有 4 行记的是被取代副本的隐式字数。
- ~~pdms-io 直读底座新增 Core3D session-pinned `open_at`、严格 page type 5 解码、
  `IndexCursor` 实例查询和 raw index slot 类型；多 extent 不再隐式退回 legacy 扫描。~~
  **（此条描述的是 `pdms-io-fork-engine-v2` 工作副本里的改动，该目录已从磁盘消失、
  代码不复存在；留着划掉以免日后查史被它带偏。上面 e3d-io 几条是重新实现的等价能力。）**

### 修复

- `Cargo.toml` 里指向 `../../pdms-io-fork-engine-v2/crates/pdmsdb_engine_v2` 的手动
  `[patch]` 改回注释态：该工作副本已从磁盘上消失，带着悬空 patch 连 manifest 都解析不了。
  `cargo metadata --no-deps --offline` 恢复正常。
- Python direct 解析新增 `parse.attmap` 与 `parse.subtree`，通过 `pdms-io` 直读指定 refno 及全部后代；补充 AMS 8000 ZONE 离线验收与 IDA DbElement 交叉证据。

## 2026-08-29

### 新增

- **ADR-055：新版 pdms-io（db1~db5）的元素语义以 Core3D.dll 为准**（grill Q1–Q8 全采
  推荐项）。分层定权威——db1–db3（页/会话/B 树）继续以 core.dll 为准，db4–db5 及以上
  （noun 位分类、三模成员遍历、significant 攀爬、库类型门、CE 导航）以 Core3D 为准；
  noun 位表走 `trait NounBitSource` 双实现（生产读 `core_sha256` 钉版的快照、校验不过
  报错不回落，对拍走 core.dll FFI 现取）；证伪靠三层 oracle（legacy + core_dll + 新增
  `core3d_oracle`，以可执行参考模型为期望值、C 编号用例数据驱动）；pdms-io 只做读语义，
  队列/去重/三遍消费留在 gen-model；本轮写侧冻结。配套 `specs/034-core3d-semantics/`
  三件套（spec / plan[Constitution Check] / tasks，P0 尺子 → P1 db4 语义层 → P2 CE 栈 →
  P3 库类型+extent → P4 页大小/时点收尾 → P5 gen-model 升 rev 联动）与
  `docs/plans/pdms-io-v2-core3d-alignment.md`（拷问全文，已镜像到 pdms-io 两个工作副本）。
  实施基线核查发现 P4 两条根因（文件头 `0x34` 按 4 字节字解释、页大小探测假匹配拒绝）
  已在 engine-v2 分支以 `348d187`/`cb7dd95` 落地，P4 缩为真库回归 + `open_at(sesno)`。

## 2026-08-28

### 新增

- **core 的 noun 粒度位表导出入库：`significant` 127 / `primitive` 374**
  （模型增量更新向 Core3D 对齐计划的 P0）。`PartialUpdateDesiMgr` 决定「改了之后重画
  什么」用的不是类型名单，而是 noun 描述符上的两个 bool 位；本轮把它们从 live E3D 3.1
  取下来落进 `tests/fixtures/core-noun-granularity-e3d31.json`（schema 2，三张表
  unknown 与 not-found 全为 0）。采集脚本同批泛化成多字段（`--field granularity`），
  单字段仍写 schema 1 且序列化逐字节不变（1931 行回放验过）；跑之前先问 core 自己的
  `DB_Noun::fieldType` 决定用 bool 还是 int 重载而不是猜——**用错重载是静默的**，
  实测四个 bool 位返回 0、`negative` 返回 3，独立复核了逆向取证的类型划分。
  数据本身回答了计划里的分叉点：core 的 significant 是 127 个而不是原先估的「数百」，
  对我们 4 个 MDU 类型仍是 32 倍差距；`primitive_b` **不是** `primitive_a` 的子集
  （或式差 27 个，合并成一个位会丢 noun）；1514 个 noun 两位都不带——core 对它们的
  变化什么都不做，而我们走 `Unknown → Regen`，这是差距最大的一格。第一处对账也出来了：
  `DEFAULT_DELIVERY_UNIT_TYPES` 四个里 core 认 BRAN / EQUI / HANG，**不认 `SUPPO`**。
  三条守卫测试落在 `generation_root.rs`（快照自洽、两个 primitive 位不冗余、SUPPO 是
  唯一分歧），快照重导后结论若变会直接变红。生产判据本轮不动，那是 P1/P2 的事。

- **失败记录带走落记录那一刻的暂存窗口残留（ISSUE-025 §四 4a 的记录一半）**。
  `BatchFailure` 新增 `staging` 一格，取 `staging::lifecycle::resource_snapshots_for(dbnum)`
  的原样输出写进 `logs/batch-failures-*.jsonl`，与 `/health` 的 `staging_windows` 共用
  同一个成形函数——面板卡与失败记录说的必须是同一句话，不许出现第二种形状。语义三分：
  `[]` = 记录时该库名下已无窗口（回滚干净，或这一批走直写没建窗口）；非空 = 残留，
  重跑多半撞同一堵墙；字段缺席 = 老版本记录，答不了这一问。面板失败卡照此渲染（残留
  红字带写回原话与档位，空一句带说明，老记录不画）。这一格的动机就是 8191 现场：
  「暂存窗口」卡活在进程内、重启即清空，而人来问「`staging_8191_1` 回滚没有」的时候
  往往已经重启过了——落盘那一份才是异机与事后能看到的。测试
  `a_record_carries_the_staging_leftovers_at_write_time` 钉住空数组不许藏格。

### 修复

- **`primaryList` 快照里那 52 个「core 读不出来」是读取通道的假象，现已补全并按 core 改判**
  （ADR-009 修订）。`scripts/e3d/dump_core_primary_list.py` 一直走
  `db_get_element_info`，而那个导出**根本不是通用字段读取器**：它转发到
  `sub_5B05280`，后者把 field id 写死成一个五路 switch（`642215` / `297853135` /
  `243803617` / `11037101` / `282170750`），落在外面一律置错误码 542 返回 0；
  `primaryList` 能读通只是因为它恰好是这五个之一。更要命的是外壳在**内部 noun 查找
  失败**时同样报错返回，2026-08-18 记的那 52 个 unknown 正是查找失败的那批，被当成
  「core 没答」保守取真。改走 core 自己导出的 `DB_Noun::findNoun` +
  `getField(int|bool)` 之后一次读通：1931 个 noun **全部解析**，与旧快照重叠的 1879 个
  **逐值相同、零处不一致**，那 52 个**全部为 `false`**（其中 8 个还带着真实粒度位，
  说明记录是实的不是零值）。按 ADR-002 的 core 权威口径改判，`false_count` 737 → 789，
  这 52 个类型不再多做成员差分；保守兜底保留，但只覆盖「`noun_flags.json` 之外的
  noun」，快照测试把 `unknown` 为空钉死——它若回来就说明导出器退回了旧通道。顺带答了
  08-18 记的那条待决项：`core_sha256` 对不上是因为本机 core.dll 在 08-23 被改写过
  （大小不变），而值逐个相同，**是产物层面的溯源漂移不是数据漂移**。

- **错误日志按登录项目筛：`?dbnum=8191` 不再把上一份配置留下的同号记录一并端出来**
  （ISSUE-025 §五 5a/5b 收口）。`logs/` 落在进程工作目录下、跨配置改动活着，同一个
  目录先后跑过两个 `project_name` 时，8191 这类号在每个项目的 sys 库里都存在。
  `/api/v1/error-log` 与 `/api/v1/batch-failures` 现在认 `?project=`：缺省只回本服务
  登录的项目（服务自己知道它登录的是哪个项目，这件事不该问人），`all` 不筛，其余值
  指定项目。**不带 `project` 字段的行不筛**——`queue_stall` 与老版本记录本来就没有
  这一格，按项目筛把它们静默滤没，「这台机器没停滞过」就成了筛出来的假话。回执带
  `project_filter`（`null` = 没筛）：「这个库没失败过」与「被筛掉了」必须分得开。
  面板同批收口：错误日志卡加「全部项目」开关（服务端重取，口径以**已取回那份**回执
  为准、不抢答），空状态分「本项目名下没有」与「真的没有」两句话；`byDb` 同号显式
  择优（取归属 `h.project` 的那一行，一行都对不上退回头一行，此前是后来者静默覆盖），
  连败卡的撞号候选表只在当前项目一个都对不上时才摆出来。测试
  `the_project_filter_keeps_own_and_unattributed_rows` 钉住三件事：本项目 + 无归属
  留下、别的项目滤掉、回执报筛子。

## 2026-08-27

### 新增

- **内存模式：持久层可以整个换成进程内嵌的 kv-mem，rocksdb 不再出现在链路里**。
  `DbOption.toml` 新增 `in_memory_db`（默认 `false`，环境变量 `AIOS_IN_MEMORY_DB` 压过它）。
  开着时 `run_app` 不再连 `v_ip:v_port` 那台 SurrealDB，而是把进程全局 `SUL_DB`
  接到 `mem://` 上：初始化解析、增量窗口写回、模型与房间派生数据全部走原来那条路，
  只是落点从 rocksdb 变成内存。没有第二条代码路径——`SUL_DB` 是 `Surreal<Any>`，
  引擎选择关在句柄里头，上层每一处查询与写入一个字都不用改。嵌入式引擎没有认证面，
  因此不 `signin`；ns/db 仍取配置里那一对，同一份配置在两种介质下指向同一个逻辑库。
  与 ADR-017 的暂存窗口不是一回事，也不冲突：窗口本来就各自占一个独立 `mem://` 实例，
  这里连的是另一个实例，journal 分块重放与水位发布的语义原样保留。
  **这是非正常运行状态，因此三处都要出声**：连接时打两行取舍、启动横幅点名、
  `/health` 加 `in_memory_db` 栏。代价写在配置注释里，两条都不可逆——进程一退整库就没了
  （崩溃现场只剩日志），库进了进程内也就没有端口可连（`rvm_verify`、`/sql` 探针、
  `Capture-*Evidence.ps1` 全部够不着）。要留证据或要让别的进程读库，用部署脚本的
  `-InMemory`：那是让**外部** surreal 进程换 memory 后端，端口还在。管不到的东西也说清楚：
  `assets/meshes` 与 `accel_tree` 快照仍然落盘，它们是 aios-database 自己写的派生数据；
  空间树启动判据照旧按库侧指纹裁决，空库走重建这条不变。

- **运维面板内嵌进二进制，部署只剩一个 exe**。`ops.html` 由 `include_str!` 烘进
  `aios-database`，`GET /ops.html` 走显式路由、压过底下那个 `ServeDir` 兜底，所以
  现场不再需要跟着同步一个 `PLANT_UI_WEB_ROOT` 目录，也不会出现「后端升了、面板还是
  上一版」这种两份东西各走各的。**磁盘副本仍然优先**：`ui_root/ops.html` 存在就用它，
  且是每次请求现读——「改一句判决措辞、落个文件刷新就行」这条不能被内嵌收走，重编一次
  是分钟级还得停服务换 exe；靠目录部署的机器因此行为一字不变。响应带
  `x-ops-panel-source: embedded|disk`，两份内容不一致时这是唯一说得清的地方；另加
  `Cache-Control: no-cache`——页面此后跟着 exe 版本走，而它恰恰是用来回答「现在是什么
  状态」的，自己被缓存住最要命。启动时也把用的是哪一份打在控制台上。
  测试 `the_embedded_panel_is_real_and_a_disk_copy_still_wins` 同时钉住两头：内嵌的
  确实是那一页（`include_str!` 指错文件或 `web/ops.html` 被清空，编译照过，直到有人
  打开一个白页才发现，而那多半发生在出事的时候），以及磁盘副本压过内嵌。
  同批把 `http_api` 并进了 `default` feature：内嵌面板的意义就是「一个 exe 拷过去就能用」，
  而 08-27 当天真踩过一次反例——默认 feature 编出的 exe 里根本没有 Web 端口，拿去部署，
  面板和 REST 一起消失，且没有任何报错。现在裸 `cargo build --release` 与 CI 的显式清单
  （`ws,gen_model,manifold,project_hd,http_api`）等价，`--no-default-features` 的场合才需要
  自己点名；编没编进去（能力）与要不要监听（`DbOption.toml` 的 `http_api_addr`，注释掉即
  不开端口）仍是两回事。web_service 的单测也从此跟着默认口径一起跑，不再要求单独点 feature。
- **`/assets/ops.html` 加实时事件流，并把失败原因、当前阶段、撞号候选摊到页面上**。
  轮询回答「现在是什么状态」，新的「实时事件」面板经 `WS /api/v1/ws → tasks` 回答
  「刚才发生了什么」：批次起止与模型单元逐个完成按发生顺序各出现一次，失败带原话。
  它与轮询各活各的——WS 连不上（老后端、代理不转 upgrade）只让这一块变灰，页面其余
  部分照常；`seq` 只用来认掉帧（服务端跳帧时出提示条）而不当行 key，因为它由每条连接
  自己编号、重连从头开始。按 `ws.rs` 的协议每 30 秒 ping 一次：服务端 90 秒收不到
  **客户端**消息就断，广播出去的事件不算数，不心跳的连接每 91 秒必断一次，而空闲恰恰
  是最该盯着这一块的时候，断口里的事件没有重放、补不回来；`pong` 也占一个 `seq` 号，
  先记再退出，否则自己的心跳会被一条条数成服务端跳帧。同批三处是「数据一直都在、只是
  没画出来」：失败任务的 `result.batch.message` 整段摊开（此前查一次失败要么开
  devtools、要么另开窗口打 `/tasks/{id}`）；running 行接上 `current_stage` 与交付单元
  计数，并单报**本阶段多久没有新进展**（「慢」和「卡住」在耗时那一列上长得一模一样）；
  连败卡片改为并列展示「批次回执 —— 权威」与「`batch_failures.reason` —— 取的是
  `warnings.last()`」，后者在净窗口硬失败时只剩「收集口径：净窗口 …」那条标注，照它
  写卡片就成了一句正常统计。另外 `dbnum` 不是主键——SYS 库在每个项目里都是 8191，撞号
  是常态，而水位、重试记录与连败账本三个键又都只认号，于是撞号时把候选按 `file_path`
  全摆出来让人自己认。默认主题改为亮色：值班屏幕反光下深色那组红/琥珀拿到白底只有
  2~3:1，三色重挑到 ≥5:1；深色加 `?theme=dark`，刻意不跟随 `prefers-color-scheme`
  ——跟着操作系统走等于两台机器看到两个样子。

- **面板「最近任务」的每一行都能点开，摊出这条 `TaskEntry` 的全部字段**。
  上一批把 `result.batch.message` 摊了出来，但那只是整行里的一句话：任务下面还压着
  入队／开跑／结束三个时刻、会话区间与它对应的保存窗口、`current_stage` 与
  `stall_deadline`、依赖 refno 的总数／已解析／缺失、以及 `result` 与 `detail` 两段
  完整 JSON——**这些数据一直都在轮询回来的 `/tasks` 里**（`TaskRegistry::list()` 返回的
  就是完整 `TaskEntry`），只是没画出来，于是想看一眼仍旧只能开 devtools 或者另开一个
  窗口打 `/tasks/{id}`。现在点行即展开，**不发任何请求**，同时只开一行（五行全摊开这张卡
  就成了滚动条）。缺席的字段画「—」而不是把那一格藏起来：「这一批没有依赖阶段」和
  「这版后端还不写这个字段」处置完全不同，藏起来两者就长得一模一样；只有整组都不适用
  （依赖计数）时才整组不画，那时一排「—」只是噪音。三处按状态分叉，因为同一个字段在
  running 与终态上问的不是同一件事：跑着的行报「本阶段静默多久」（超 60 秒转琥珀）、
  终态行改报「末次进展」的时刻——一条三小时前失败的任务说它「静默三小时」是把死亡读成了
  卡顿；`stall_deadline` 只在 running 且已过期时标红；`current_stage` 在终态行改叫
  「停在哪一步」（`finish` 不清这个字段，所以它就是任务停下的地方）。排队多久与跑了多久
  分两格报，`created_at` 是入队、`started_at` 才是开跑，只报一个「耗时」会把等待算进执行里。
  展开块底部两个出口：`→ {dbnum} 的落盘错误记录` 复用队列卡那颗跳转按钮的逻辑（此前它
  只长在被 phase 挡住的队列行上，从任务卡看到 dbnum 还得自己手打进筛选框），以及直通
  `GET /tasks/{id}` 原文的链接。
  同批把这张卡「只画最近 5 条」的硬上限做成开关：勾上「列出全部」列出这一轮取回来的
  全部终态任务，**并把取数上限从 12 提到 60**——只放开渲染不放开取数，页面会一边说
  「全部」一边只有 12 条，是自己骗自己。收起时**如实报出还有多少条没画**（「另有 N 条
  跑过的任务 —— 勾上「列出全部」」）：截断了却不说，等于让人以为跑过的就这几条，而这张
  卡恰恰是用来回答「刚才都跑了什么」的。展开后若取回来的条数顶到这一轮要的上限，再说
  一句这不是全部并给出 `?limit=160`（服务端硬顶）——注册表留到 1000 条，一次全取会撑破
  单响应体积契约，而这一份是每 2 秒都要拉的。排队中的行任何时候都不混进来，它们归队列卡。

- **面板画出「暂存窗口」卡：失败之后那个窗口到底回滚了还是残留着**（ISSUE-025 §四 4a
  的面板一半）。数据一直都在 `/health` 里（`staging::lifecycle::resource_snapshots()`），
  此前一个字都没画——于是 8191 现场那屏「使用 kv-mem 暂存窗口 `staging_8191_1`
  （sesno 36..=37）」之后窗口的下落，只能去翻控制台。它直接决定重跑会不会撞同一堵墙，
  也是资源泄漏的第一现场。新卡摆在熔断卡正下方，一行一个窗口：dbnum、窗口名、会话区间、
  档位、体量。体量报的是**合计**（暂存 SQL + journal），因为配额判的就是合计，两个分量
  塞进 title——分开报会让人拿单个分量去对阈值。字节与行数摞成两行而不是挤在一行：
  `ResourceThresholds` 里 `*_bytes` 与 `*_rows` 是两组各自判档、取高者的阈值，两个数
  谁也不能省，而一行在这一列上会把 `1.19 GB · 812,344 行` 截成 `812,34…`——截掉的
  恰好是数量级那一头。列宽按 480px 排：`.page` 封顶 1440、`.col-r` basis 就是 480，
  任何 ≥1440 的屏幕上这张卡都恰好这么宽，是常态不是极端。
  `state = writeback_stalled` 的行另起一行
  摊开写回原话：那是「窗口没放掉」而不是「正在跑」，两者在表格里长得一样。同屏还带
  `staging_window_blocks`（`increment_update_attempt` / `window_block`，**跨重启活着**，
  内存窗口清空之后它是唯一还说得出话的证据）与 `staging_commit`（上次提交耗时与重试次数，
  摆脚注不摆表里——窗口提交完就没了，这两个数还在，进表会被读成「还有一个窗口」）。
  三条免责声明钉在卡上，少一条这一屏就会骗人：**空不等于干净**（没有批次在跑、这一批走
  直写没建窗口、窗口刚被废弃，在这一栏上是同一个空白）；**这一份活在进程内**，重启必然
  为空，那时该问的是水位表；**档位阈值来自 `AIOS_STAGING_*` 环境变量**，面板不知道数值，
  所以只报档位、不画进度条——编一个百分比出来比不画更糟。字段缺席（老后端不报这一栏）与
  空列表分开渲染，`staging_window_blocks` 落进 `degraded_sections` 时明说「此刻是未知不是零」。
  上次提交耗时为 0 写成「本进程还没提交过窗口」而不是 `00:00`：后者恰恰是「窗口一直没收口」
  的旁证，跟「提交是瞬时的」不能同形。
  落盘那一格（`BatchFailure.staging`）同批也画进了错误日志卡：残留时逐个列窗口名、`sesno`
  区间、状态与档位，写回卡住的另附原话；空时明说「已无暂存窗口挂着」，并点出它是**记录时的
  快照**、与上面那张进程内的卡不是一回事——它活得过重启，而 8191 现场恰恰是重启之后才来问的。
  档位在两处走同一张 `SW_BAND` 表：一处写「拒绝扩窗」一处写 `refuseabsorb`，读的人得先花时间
  确认它们是不是同一件事。

- **`init_mdb` 不再是空壳：启动时从 SYS 库文件解出本 MDB 声明的库清单**。
  「哪些字典库适用」是 MDB 的属性而不是项目目录的属性——`/ALL` 声明的六个字典库
  有四个在 `AvevaCatalogue` 与 `SCB` 底下，靠扫目录猜会两头错（第一次手工建 UDA
  快照就漏两个、多一个），而且漏一个字典库跟「这个 UDA 没值」长得一模一样。新增
  `src/data_interface/mdb_membership.rs`，在 manager 进 `Arc` 之前解析 `CURD` 并
  登记进程级注册表，按 `STYP` 分类取用；它读文件而不是 SurrealDB，所以在任何东西
  被同步之前就能回答（`UpdateScope` 那条查询要等 SYS 先同步，拿它定字典范围是
  死结）。声明了却不在盘上的库逐条告警——那是部署缺件，不是「本 MDB 没有这个库」；
  解析失败只告警不中止（尚无消费者）。DICT 的 `STYP` 按 AMS SYS 实测钉为 8，
  aios_core 的 `DBType::DICT = 6` 在该库里一个都配不上。文件定位复用 extract-family
  命名解析（ADR-028）而非后缀匹配：`ends_with("100_0001")` 会把 `ams8100_0001`
  也吞进来无声配错库，无后缀主库与非 `_0001` 抽取又是同一逻辑库的合法身份、原先
  永远配不上被误报缺件；抽取叶子压过主库，多命中先排序再取，两条回归测试各钉一个
  缺陷。附 `mdb_dict_probe`：独立跑同一套解析，给 `e3d-descriptor emit-uda-table`
  打印现成的 `--dictionary-db-list`（`CURD` 序，`UKEY` 首定义胜出）。

- **`/health` 新增 tokio 调度延迟，CPU 段挤掉调度这件事第一次有数**（specs/033 T003）。
  ADR-052 说要把几何 CPU 段挪出 tokio worker，理由是三角化和布尔占住 worker 会挤掉
  shape receiver、SurrealDB response、watcher、`/health` 与 timer——但这句话在改动前
  无法证伪，手上只有「/health 感觉有点卡」这种印象。新增 `src/runtime_lag.rs`：一个
  只睡觉的采样任务，量自己「睡 100ms」实际睡了多久，超出的部分就是它排队等 worker
  的时间，在 `run_cli` 定死几何额度之后立即起（`run_app` 转手调 `run_cli`，`OnceLock`
  保证只起一次）。每轮独立计时、不补追落后的轮次：一次 3 秒卡顿应当留下一个 3 秒的
  样本，而不是三十个越来越小的样本把 p50 拉回正常。512 样本的滚动窗口之外单独保留
  进程期最坏值——现场最狠的那次卡顿常常发生在几十分钟前，只留滚动窗口等于没留。
  `sampling: false`（采样没起来）与「延迟为 0」在 `/health` 上分得开；分位数复用
  `model_concurrency::percentile`，两个区块的 p95 是同一把尺子。测试里顺带钉住一件
  容易误读的事：五个样本撑不起 p99，最坏值只能看 `max_micros`。

- **几何闸开始记许可持有时长，闸利用率第一次算得出来**（specs/033 T002）。原来只有
  在飞、在等和累计**等待**时长，没有累计**持有**时长——于是「额度开到 8 到底兑现了
  几路」这个问题在现场无法回答，只能靠 CPU/wall 事后倒推。现在许可从拿到手那一刻起
  算持有，`GeometryConcurrencySnapshot` 多出 `active_permit_micros` 与
  `observed_micros`，利用率由 `utilization_since` 对两次快照做差得到。刻意不提供
  「进程至今」的平均利用率：现场的形态是模型本体 258s 跑完、之后等死信干耗 22 分钟，
  把空闲摊进分母得到的是一个既真实又没有意义的小数。计量同时从模块级 static 收进闸
  实例，单测里的临时闸各记各的，读数不再被同一个测试二进制里并行跑的用例污染。
  三条新测试钉住：利用率大于 1 不夹回去（那是计量出错的信号，藏起来等于把 bug 显示成
  100% 健康）、墙钟增量为 0 时返回 `None` 而不是编个 0、排队时间不计进持有（额度 1
  时持有不得超过闸自己的墙钟，这条上界是结构性的，不看机器快慢）。当前许可仍罩着
  整个 `Future`，所以这个读数现在是「许可被占住多久」而不是「CPU 忙了多久」，两者要
  到 ADR-052 的执行域切换落地之后才重合——模块文档里写明了，免得被当成 CPU 利用率
  写进结论。

- **ADR-052 转 Accepted，`specs/033-geometry-execution-domain` 补齐计划与任务**。
  几何并发额度此前装在整个 `Future` 上，一张许可里同时罩着 SurrealDB 查询、跨
  `.await` 持有的暂存 mutex 和同步文件写，于是 `active` 与真实 CPU 占用脱钩、
  16 张许可可以全在等同一把暂存锁（容量倒置），现场 `permits = 8` 只兑现约 2.3 路。
  plan 按 ADR-052 六条决策拆成六个阶段（可归因基线 → 执行域地基 `run_gated_cpu`
  → `manifold_bool` 首站 → 其余叶子与动态领取 → SQL 攒批解耦与 shape 观测
  → 控制器归位与正式 A/B），tasks 三十三条逐条带文件路径与并行标注。Constitution
  Check 记下两处偏离：新增专用有界执行域与 `model_concurrency` 模块头「不创建新线程池」
  的措辞冲突，以及 ISSUE-023 未闭合时「进程内全局额度」在 CentOS 7 上不成立——
  后者登记为发布阻断项，不是可以边做边看的风险项。与 `specs/032` 的排他也写进计划：
  `cata_model.rs` 与 `gen_model.rs` 同一时间只允许一条线在改。

- **属性 schema 换成描述符权威表，noun 从 339 涨到 1878，已有取值一条没变**。
  `all_attr_info.json` 由 `e3d-descriptor emit-attr-info` 从 20 个 `*vir.dat` +
  `attlib.dat` + 逻辑目录合成，`--legacy-attr-info` 把老表覆盖到的地方逐字照抄，
  所以「不回退」是构造性的而不是靠测试兜：老表 6556 对在 `name` / `hash` /
  `offset` / `att_type` / `default_val` 五个字段上逐条相等。规模落在计划靶心：
  20 库 / 1875 noun / 35734 三元组 / `flat_conflicts` 恰好 14。产物 35655 对，
  比 2026-08-26 那份中间件多 1023 对——那 1023 条是 `TYPE` / `LOCK` / `OWNER` /
  `CLAIDB` / `UDNA` 这类描述符里没有的结构属性，中间件把它们丢了，新表按
  「老表 ∪ 新表」保留。代价写在这里：`--legacy-attr-info` 保证不改值，也就保证
  不纠错，`SLOREF`（表说 STRING、描述符说引用）与 `MDSYSF`（表说 ELEMENT、
  描述符是 21 字的非引用）两条错误类型原样留着，测试里具名钉住。

- **新增离线核对设施：一个元素的全属性（含 UDA）第一次能不连库复现 `q att`**。
  `all_attr_info.json` 描述不了 UDA——UDA 不在 `*vir.dat` 里，E3D 把它存成字典库
  下的 `UDA` 元素（`UKEY` / `ELEL` / `DFLT`）。新增 `src/uda_table.rs` 读
  `e3d-descriptor emit-uda-table` 导出的字典快照，`full_attribute_view()` 把
  schema 默认值、UDA 字典默认值与记录里真存的值三层叠成一张表。**运行时取 UDA 的
  路径一行未动**：仍是每个项目自己的字典库同步进 `UDA` / `ATT_UDA`。快照是夹具不是
  配置，两张表都作为参数显式传入，不设隐式默认——UDA 定义属于某一个 MDB，能被环境
  悄悄替换的默认值在这里等于埋雷。夹具 `tests/fixtures/AvevaMarineSample_uda_info.json`
  跨三个项目五个字典库：`:SCHrefHole` 来自 SCB、`:PFILoose` 一家来自 AvevaCatalogue，
  只取 AMS 自己的字典会缺 BRAN 27 个 UDA 里的 5 个。
  `tests/full_attributes_real.rs` 拿 E3D 实打实的 `q att` 当权威，覆盖两条路：
  PIPE `=24383/73958` 79 行全部复现（20 个 UDA 全是字典默认值），BRAN
  `=24383/85432` 覆盖存值那条（`:ROOM_NO = NB122`，`UKEY` 离线解名，不查
  SurrealDB）。诊断入口 `src/bin/refno_attr_probe.rs`，另带 `--scan-uda` 全库扫描
  ——7999 库 44535 个元素里 1858 个真的存了 UDA 字节，所以那条路不是理论存在。

- **连败卡上加「只重扫这个库」，把全范围那一推收窄到一个 dbnum**。此前面板只推得动
  全范围（`POST /update/execute` 空体），而人在连败卡上想做的事从来只是「把这一个再试
  一次」——为了一个库去扫全范围，代价是别的库跟着入队、水位跟着动。服务端其实早就认
  `dbnums[]`（ADR-020 第 3 项的子集选择），缺的只是面板这一侧的入口。
  **名字叫「重扫」不叫「重跑这一批」**：它不是把那段冻住的会话窗口原样重放，而是从水位
  重新扫一遍再入队；文件没长出新会话的话算出来还是同一个窗口，于是确定性失败会一字不差
  地再来一遍。这句话与「SYS meta（SYST/DICT/GLB/GLOB）不是可勾选对象、永远随批」一起
  写在确认行上——不说的话，点完看到同样的报错，人会以为按钮没生效；而「只推这一个」
  在 SYS meta 上根本不是字面意思。回执新增 `本次没勾 N` 一栏（`unselected`，只在带
  `dbnums[]` 时才有），否则看着「入队 1」会以为其余的库也被顺带处理了。
  按钮**不自己发请求**，只是把页顶那一行确认填好：写操作全页只有那一处出口，确认样式、
  二次确认、回执展示都在那儿，在卡片里再写一遍就会有第二套。那一行在页顶而这张卡在半屏
  以下，所以点完顺带滚过去——确认摆在看不见的地方等于点了没反应。`act()` 的确认态因此
  改为按 **key + 目标**一起认：只认 key 的话，先点 8191 再点 3021，第二下会被当成对 8191
  那一下的确认，而这两条命令推的是不同的库、屏幕上却只有一行请求在变。`?readonly=1`
  下这个按钮不长出来，与顶部那排同一道门。

- **错误日志卡加 `task_id` 与时间范围两个筛选维度，并把「筛的只是取回来那一窗」说出来**。
  此前只筛得了 `event` 与 `dbnum`，而排查里最常问的两句是「**这一次**任务在磁盘上留下了
  什么」和「**只看最近这一阵**」。`task_id` 用包含匹配——真实 id 是
  `db-20260827-114844-000000` 这种，要人一字不差敲全等于这个格子没人用；任务下钻里那颗
  `→ 这一条的落盘记录` 填的则是整条 id，包含匹配一样命中。时间范围给四档现成的
  （不限 / 近 1 小时 / 近 24 小时 / 近 7 天），下界按此刻算：这一格问的是「最近这一阵」，
  不是一个冻住的区间。
  类型计数改为按**其余筛选之后**的集合算：勾着「近 1 小时」时「批次失败 12」若数的是
  全窗，点下去只出来 2 条，那两个数里必有一个在骗人。
  同批补上两句这张卡一直欠着的话。一是**窗口边界**：筛选在前端做（切一下不用等一轮网络），
  所以它筛的是取回来那 40 条而不是磁盘上全部；窗口一旦是满的，任何筛选下的「0 条」都
  可能只是被切在了外面，此时把去磁盘上重问的那条 URL 拼好摆出来，并注明服务端只认
  `kind` 与 `dbnum`——`task_id` 与时间范围它不认，这两个维度天然只能在窗内筛。二是
  **park 记录没有 `task_id`**（它记的是某个库连败到上限，不属于哪一次任务），所以按
  `task_id` 筛必然把 park 全灭掉；不说的话，「这条任务没被 park」与「park 压根不带
  task_id」在这一屏上是同一个空白。跳转一律先把其余筛选清干净再落一个：留着上一次的
  （比如还开着「近 1 小时」），跳过去多半是一屏「没有记录」，而人会把它读成「这个库没出过错」。

### 修复

- **初始化解 MDB 声明时的同号选主，改用摄入侧那套项目排名**。`/ALL` 的成员按 dbnum
  声明，而 dbnum 只在项目内唯一——`AvevaMarineSample` 与 `AvevaCatalogue` 在同一份
  配置里就同时有一个 7000。过去 `mdb_membership::locate` 把所有配置项目的库目录混成
  一个列表、只按号匹配，靠**路径字典序**决胜，两个分支方向还相反：抽取叶子取字典序
  最大的路径、无后缀主库取最小的。AMS 那份 7000 能被选中纯粹因为
  `AvevaMarineSample` > `AvevaCatalogue`——改一个项目名、给外项目一个 `_0002`、或者
  现场用无后缀主库部署，同一份声明就会静默绑到另一个项目的文件上，**而这份名单正是
  UDA 字典的来源，从错的项目读字典跟「这个 UDA 没值」长得一模一样**。它同时与摄入侧
  各说各话：`select_catalogue_candidates` 早就按 `catalogue_project_priority` →
  `included_projects` 的顺序选主并打 `[manifest] … 被项目优先级遮蔽`，一个进程里对
  「7000 是哪个文件」给出两个答案。现在 `locate` 拆成「项目内按 ADR-028 挑」加
  「跨项目按同一套排名选主」；SYS 记录里一直被解析掉的 `PROJ` 也留了下来，压在排名
  之上决定**先看哪个项目**，且**只排先后、不做排除**——`*MDU/CATA`(7355) 声明
  `PROJ=3` 而 `AvevaMarineSample` 是唯一有文件的项目，硬分桶会把一个解得出来的库
  报成部署缺件。`PROJ` 也确实只够用来排先后：`AvevaCatalogue` 的库与未部署的
  `SCB` 整块（`6000`–`6003`）读出来都是 3，它区分不出是哪一个外项目。落选的同号
  候选不再静默丢弃（`MdbDatabase::shadowed`），文件归属项目一并记下
  （`MdbDatabase::project`），启动日志与 `mdb_dict_probe` 逐条打出来。
  `the_ranking_agrees_with_the_ingest_side_adjudicator` 把两边摆在一起断言同一个
  赢家，谁改了任一边的口径谁红。实测沙箱：7000 解成
  `[AvevaMarineSample] ams7000_0001` 并遮蔽 `acp7000_0001`，与 `[manifest]` 那一行
  逐字对上。

- **`init_mdb` 的字典库计数不再自相矛盾，缺件自己点名**。过去那一行左边写
  「按 STYP {8: 6}」、右边写「其中字典库 5 个」，而唯一解释这 6→5 的那句话走
  `log::warn!`——`enable_log = false` 时 logger 压根没初始化，2026-08-27 沙箱的
  stdout 与 stderr 两份日志搜下来零命中，差额就这么在同一行里自相矛盾地摆了一整天。
  现在报数行同时给出「盘上几个、缺几个」，缺的字典库逐条 `println!` 点名（带 dbnum、
  名字、`PROJ`，并写明它定义的 UDA 之后一律读成「没值」）。非字典缺件合成一行：
  它们不像字典那样会把缺失伪装成一个合法的空值，逐条铺开只会把上面那几行冲掉——
  而合成一行又恰好让 `6000/6001/6002/6003` 这一整块 `*MASTER/SCB*` 并排出现，
  「SCB 这个项目整个没部署」的结论自己就跳出来了，过去它们分散在五条看不见的 warn 里。
  `log::warn!` 保留给有 logger 的部署。一条读自己源文件的顺序断言钉住三件事：报数行
  必须同时给两个数、缺件必须排在它之后逐条点名、且那一段里必须有 `println!`。

- **面板上所有 `<details>` 此前每 2 秒自己合上一次**。`render()` 是整块换
  `innerHTML` 的，而它每个轮询周期都跑一遍，于是刚点开的「原始记录」「伴随告警」在人
  把那段 JSON 选中之前就没了。这块折叠区恰恰是为「排查到最后总要把原话贴给别人」准备的
  ——它打不开，等于没有。现在展开状态记在 `S.openDetails` 里，重画时照着把 `open` 写回去；
  键取记录自身的身份（`task_id`、`dbnum` + 时刻）而不是序号，列表一重排序号指的就是另一条
  记录了，人会看着一段 JSON 在眼前换成别人的。`toggle` 不冒泡，所以监听挂在 document 的
  捕获阶段。错误日志卡、连败账本、新的任务下钻三处共用这一条。

- **model_impact 对账快照跟上 descriptor 属性表**。339→1878 换表（d32b06ff）让六个此前
  「字典有名、快照无属主」的属性有了属主——CACHID / LCHKDA（业务元数据表）、FSPREF / SCREF
  （级联表）、PTOF / SIZE（直接几何表）——而
  `curated_tables_are_reconciled_against_the_runtime_schema` 的钉死名单没跟着走，全量 lib
  一跑就红。按新 schema 重钉：DATA_ONLY 只剩 FUNCTION，级联 13→11，直接几何 39→37。
  这条测试的价值恰在于此：换表时逼着人把「哪些名字从此有主了」过目一遍，而不是让对账悄悄失真。

- **RVM 对拍里那条「什么都没量到也报绿」的门补上了下限**。
  `mesh_pipe_surface_distance` 是取证型用例，此前**一条断言都没有**：RVM 里找不到 group
  就 `continue`、库里没有生成几何也 `continue`，于是「九件件件贴合」与「一件都没量到」
  共用同一个退出码 0。2026-08-24 的作废通告里那唯一一条「1 passed」就是这么来的——在一个
  没有 1112/8000 生成几何的库上，它是八条里唯一报绿的，而它恰恰什么都没量到。现在结尾
  断言九件件件量到、跳过谁点谁的名。顺带把这条门的口径限制写进注释：C-OR 九件在 E3D 侧
  全是 12 三角，两个 BEND 也一样（gen 侧 618 / 908 三角），所以它量的是「gen 细网格贴不
  贴 E3D 粗替身」，量不到真实弯头形状；弯头的可信判据在
  `mesh_branch_union_surface_distance` 与 `mesh_full_branch_union_surface_distance` 的整段
  union 上（腿归属差在装配层自洽抵消）。同轮八条 mesh 对拍在
  `.surreal/ams-rvm-rebuild-20260824` 上 **8 passed / 0 failed**（44.1s），GWALL 三个可归因
  读数与 08-25 逐位相同——自那以来落地的改动没有动到几何。

- **失败批次在控制台上自己说出原因，不再把人指向一个当时拿不到的回执**。完成行只报
  `状态=failed`，真话在 `result.batch.message` 里，而那句话此前**只**进任务回执，屏幕
  上唯一的提示是「失败原因见本批回执」——取回执要 HTTP，现场可能没开 `http_api`、端口
  不通、或者人根本不在那台机器前面。2026-08-27 的 SYST 8191 就是这个形状：一整屏阶段
  日志把「死在收集增量这一步」说得清清楚楚，唯独缺了「为什么」，而收集口十几个各自具名
  的硬失败出口只能靠那一句分辨。现在 `render_failure_reason_lines` 紧跟完成行打印，三条
  纪律各带一条测试：① `batch` 缺席（冻结重扫就失败、批次压根没建起来）时回落到
  `warnings.last()`，并**报出这一句是从哪儿来的**——两个来源权威性不同，混成一句会让人
  把净窗口的口径标注当成错误；② 伴随告警只出前 3 条并报剩余条数与 `/api/v1/tasks/{id}`
  ——一条口径标注就上百字，整串打出来会把上方的阶段行冲掉，而阶段行正是判断死在哪一步
  的依据；③ 一条都没有也仍留一行，点名「引擎缺陷」，静默失败比错误的原因更难查。成功
  批次一行都不加。「初始化批次未收口」那句同步改口指向下方的原因行，源码顺序断言钉住它
  不许与「见本批回执」并存。

- **模型队列整页空转：生成器回报的根与队列行的 `target_refno` 拼法对不上**。
  2026-08-27 现场 1462 个根连着 118 页 `page_claimed=100 / page_completed=0 /
  remaining=1462` 烧掉 50 分钟，而每一页的任务都报 `succeeded`、`/health` 一路 `ok`、
  `attempts` 全是 0、死信是空的、日志一个字都没有。根因是同一个身份的两种写法：队列
  行存 E3D 的 `24381/100817`，而 `TargetedGenerationReport` 里那一串经 `RefnoEnum`
  走了一趟、`Display` 打成 `24381_100817`。`run_regen_group` 拿两边直接比字符串，于是
  `completed.contains(&job.target_refno)` 恒为 false（`settlements` 为空 →
  `clear_regen_work_batch` 一行不删 → 队列行原样留着被下一页再认领），`failures` 里那
  一串也配不上任何 job（`record_failure` 一次不调 → `attempts` 永远 0 →
  `MAX_ATTEMPTS` 永远够不着 → 死信永远是空的）。「收不掉」与「没失败」同时成立，所以
  它安静得跟正常跑完一模一样。这行比较由 `5f7ef21f`（08-26，合批生成那个特性）引入，
  两侧同出一个提交，事故发生在 08-27——不是潜伏很久，是落地即坏，第一次跑大批就现形。
  现在两边都过 `root_identity_key` 归一再比；解析不出来的
  **原样返回**，不能 `unwrap_or_default()`——那会把所有坏值折成同一个零值，两个不同的
  坏根反而被认成同一个。`settlements` 本身仍存队列行自己的写法：它要拿去寻址那一行，
  不是拿去比（ADR-011 队列纪律：寻址按行内实际字段）。单测钉住两种拼法归一、不同根不
  许归一、解析不出的原样留着，外加调用点守卫（改回直接比字符串即红）。
  `live_bran_pending_is_actually_regenerated` 一比一复现了这个形状；归一后 @8019 六条
  相关 live 全绿（BRAN / HANG / ZONE-EQUI / `live_generation_failure_keeps_pending_and_watermark`
  / `live_failed_queue_cleanup_does_not_stall_the_rest` /
  `live_non_regen_drain_consumes_the_whole_queue`）。证据
  `docs/evidence/2026-08-27-model-drain-root-key-mismatch.md`。

- **模型页整页空转不再无声：未回报的根按失败计次，页级停滞进 `/health` 与启动等待行**。
  上一条那种「收不掉又没失败」此前没有任何东西会发现，两道兜底同批补上。①
  `run_regen_group` 里生成没报错、根却既不在完成名单也不在失败名单时，那一行无声落地，
  下一轮被原样认领、再无声落地一次。新增 `settle_unechoed_roots`：点名（有界样本）并走
  `record_failure` 计次，撞满 `MAX_ATTEMPTS` 进死信。这与 2026-07-30 审计 C2「收口失败
  不涨 attempts」是两回事——C2 防的是 flaky 的 DELETE（根已经进了收口集合，下一轮重试
  就好），而这里根本没有 DELETE 可言，根压根没进得了收口集合，重跑一万次结果都一样；
  `settlements` 因此必须先并进 `disposed` 再做清扫，源码顺序断言钉住，否则 C2 修掉的
  那个 bug 会原样回来（`live_failed_queue_cleanup_does_not_stall_the_rest` 复验通过）。
  ② `ModelDrainTelemetry` 那三个数（`last_page_claimed` / `last_page_completed` /
  `last_remaining`）一直都在，却没有任何人拿它们做判断。新增 `starved_pages` 在认领下
  一页时结算上一页，三个条件缺一不可（认领过、零收口、待办总数纹丝不动）——只看
  `completed == 0` 会把「整页被根锁挡住」这种正常让位算进来，只看待办不变会把「收掉几个
  又排进来几个」误判成停滞。连撞 `MODEL_DRAIN_STARVED_PAGES_ALERT = 3` 页（现场每页约
  26 秒，≈80 秒）即判定停滞，推进 `/health` 的
  `blocking_conditions.model_drain.page_starved`（`degraded`）。启动等待行的「数字不变
  就退到 300 秒一行」这条退避在卡死时是反的，越卡越安静：现场前 19 分钟每 60 秒一行，
  真卡住之后反而变成 300 秒一行。停滞一旦确认就按 60 秒照常出声，并把停滞页数带上。

- **启动覆盖回填写进队列的根拼法与其余入队方不一致**（ISSUE-024 名单里扫出来的第三处）。
  `sync_and_seed_model_coverage` 用 `pe_thing_to_refno(..).to_string()` 落 `target_refno`，
  那是 `RefnoEnum` 的 `Display`、下划线 `A_B`；而增量、级联、按需生成三条路走的都是
  `to_pdms_str()` 的斜杠 `A/B`。`record_id_of` 会把斜杠折成下划线，所以两种拼法抢的是
  **同一行**——不会长出重复行，但字段值变成「谁最后 upsert 谁说了算」。代价落在那些拿
  字符串精确查这个字段的地方：`staged_settlement_revision` 经 `current_regen_revision`
  查 `target_refno = '<斜杠>'`，撞上一行当前存着下划线就命中零行、返回 `Ok(None)`——与
  「本来就没有这行」完全无从区分，于是本窗口跳过收口，那个根在提交后的空闲轮里被原样
  重生成一遍，而只有 `Err` 那一支会出声；`retry_pending_unit`（死信人工复活的唯一 HTTP
  出口）同样按精确字段寻址，拼法不对读起来就是「这行不存在」。现在统一成斜杠。新增一条
  测试先钉住前提（两种拼法确实不同、且折成同一个 record id），再用源码断言守住这一处
  不许退回 `Display` 拼法。**库里已存的行不迁移**：`record_id_of` 归一保证下一次入队会
  把字段改写成斜杠，从没再被入队过的行仍是旧拼法，根治见 ISSUE-024 的 newtype 方案。

- **WS 事件流的掉帧终于露得出来：滞后错过的那几条也要占号**。信封的 `seq` 是在
  **发送时**才分配的，而慢消费者滞后（`RecvError::Lagged`）那一臂什么都没做，于是
  丢帧之后编号照样连续——客户端「相邻两号差 1 就是没丢」的判断永远为真。这比没有
  掉帧指示器更坏：它是一句错误的保证，一条残缺的事件流在面板上与完整的流长得一模
  一样，而 `ops.html` 的实时事件面板正是照着模块头那句「滞后跳帧只体现为 seq 空洞」
  建起来的。现在滞后把错过的条数补进 seq，空洞真的出现在客户端那一侧，它据此提示
  「以 REST 读数为准」。计数偏保守：`missed` 含订阅之外的事件，那些本来也不占号，
  所以空洞可能略大于真实丢失——宁可说「最多漏了这么多」，也不能像原先那样说「一条
  没漏」。编号收进 `SeqCounter`，只增不减、饱和不回绕（回绕会被读成「服务端重连
  了」），撞上限也不 panic：一个计量数字不配把整条事件流拽下水。两条测试：编号语义
  （退回空分支即红）、以及滞后那一臂确实在跳号的源码断言（纯函数钉不住「有没有人
  调它」，而这个缺陷的形状恰恰就是那一臂空着）。

- **闸利用率不再把「正在算」读成 0：许可持有改为读时结算**（specs/033 T002 补）。
  持有时长原先只在许可 `Drop` 时累加，快照落在长布尔段中间时，正被占着的那几张许可
  一微秒都不计——单个 BRAN 布尔能跑几分钟，而 T004 的基线采集与 T013 的 0.7 验收线
  偏偏都要在 CPU 密集段内取样，读数被系统性压低，达标的配置会被判成不达标（额度 1、
  一张许可从头占到尾这种极端形态，旧写法读出来是 0.0）。账本改记两笔——已归还许可的
  持有之和、在飞许可各自的取得时刻之和——读数时补上「在飞张数 × 当下 − 取得时刻之
  和」。两笔是耦合量，各用一个原子量读会撕出一整段凭空的持有，因此收进同一把 std
  `Mutex`（取还许可的频次就是几何叶子任务的频次，锁内只有几条整数运算、不跨
  `.await`）；「当下」也必须在同一次持锁内取，否则在飞那项会算成负数被 saturating
  抹平回 0——那正是要修的形态本身。快照方法挂到闸实例上，单测的临时闸各读各的账。
  两条回归测试各钉一个形态（段中读数不为 0 / 一张许可占满整窗读作 1.0），退回旧写法
  双双转红。读数口径不变：许可仍罩着整个 `Future`，这是「许可被占住多久」而不是
  「CPU 忙了多久」，两者要到 ADR-052 的执行域切换落地才重合。

- **非 Windows 也有真实的进程单实例锁了**（ISSUE-023 / specs/033 T001）。
  `acquire_process_instance_lock` 的非 Windows 分支曾是一个空 `Ok(())`：部署到
  CentOS 后第二个进程直接放行，成为同一份 dabacon、同一个 SurrealDB、同一批 mesh
  的第二个写入者，几何闸额度、暂存串行、启动清理这些「进程内全局即全局」的前提
  一起失效。现在 Unix 走 `flock(LOCK_EX | LOCK_NB)`（`File::try_lock`）：锁挂在
  open file description 上，与手柄同生共死，SIGKILL 后由内核回收——刻意不做
  「看锁文件存不存在」那种会留陈旧锁的写法；冲突时读回持锁者写进文件的
  project/pid/started_at 一起报。函数体抽成 advisory 辅助在**所有平台**编译
  （Unix=flock、Windows=LockFileEx 同形），守卫测试从 `#[cfg(all(test, windows))]`
  改为三条跨平台——这个缺口的成因正是非 Windows 代码在 Windows 机器上零编译
  零测试。Windows 生产路径维持 deny-share 不变。真机 Linux 复跑仍待办（登记在
  issue），在那之前性能结论照旧只按 Windows 单实例口径引用。

- **UDA 快照的来源从「扫目录猜字典」换成 MDB 声明本身**。
  `AvevaMarineSample_uda_info.json` 重建为 `/ALL` 声明的六个字典库（用
  `mdb_dict_probe` 打印的 `CURD` 序清单），`all_attr_info.json` 同批刷新。「快照说
  适用但 `q att` 不打印」的台账从 2 条 `Psi*` 扩到 5 条（补进 `:MDSComment` /
  `:MDSDType` / `:MDSTrun`），按名字与数量双断言钉住；五条的共同点是归属应用
  （PSI 应力 / MDS 综合支吊架）而 `q att` 跑在 Design 模块下——模块过滤能解释它，
  但当前解码的字典元素里没有任何字段支持这个说法，先记为开放问题不猜。测试文档
  同时写明部署警示：`SCB` 不在 `included_projects` 里，`scb6002_0001` 会被报成
  不在盘上，而 `/ALL` 声明了它、`q att` 也真打印它的 `:SCHrefHole`。

## 2026-08-26

### 修复

- **启动模型覆盖核对补上未限定域的口子，且不再中止启动**（ADR-050 补偿闭环）。
  ADR-050 的启动清空是无条件的，但补偿（生成根凭证核对与补种）此前只挂在监听
  限定域上：未声明 `watch_dbnums` 的进程在模型积压期间重启，剩余工作单被清掉后
  没有任何东西重建它们——watcher 重扫按水位比对不再排模型工作，`model_ready`
  仍按 pending=0 判真，缺的模型静默消失。现在启动核对由
  `reconcile_model_coverage_at_startup` 统一承担：限定域照旧逐库
  `fn::sync_gen_roots` + 补种；未限定域改按 `gen_root` 凭证点查（`gen_root_dbnum`
  索引，一次 GROUP BY 列清单），发现凭证过期才对那一个库做完整 sync + 补种——
  全库跑 `fn::gen_root_cover` 在 448 库的现场付不起；没有凭证记录的库当面播报
  「核不了」，不许静默当作完整。同时任何一库核对/补种失败都只告警不再 `?` 上抛：
  原写法会把一个 watch dbnum 的配置手误（或 `sync_live=false` 空库首启）变成整个
  服务的启动崩溃循环。内存库测试钉住清单查询跳过无归属行，源码测试钉住
  「不得只挂限定域」与「不得中止启动」两条。

- **人工执行在宽松身份冲突档下不再虚计阶段总数**。watcher 路径对「仅记录并跳过」
  的库会 `manifest_totals.retain(..)` 摘掉阶段总数，人工路径此前不摘：默认配置
  （`block_file_identity_conflicts=false`）下人工执行遇到身份异常文件，/health 的
  阶段 total 会为一个永远不会来的批次虚高一格——正是此前修过的「阶段就绪显示失真」
  形态。现在人工路径把这类库记入 `manifest_lenient_excluded`，收集总数时排除；
  两路径共用性测试同步加严。

- **模型生成的三处小收口**：根级失败摘要（「N model root(s) failed: 前三条」）三处
  拷贝合并为 `occ_generate::summarize_root_failures`；按需 ensure 在没有在飞重建时
  不再为 `reject_ensure_during_rebuild` 多付一次数据库往返；adaptive 并发额度裁决
  抽成纯函数 `next_effective` 并把原来只断算术常量的测试换成对裁决本体的六条断言
  （压力减半有下限、进展加一有封顶、空窗不抖动、压力优先于进展）。

- **空闲模型页的进度簿记收进一个类型**。`drain_where_cooperative` 此前用一个
  7 参自由函数在四个调用点重复上报任务 detail 与 telemetry，收口/让位/锁挡三种
  终态又各自手拼 finish——现在认领时定死的 task_id / queue_total / page_claimed
  钉进 `DrainPageProgress`，子阶段推进走 `report`、页终态走 `settle` /
  `settle_deferred_for_lock` 一处收口。行为逐位不变（detail 缺失时整个跳过的旧
  语义保留），既有源码钉（合批先于逐根循环、探测先于消费、锁覆盖结算顺序）与
  全模块 76 条测试复跑通过。

### 新增

- **新增属性描述双读对拍探针 `attr_info_compat_probe`**。它加载解析库生成的
  `PdmsDatabaseInfo` 兼容文件，先逐项验证内嵌旧 schema 全部保留，再用新旧两份
  schema 解析同一 Dabacon 文件的最新记录并逐属性比较，为逐步退役
  `all_attr_info.json` 建立不切换生产路径的验收门。

- **E3D TTY 与导出的 AMS Agent 使用教程**。集中说明当前权威入口宏通道、AMS 8000
  FTUB apply/restore、RVM geometry-only、RVM+ATT 成对发布、全局 noun/属性字典导出、
  证据与回滚判据，以及继续提交正常服务增量并检查 task、水位、staging、side effect、
  worker 和 health 的完整退出门。

## 2026-08-25

### 修复

- **`watch_dbnums` 同时收窄 `inst_relate` 缓存维护，启动不再被一次全表扫压住**
  （ADR-048 决策 2 的延伸，specs/025 的一段前置）。`insts_flat` 上没有索引，命中
  稀少时 `LIMIT` 一格也省不下来——引擎要把整张表走完才敢说「不足一批」，而
  `aabb.d` 是记录链接，每条 `insts_flat = NONE` 的行还要多付一次 `aabb` 点查
  （issue #21 在库 A 的普查：NONE 行里约 97% 是读者不可见的）。448 个 dbnum 的
  现场库上，启动序列停在「正在清扫 inst_relate 平表副本」不再往下走。现在回填
  圈行、脏值探测、RM13 修复循环与复核、老格式再现探针这五条全表谓词，在声明了
  限定域时一律带 `dbnum = …` / `dbnum IN […]` 前缀走 `idx_inst_relate_dbnum`；
  列名裸着出现，不套 `(dbnum?:0)`——那会把索引藏进表达式里让 planner 回落全表。
  **收窄跑过的 RM13 migration 不落完成标记**：标记的含义是「这一库全表收敛过」，
  域内干净担保不了它，落了域外那批老格式行就永远没人再看。收窄时启动日志点名
  限定域、来源，以及「域外的行与 dbnum 为 NONE 的历史行这一轮不维护、读侧走 slim
  兜底」。不声明限定域时逐位不变，全表扫照旧。
- **`project_dirs` 恢复为项目目录的同位置映射**。`included_projects` 继续作为唯一项目
  范围；提供 `project_dirs` 时，watcher、初始化解析、CATA 路径和进程单实例锁统一使用
  其中同下标的真实文件夹名，不再错误拼成 `project_path/included_projects`。映射缺项或
  非单层文件夹名会明确报错，名单外项目仍不会进入扫描范围。

- **7997 并发版现场启动修正**。生成根覆盖查询把 `subtree` 加入投影，兼容
  SurrealDB 2.1 对 `ORDER BY` 字段必须出现在 `SELECT` 中的限制；adaptive 的 RSS/写入
  基线改为在整个 K=1 观测期累计峰值，避免用进程刚启动的低水位样本把并发永久误压为1。
  `test-worklspace` 实测识别2720根、保留528个当前凭证、补种2192根，并观察到 K=1→2→3
  后按 AABB/RSS 压力回落，期间零失败、零死信。

- **7997 模型完整性与根级并发生成闭环**（ADR-011 / ADR-025）。启动数据阶段后用
  `fn::sync_gen_roots` 对齐权威生成根，完成凭证必须与当前 `applied_sesno` 及保存时刻
  一致；缺失根进入原有 `regen_root` 队列，`model_ready` 不再把 `pending=0` 当作模型
  完整。每页仍预取 100 根，但只按 16 根 execution group 获取根锁，mesh/布尔后半程
  有界并发、Shape writer 与 AABB 提交保持单路；健康接口新增模型、Shape 和 geometry
  并发遥测。新增 `POST /api/v1/dbnums/{dbnum}/model/rebuild`，范围校验、幂等任务、旧模型
  保留和水位不变均复用现有协调器；`legacy` 配置可即时退回串行并关闭端点。

- **7997 首次初始化的模型工作恢复有界消费**（ADR-011 / ADR-025）。
  自动空闲轮不再用无上限 `SELECT` 一次认领 18674 个 `regen_root`；Regen
  固定每页 100 根、后置 AABB 每页 256 条，Regen 清空前的二次探测禁止
  AABB 越过阶段。忙根锁原样延后且不消耗 attempts，全页忙时按 30s 退避。
  `model_drain` 任务只保留 10 个根样本并报告页进度；启动等待日志改为
  真实 Regen/AABB 阶段与剩余数，相同无进展信息最多每 300s 输出一次；
  `/api/v1/tasks` 单次最多返回 160 条，最大响应保持在 256 KiB 内。
- **模型开关不再导致每次启动整库全量生成**（ADR-051）。`gen_model` / `gen_mesh`
  只表示允许增量批次进入模型与网格阶段；服务启动统一经 watcher 重扫，把文件最新
  会话号与 `applied_sesno` 比对后，仅为首次导入、文件回退或真实增量建立工作。移除
  `run_cli` 中对 `gen_all_geos_data` 的直达与延期全量分支；显式探针/工具的全量入口保留。
  启动日志会明确播报当前是水位比对策略，源码回归测试禁止三个全量锚点重新进入启动路径。
- **停止逐会话打印 `[paged_db] path=...` 分页读取统计**。按需读取及快照校验逻辑
  保持不变，仅移除会话销毁时输出的页数、缓存命中和解析记录数明细，避免初始化阶段刷屏。
- **`watch_dbnums` 同时约束持久模型工作单的自动消费**。此前它只挡新数据批次，
  空闲轮仍会把 `model_update_pending` 里范围外的历史根全部捎进来；现场只监听 8000，
  却继续生成 1112 的 5025 个根。现在自动 drain、是否还有活、死信门和 `/health`
  共用 `(dbnum?:0) IN watch_dbnums`，本进程新建的范围外/无库号行不参与自动消费；
  不声明限定域时行为不变，当前批次的精确键 scoped drain 与只读待重试清单不变。
- **启动时先清空 `model_update_pending`**（ADR-050）。这张表只承载本进程内的模型
  调度，不是跨重启恢复日志；增量解析数据与模型数据已从 kv-mem 作为一个整体写回
  RocksDB，重启后重放旧工作单会把两个快照混在一起。现在数据库连接成功后立即全表
  清理，且严格早于空间树、预加载、db manager、watcher、worker 与模型阶段；清理失败
  直接阻断启动。启动日志报告实际删除数，定向内存库测试和源码顺序测试钉住该边界。
- **停止逐批打印 `shape_save_flush` 控制台明细**。实例保存仍按原阈值执行并累计
  `ShapeSaveRunOutcome`，每轮结束的 `shape_save_summary` 汇总保留，避免 MaxWait
  小批次持续刷屏。
- **启动数据身份冲突默认降级为“记录并跳过”**。文件名/文件头库号不一致、同项目
  同 dbnum 多文件仍不摄入、不写观察值，控制台与人工执行回执保留完整告警；但冲突库
  不再把 Meta/Catalogue/Design 阶段永久标成 blocker，其余库可以完成初始化。
  watcher 与手动执行共用 `block_file_identity_conflicts`，默认 `false`；需要恢复旧的
  严格阻断行为时在 `DbOption.toml` 设为 `true`。
- **声明了 `watch_dbnums` 就算启动上弦**（ADR-048）。`startup_autorun=false` 与
  `sync_live=false` 撞在一起时，重扫行挂起、持久积压不消化，而解封条件「某个 dbnum
  真的来一次增量」永远不会发生——watcher 压根没起。现场
  （`DbOption-rvm-rebuild` + `AIOS_STARTUP_AUTORUN=0`）`model_update_pending.retryable`
  卡在 7655 一动不动，进程 33 分钟只吃 74.9s CPU（单核 2.1%），
  `model_drain.last_claimed_epoch = null`：那 7655 行「可消费 / 可收口 / 可复活」
  三条出路一条都没有，对外还长得像「在慢慢跑」。起手上弦改由纯函数
  `batch_scheduler::initial_auto_work_armed(startup_autorun, watch_scope_active)`
  裁决，两个来源任一成立即上弦；限定域仍然只收窄不放宽，**也不拖回启动全量房间重建**
  （那道门直读 `startup_autorun()`，单独钉了测试）。两句启动播报补上第二条出路，
  并在 `sync_live=false` 时额外喊明「不会有文件事件来解封任何一行」。三条回归
  （真值表 / `global()` 源码断言 / 房间重建边界）均实测过「回退旧写法即红」。
  `db_options/DbOption-rvm-rebuild.toml` 同步写上 `watch_dbnums = [1112, 8000]`，
  重启实测：`model_in_flight` false→true、CPU 2.1%→14%，而 `startup_autorun` 仍为 false。
- **网格里不许留一条 f32 表达不出来的缝**（ADR-049）。CFLOOR
  `/1RS-WF03-F-C-F002` 一带三个负体在布尔入口报 NotManifold，三件三角化全成功、
  全死在读回——同一本账的三个常数：① `sin(TAU_f32) ≈ 1.75e-7 ≠ 0` 让回转体环向
  接缝两侧焊不到一起（CTORUS 实测 10 条边界边，与 `2RT+2T=64` 解出的 `T=8,R=3`
  逐项对得上），改由 `ring_angle_table` 建表、`j == segments` 回用 `j == 0`，
  `gen_circular_torus`（管壁与两个端盖共表）/ `gen_sphere` / `gen_spherical_dish` /
  椭圆封头四处接上；② `sin(PI_f32) ≈ -8.7e-8` 把球的南极摊成一圈针状面
  （`2 × slices` 条边界边），两极硬置 `(0, ±1)`——**这条是修 ① 时顺带查出来的，
  现场还没人撞上**；③ `assemble_ring` 的合并门槛 `POS_EPS = 1e-4` 是绝对量，而 f32
  在 1250mm 处的分辨率约 1.5e-4，f64 里刻意留的缝落盘即被抹平、焊回去多出三面共享边
  （`99813` 那个 `{3: 1}`），改为 `POS_EPS.max(scale × 1e-6)`。新增
  `a_revolved_seam_closes_bit_exactly_so_the_boolean_can_read_it_back` 钉 ①②，
  ③ 由现场夹具 `field_floor_negatives_can_be_read_back_by_the_boolean` 直接钉——
  该夹具三件现已全绿。
- 房间归属两条真库 live 用例的断言漂移（`live_issue7_real_db_deleted_edges_come_back` /
  `live_issue13_c2_moving_out_of_the_room_clears_membership`，2026-08-19 @8019 分别红在
  「实得 0」与「起点无归属边」）。两条都不是引擎回归，是用例自己的问题，各修一条：
  - **issue7 断言错了对象**：`drain_rooms(..).done >= 1` 是**本次调用**的吞吐计数，而共享
    实库上还跑着生产 worker，它的空闲轮 `room_round` 会先把队列行收走；更糟的是这条计数
    断言排在 `after_move == baseline` 之前，引擎即便完全做对，用例也说不出来。改为
    `wait_for_room_convergence`——轮询到 `room_recalc_element_{ELEMENT}` 消失为止，
    **不问是谁收的**，并把边集断言提到最前、收敛诊断塞进失败信息。判定刻意不看「进场时
    队列行在不在」：worker 可能在第一眼之前就收走了，拿它当判据等于把同一个竞态换个地方
    再犯一次。
  - **issue13-c2 用例不自足**：它把 `edges_of_element()` 的当前值直接当基线并要求非空，
    于是隐式依赖「issue7 先跑过」，同批次 #01 一红它就报一个不属于自己的前置阻断。新增
    共用的 `build_room_baseline`（备料两侧几何 → `rebuild_tree_from_pointers` → 只重建
    `-RM05-R512` 这一间）由两条用例各自铸基线，可任意顺序单独运行。指针重建不能省：
    `build_room_relations` 前面那道覆盖率门要树的条目数达到库内可用包围盒指针数的 90%。

  - **重建范围与对拍范围不一致**（跑之前没人发现，一连库就撞上）：两条用例都把
    `ROOM_KEY_WORD` 卡到 `-RM05-R512`（那间房只有 1 块面板）以避开全库两百多间房的
    重建，却把 `baseline` 取在靶件的**全库**归属边上。`python/testbed/.surreal/pytest-ams`
    上那个 CAP 同时挂着 `24381_35844 -> R512` 与 `24381_1391 -> R142`，于是增量最多
    只能复现一半，边集断言必红；更糟的是元素分支发的 `DELETE {element}<-room_relate`
    只避开 `protected_panels`（在册但缺几何的面板），R142 那块在这个 keyword 下压根
    不在册，它的边会被抹掉且收尾不写回——**跑一次少一条，共享沙箱里没人补得回来**。
    2026-08-06 那次能过是因为当时库里 `pe:24381_1391` 不存在，验证报告把它记成了
    脚注。现改为：断言收进 `scoped_panels()`（`24381_35842` 名下子 + 孙两层 PANE），
    范围外的边进场时按完整载荷备份、收尾最后一步原值写回。

  仓内已有同类先例（`live_shared_spco_expands_to_generation_roots` 改自足、
  `live_shared_spco_cascade_regenerates_every_consumer` 把钉死计数拆成动态口径）。

  验证：`db_options/DbOption-room-live-8029`（由 `DbOption-pytest` 拷贝，只改 v_port
  与 `room_incremental`）+ 8029 上的 `rocksdb:python/testbed/.surreal/pytest-ams`。
  两条各 20s 通过，日志复现 08-06 的黄金形态 `无房间 -> R512` 与反方向的
  `R142, R512 -> 无房间`；又按 c2 → issue7 反序各自独立进程复跑一遍仍全绿，顺序依赖
  确实断开。跑完核过：两条归属边载荷逐字回到进场值、`POS.z` 回到 5821.669921875、
  房间队列零行。唯一的持久变化是那块面板的成员边收敛到 632 条（全库 `room_relate`
  41370 → 41372），来自本轮的指针重建 + 定向全量重建，不是用例残留。
  台账两行已更新，日志在 `output/room-live-20260825/`。

- 直线扫掠体的 path 坐标系与实例旋转重复计（aios-core `0e391ff1`，本仓跟进三个 rev 到
  `0e391ff1` / `5344440b` / `257ea253`）。两个各自独立的缺陷叠出 STWALL 4
  （`pe:17496_105816`）整块墙绕 Z 转 90°：`create_profile_geos` 的 POSS→POSE 分支把
  **世界系**的 `pose - poss` 直接存进局部系的 path 字段（同一处的 `DRNS` / `DRNE` 都过了
  `inv_quat`，只有它没过；`height` 是标量所以长度一直对、只错方向）；`SweepSolid::get_trans()`
  又把 path 切向折进实例旋转，而两台建体引擎（manifold 的 `sweep_solid_mesh`、f9f1bf0
  删除前的 `gen_occ_shape`）对直线扫掠一律沿局部 +Z 挤、只取 `line.length()`——方向属于
  元素的 `world_trans`，再带一次就是重复计。只有走复用单位体的件会炸：另外三堵 STWALL 带
  `drns` / `drne`，`is_sloped()` 为真、`get_trans()` 返回单位阵，脏方向没人读。
  修后 STWALL 4 的世界 AABB 与 E3D 逐位相同、四堵墙双向表面距离全 0.00mm，
  带非单位实例旋转的行 63 → 61（消失的正是 STWALL 那 2 行）。
  八条 mesh 级对拍 6/2 → **7 passed / 1 failed**，仅剩
  `mesh_gwall_extra_against_cwall_union` 卡 105828（另一条线）。
  证据 `docs/evidence/2026-08-25-sweep-path-frame-fix.md`，台账已更新。

- 负体做差前的「让量」从 `1e-6` 等比改成**逐轴各向外让绝对 0.051mm**
  （新常量 `libgm_discretise::RES_TOL_MM`，就是 libgm 的 `GM_User::restol_`），
  收掉 ISSUE-022 那层外皮。`manifold_csg.rs` 里本来就有这一步，坏在量级与形状：
  等比量在薄方向上等于没让——那堵墙的负体沿墙厚只有 750mm，`1e-6` 给出 0.000375mm，
  比实测那道缝还小一个量级；而三个轴 2600 × 750 × 2180 差着一个数量级，等比放大
  要么长轴让太多、要么薄轴让不够。退化到零厚的轴不缩放。
  回归钉 `fast_model::manifold_csg::tests::a_negative_stopping_a_hair_short_still_opens_the_exit_face`
  ——出口停在差 0.01mm 处，挖穿了亏格 1、留皮亏格 0；退回旧等效量（该负体上 0.000055mm）
  实测即红（`genus=0`、`volume=3360061.89`，多出的约 62mm³ 正是那层皮）。
  现场（8009，删掉 8 堵带负体 GWALL 的 booled `.mesh` 就地重算布尔）：105828 gen→gwall
  p95 **753.9 → 0.1**、max 1296.9 → 65.2、三角数 188 → 184；105880 p95 9.4 → 8.9；
  116569 p95 147.4 → 137.3；GWALL union both mean/p95/hausdorff
  **10.53 / 8.44 / 1286.31 → 4.75 / 5.33 / 647.09**。八条 mesh 级对拍
  **7/1 → 8 passed / 0 failed**，台账已更新。

- 定位：八条对拍里剩下的那条红 `mesh_gwall_extra_against_cwall_union` 卡的
  `pe:17496_105828`，**不是摆位也不是尺寸，是一张本该挖掉的外表面还在**。逐三角形对照
  E3D GWALL 18：洞的两侧门垛（`[±1300, −17018, 1433]`）与过梁底面（z=2160）gen 都切出来了，
  唯独 y ≈ −16651 那张外表面，E3D 在 x∈[−1300,1300]、z≤2160 一个三角形都没有，gen 铺满。
  负体 `pe:17496_105841`（NXTR，HEIG=750）的出口面与墙体外表面共面（都在 y ≈ −16651.40，
  f32 在这个量级的 ulp 约 0.001mm），布尔把它留成一层外皮。两个数因此都能对上：
  max 1296.9 ≈ 洞半宽 1300、p95 753.9 ≈ 墙厚 748。`manifold_bool.rs` 里没有任何沿挤出轴
  给负体加余量的处理。开 `issues/ISSUE-022-coplanar-negative-leaves-outer-skin.md`，
  属 ADR-044「共面留一层壁」同族。

- IDA 取证（同日续查 ISSUE-022 的 ε 口径）：**libgm 的「多近算同一个」是 0.051mm，
  而 E3D 的负体根本不是三维实体 CSG**，证据
  `docs/evidence/2026-08-25-ida-libgm-coincidence-tolerances.md`。Core3D 建体前
  （`0x104da260`，MTR 标签 `adp_geometry/adp_gm_mk_body`；另一处 `0x108e6a80` 同值）
  连调四次：`gm_SetResolutionTolerance(0.051)` / `gm_SetDefaultNormalisationTolerance(0.051)`
  / `gm_SetDefaultTangentTolerance(0.0087266)`（0.5°）/ `gm_SetDefaultFacetTolerance(0.5)`
  ——最后那个就是本仓 `FACET_TOL_MM`，说明这处调用点正是我们该抄的那处。负体侧：
  `addStandAloneNegative` 建 `gm_CreateCombination(3)`，`GM_AggregateCombination::calcFacets`
  把 `restol_` 传给 `GM_CompFacets::aggregateWith`，真正相减在 `GM_Facets::obscureFaces`
  （libgm `0x10068710`）**面内做二维多边形相减**，切分 side 判定与 `D2_PolySet::normalise`
  都吃 `restol`。所以 ISSUE-022 不是「有条共面规则没抄」，是**「libgm 有 0.051mm 的重合
  容差，我们一个都没有」**——`plant_mesh_to_manifold` 焊顶点用的是 `to_bits()` 逐位相等。

- 同上取证顺带纠正一条仓内注释：`libgm_discretise.rs` 的 `NORM_TOL = 1e-6` 写着
  「没有人改它……运行期恒为初值」。成员写入器 `GM_User::normtol(double)` 确实零调用，
  但 Core3D 走的是自由函数 `gm_SetDefaultNormalisationTolerance`（同一处调用点），
  运行期真值是 **0.051**，差 51000 倍。`normtol_` 的读者是 `gm_CreateBody` /
  `gm_CreateNormalisedItem` / `gm_CreateFacetStructure` / `gm_QueryMass`，还管回转轮廓的
  轴心吸附。改它会动到所有回转体，**本次只记录、未动代码**。

- 补上两条原本谁也拦不住这类漂移的钉子：aios-core 的
  `world_path_direction_lives_in_instance_rotation` 直接断言的是旧行为，改写为
  `path_direction_stays_out_of_instance_rotation`（+X 的 path 必须得到 `Quat::IDENTITY`）；
  gen-model 新增 `fast_model::sweep_mesh::tests::a_reused_unit_lands_where_the_direct_build_lands`
  —— +Z / +X / 斜向三档 path 下，`sweep_solid_mesh(单位体) × get_trans()` 必须逐顶点等于
  `sweep_solid_mesh(原件)`。原有 `get_trans()` 用例的 path 全是 `Vec3::Z * k`，恰好是所有
  分歧点都退化的那一档；这条等价性还顺带盖住同族两处潜伏偏差（非 +Y `na_axis` 时的 plax、
  无条件乘 bang）。

### 新增

- T041 的 A / B 两组门**写成了真单测**（`src/fast_model/pdms_inst.rs`，`t041_` 前缀
  11 条）：判据先落地、实现随后，**6 条按设计红着**，逐条登记在
  `specs/009-retire-occ/tasks.md` 新增的「预期红测」一节（每条写明现在为什么红、
  转绿的条件）；另 5 条是不许变红的反向门。CI 口径全量
  **1119 passed / 6 failed / 85 ignored**，失败逐名就是那 6 条，无旁落。
  写测试时看清两件事：(1) 配对必须是「**同一形状比例、不同绝对尺寸**」——单位行的
  半径恒为 1，拿两个不同比例的件对比问不出「段数有没有进键」；(2) `t041_b1`（碟的
  三元组）**今天绿得不作数**——两键不同只是因为 `Dish::hash_unit_mesh_params` 哈希的
  是未归一化的 `prad`，等 `b1b` 把它收成比值这条才开始真的量三元组。`b1b` 顺带把
  T053 第 (3) 条那个「读码所见、未构造用例」的双键疑点变成了有用例的红测。
- T041 的门按 T053 的新范围写全（`specs/009-retire-occ/tasks.md`）：从「柱与球」扩到
  **五类**，并把此前没有的**元数**一维补上——每类混几个段数是不同的
  （柱 / 球 / Snout 一元，圆环面二元，矩形环面一元，球碟二元，椭圆碟三元），
  只混第一个数的写法 A 组门盖不住。B 组四条各带一对**实算出来的判别样本**：
  椭圆碟 `a=1000` 的 `h=5` 与 `h=20` 绕轴同为 100 而 `(hub,knuckle)` 是 (2,2) 与 (3,3)；
  圆环面 `rins/rout=0.5` 下 `rout=104` 与 `105` 环向同为 36 而管向是 16 与 20；
  球碟 `(100,2)` 与椭圆碟 `(100,2,2)` 前两位逐位相同（分支今天靠 `prad` 分，
  加段数时不许把变长元组摊平成不带长度/不带分支的写法）。
  另钉三条容易反向做错的：矩形环面**只有一元**（矩形截面无管向）、球**只混 n**
  （stacks 恒 `n/2`，IDA 已钉）、SSCL 与偏心 Snout 两支本来就带真实尺寸，
  **键必须逐位不变**、不得重复混。整体门 C4 用 T053 的脚本口径复核五类行数落在 474——
  偏多是混了不该混的，偏少是漏了一维。
- T053 范围盘点：**段数进身份键之后，五类复用曲面原语合计 392 行 → 474 行（+82）**，
  证据 `docs/evidence/2026-08-25-t053-segment-identity-scope.md`（库 A 一次性副本 @8039，
  `FACET_TOL_MM = 0.5`，规则逐行照抄 `libgm_discretise` 并先跑其单测对照表自检）。
  逐类：单位柱 1→44、`PrimCTorus` 95→102、`PrimRTorus` 167→174、`PrimLSnout` 112→133、
  `PrimDish` 17→21。量级与旧估同档，ADR-044 决策 2 与 D1「同批重建」都不必回开；
  plan 的 G3 / V5 与 tasks 的 T053 已按新口径改写。

- 盘点口径澄清：T045 的「柱 1 → 37」算在 `inst_relate.insts_flat[]` 上，那是带
  `geo_type = 'Pos'` 的**读侧投影**；而 `.mesh` 按 `geo_hash` 存、负体同样吃
  （`apply_cata_neg_boolean_manifold` 取操作数走 `geo_type == "Neg" or
  "CataCrossNeg"` 再 `record::id(out)`，与正体同一张表）。按 `geo_type` 拆开：
  Pos/Compound 392→460（柱 1→37，与 T045 逐位吻合）、负体族 392→432、
  全部吃 `.mesh` 的 392→474（柱 1→44）。**排期按最后一档**——差异集中在
  8,000–41,800 mm 的负体柱，负体分错段数正是 ADR-044 要治的「共面留一层壁」。
- `insts_flat` 的 11,992 个空数组判为**非缺陷**：逐行对着回填式的四道过滤分类，
  11,979 行（99.9%）的边全是负体、13 行有 Pos 边但全部不可见，「有合格边却仍为空」
  与「`booled_id` 有效却仍为空」两个缺陷桶都是 0。真残留在另一头——
  `insts_flat = NONE` 的 1,479 行里 **40 行对读者可见**，正是清扫段 `WHERE` 圈的、
  也是 `pdms_inst` live 断言禁止残留的那一类（三行还带 `booled_id`）。
  开 `issues/ISSUE-021-insts-flat-visible-none-residue.md` 记录，**当日查完判为非缺陷**：
  `queue_control` 上根本没有 `booled_flat_repair_migration` 标记（只有 `main`
  `paused = true` 与 `watermark_seed`），把清扫段 SQL 原样重放到一次性副本上
  40 行**一批归零**——既不是「标记落早」也不是「清扫有漏」，是一份冻结基线。
  同趟量出 RM13 修复 migration 在这库上有 **6,599 行在等**，原样重放 14 批 6,595 行
  收敛到 0。库 A 是**前 migration 的基线**：RVM 对拍走 `tessellate_libgm_param`
  不读 `insts_flat`，不受影响；但拿它起服务给人看，读侧会端出 RM13 那种错误正体，
  直到启动序列跑完。ADR-043 决策 5 在回填侧的脏位缝仍是真问题，但**不是这 40 行的
  成因**，留在 specs/025。

## 2026-08-24

### 新增

- IDA 取证：**挤出轮廓在 libgm 侧没有清理层**，`docs/evidence/2026-08-24-ida-extrusion-profile-no-cleanup.md`。
  `mth::mthArcFillet`（libgeom 3.1 `0x10043470`，Core3D 走同一份）只有五条早退——两条邻边
  退化、`|R| ≤ 1e-6`、θ≈0°、θ≈180°——过了就按 `T = R/tan(θ/2)` 硬算切点，**从不把 T 与邻边
  长度比较**，不夹取也不裁剪；`GM_Extrusion::calcFacets`（libgm 3.1 `0x10056f10`）的全部
  过滤器只有「按存的标志翻绕向 / 起止点位级全等才跳过 span / `fabs(bulge) >= 0.0000306`
  分弧直 / 圆面按 `normtol_` 去重」，不查自交、不查闭合，输出是「侧壁四边形 + 两个不三角化的
  n 边形盖」——E3D 本就不把挤出当闭合实体。结论是否定式的：**没有可照搬的 libgm 规则**，
  `flatten_profile_loop` / `extrude_flat_polygons` 现有注释与 `FillRule::NonZero` 选型至此
  有反编译背书。顺带改写了现场三件参数的性质：`FRAD = 4553.95` 不是超发值，它在 172.336°
  的近直角上算出 `T = 305.023`、邻边 305.030，是**故意做成弧接弧、直段长度为零**的轮廓，
  残留 ±0.0007mm 只是坐标三位小数的舍入。
- CFLOOR `/1RS-WF03-F-C-F002` 负体 NotManifold 的离线回归：
  `field_floor_negatives_can_be_read_back_by_the_boolean`（`src/fast_model/manifold_tessellate.rs`）
  + 夹具 `tests/fixtures/floor_wf03_bool_neg_not_manifold.json`（三件参数自
  `.surreal/ams-rvm-rebuild-20260824` 的 `inst_geo` 原样取下）。它把失败切成「三角化」与
  「布尔读回」两段分别报告：三件**三角化全部成功**、全部死在
  `manifold_csg::plant_mesh_to_manifold`，而对照组（单位箱 / 单位柱 / 方形挤出）同一往返全绿
  ——红的是几何本身而非往返。不连库、0.00s，进得了 CI。

- spec 025 T06（FR-9）：RM13 布尔平表存量修复从「每轮清扫必跑的常驻全表段」改制为
  **带库上标记的一次性 migration**（`run_booled_flat_repair_migration_on`，标记
  `queue_control:booled_flat_repair_migration`，流程「标记不存在 → 修复到收敛 →
  复核无残留 → 落标记」）。标记已落的库每轮只付一次 record id 点查；复核有残留不落
  标记、下轮从头重跑；旧备份恢复/库拷贝带回无标记状态即自动重跑。源码顺序钉 +
  mem 行为钉（标记落一次、再跑跳过、标记消失重跑）+ 双跑补标记语句形态；
  spec 019 状态注记同步。
- spec 025（`insts_flat` 失效协议）阶段 0 落地：共享 geo `bad→meshed` 反例 live 用例
  `live_shared_geo_bad_retry_must_refresh_sibling_insts_flat_on_disposable_db`
  （`src/fast_model/pdms_inst.rs`），在 8019 一次性内存沙箱按设计红——只对一行做定向
  重生成后，共用同一 `inst_geo` 的另一行 `insts_flat` 停在旧值（非 NONE），现有清扫
  两段都够不着（ADR-043 的缺口）。T02 判读「能复现」：FR-6 定持久 pending 表
  （选项 P）、FR-7 定反向失效（路线 B）；结论入 `specs/025-insts-flat-invalidation/plan.md`
  R1，留证 `docs/evidence/2026-08-23-insts-flat-invalidation/t01-shared-geo-counterexample.md`，
  live 台账同步。双跑套件的清扫语句同步到 `VALID_BOOLED` 共享判据形态并双引擎复验
  （T07）。

### 修复

- 收口依赖追齐后暴露的两条纯函数红：T054 的 SSCL 三角化删除本地剪切角折叠副本，
  直接消费 aios-core `f9f1bf0f` 的规范折叠值，并让「折叠后仍出界」诊断先于 bool
  `check_valid()`，不再把 90° / 271° / NaN 误报成尺寸退化；specs/023 的元件库
  顺序分批与并行优化 fan-out 同步改由全局几何并发闸推导块宽，移除遗留的固定 4 路
  和 1/2/4 分块，并扩展源码守护覆盖按输入规模选择固定 fan-out 块宽的写法。T054 三条定点门、
  并发闸 6 条纯函数门及同 feature 全量 **1104 passed / 0 failed / 86 ignored**。

- RVM mesh 对拍不再可能量到 OCC 的答案（specs/009 X1a 前半）。`mesh_compare` 的 gen 侧
  在 `tessellate_libgm_param` 返回 `Ok(None)`（非形状）或报错时会回退 `gen_occ_shape`
  ——正是 T037 从 `occ_generate.rs` 拆掉的那条形状回退，活在打分的那一侧。它也不是
  休眠的：`occ` 在 default 里，`rvm_verify` 是叠加 feature，跑法与 `Run-LiveBatch.ps1`
  都不带 `--no-default-features`，所以台账里 2026-08-14 那 8 条 mesh 对拍**全部**是带
  `occ` 量出来的，已在台账整体标注为口径失效、按未验资产对待。分支与只服务它的
  `gen_tol` 参数一并摘除，`gen_side_has_no_second_shape_engine` 钉住不许长回来。
  重新取证的前置是一个带 dbnum 1112 + 8000 生成几何的库——`.surreal/ams-8009`
  已被 3.x 写坏（fork 2.1.4 报 `format_version: 7`），2026-08-24 实测确认。

- `occ` 退出 gen-model 的 default / console（specs/009 X1a 收尾）。cfg 挂点归零之后
  这个 feature 已经和当初的 `truck = []` 一样是空壳，区别只在于它还拖着
  `dep:opencascade` 和 `aios_core/occ` 白编一遍——feature、依赖、以及随之变成
  「未被用到」的 occt-rs `[patch]` 段一并删除，`Run-L3Suite.ps1` /
  `Run-E3DFixtureSuite.ps1` / `Record-Db8000SessionChain.ps1` 三个脚本的 features 串
  同步去掉 occ。ADR-030 决策 7 的「回滚 = 把 occ 加回来」自 T037 起就已经不成立
  （加回来形状也只走 manifold），真回滚是 git revert。
  **Cargo.lock 不在本次提交内**：工作区正卡在 V4 中途，根 `Cargo.toml` 已把
  aios_core 升到 `f9f1bf0f`，而 `python/Cargo.toml`、parse_pdms_db、pdms_io 仍钉
  `29c91f48`，依赖图里因此有两份 aios_core（`RefnoEnum: From<RefU64>` 一类的红全出自
  这里，与本次改动无关）。锁文件要由收尾 V4 的那一步一起重生成。

- 摘掉 gen-model 里剩下的 occ 挂点（specs/009 X1a）。`occ_generate.rs` 的
  `apply_insts_boolean_occ` / `apply_cata_neg_boolean_occ` 两个死布尔函数、它们的
  三条 import 与全部注释调用点删除（ADR-029 布尔单轨 manifold 已一年量级）；
  两条只在 `occ` 下编译的回归用例改挂 `manifold`：房间面板自交修复走
  `tessellate_libgm_param`（轮廓修复在 wire 层，与后端无关）、GENSEC 直脊 SPRO 走
  `PdmsGeoParam::PrimLoft`。CI 不带 `occ`，所以这两条此前**从未在 CI 跑过**，
  现在两条都进 CI 且通过。`feature = "occ"` 在 gen-model 的 cfg 挂点自此归零。

- 收紧当期项目扫描范围：`included_projects` 统一解释为 `project_path` 下的文件夹名，
  名单外项目不再解析或扫描；`project_dirs` 不再重定向、扩大范围，也不再在空名单时
  充当回退名单（ADR-046 / specs/027）。

- 曲面法向的分组改由**轮廓**说了算，不再拿夹角猜（ADR-047 / specs/028 Phase 1，
  接替 specs/009 挂了一年的 T040b）。此前本仓两套做法都不是 E3D 的：`manifold_csg`
  按「同位置夹角 ≤ 10° 才平均」（`d0088e93` 引入，10° 是猜的），`sweep_mesh` 的侧壁
  干脆逐四边形写面法向、等价于**全是硬边**，弧墙 / 斜切墙 / 环形截面一律渲染成折面。
  E3D 的判据在 `GM_Profile::getPolygonForFacet` 的第二出参上：相邻两段不切线连续就把
  该顶点取负，判据 `D2_Span::leadsSmoothlyTo` 是 `|1 − 点积| ≤ 1e-6`（≈0.081°）。
  **这跟夹角阈值不是精度差异，是判据类型不同**：粗弧（面片夹角 45°）在 E3D 是光顺
  曲面、10° 会判成八棱柱；浅折角（5.7°）在 E3D 是硬边、10° 会抹平——一个阈值同时
  回答「面片多粗」和「形状多折」，两头都会错，而且错法随离散密度漂移。
  现在硬边在截面离散的同一趟里逐点算出，侧壁法向软点跨面片平均、硬点不平均、
  扫掠方向永远平均。**拓扑一位不动**：顶点数、索引、绕向、体积、包围盒全不变，
  变的只有 `normals`——所以看到弧墙从折面变光顺时，几何并没有动。
  弧形墙（`CurveType::Spline`）同批从 `Manifold::extrude` 改走 `sweep_mesh` 的挤出
  （specs/028 T06）：它的环是解析出来的四段、不可能自交，用不上 `CrossSection` 的
  NonZero 填充；一般挤出（PLOO 带倒角）**要**那道填充来化解自交轮廓，所以留在
  manifold 上等 Phase 2。半圆环的折痕自此正好是四个轮廓角 × 两个 z 层，八个，
  不多不少。
  布尔之后那一段仍走 10°（manifold 交回来的三角汤没有轮廓出处），这是 ADR-047
  写明接受的过渡态，退役条件在 specs/028 的 Phase 2 / 3。
  同批把**交线边**那一半也反完了（ADR-047 决策 6，证据
  `docs/evidence/2026-08-24-ida-edge-types-and-smoothing-groups.md`）：硬边在 libgm 里
  是边上的一个枚举值不是几何量，`normaliseStage2` 把布尔新建的边默认判硬、再用
  `isTangentDiscontinuity` 把**相切的缝主动合回软的**。判据是法向弦长 0.8182 ＝夹角
  **48.297°**——常量虽是 cos/sin 22.5°，但**有效阈值是 48.3° 不是 22.5°**，差一倍，
  按 22.5° 抄会把一大批该软的缝判硬。这条同时修正了 plan 里原先那句「交线是硬边会
  自然落出来、不需要额外规则」：自然落出来的只是硬的那一半。
- 扫掠体的截面离散补上另外两套口径（specs/009 T056）。libgm 里挤出 / 回转 / 放样
  是三个类三条路，而 `sweep_mesh` 三支共用挤出那一套折线化——T040 当时只修了
  `manifold_tessellate::tessellate_revolution`，没往扫掠这条路上看，于是弧墙与斜切墙
  一直在用 `GM_Extrusion` 的段数。现在建环与离散拆成两步（新增 `RawLoop` 与
  `ProfileCaliber`，`Polyline` 中间层去掉），三支只在「每段分几步」这一处分叉：
  挤出逐 span 自算、回转按配对 span 取大、放样再由 `GM_Collar::setSpanSteps` 在
  **外环与全部孔环之间**按 span 下标取大。
  **回转与放样必须是两个入口**：跨环取大是 `GM_Collar` 独有的，`GM_Revolution` 的
  `polygonForFacet` 按 `GM_Profile` 对象逐个调，没有那一层——合并入口等于给弧墙
  加一条 E3D 没有的规则。跨环步数表对不齐时硬失败，不猜。
  现场可见的变化是整环截面（SANN 360°）内孔的点数被抬到与外环一致；能量它的
  RVM 门（T047 / T048）仍未跑。
- 偏心 Snout 的偏移改成**上下各摊一半**（specs/009 T050）。libgm
  `GM_Snout::calcFacetsWithoutSurfaces` 逐顶点写的是 `r·cosθ ∓ xShift/2`，`calcRange`
  的支撑函数独立佐证同一约定，`GM_Pyramid` 也完全同构；本仓 `gen_snout` 与 aios-core 的
  `gen_occ_shape` 却都把偏移整个加在顶圈，相对 E3D 整体平移 `(XOFF/2, YOFF/2)`。
  **两条后端互相一致，所以此前任何双后端对比都发现不了**——只有对着 libgm 的绝对位置
  才照得出来。vendor 侧同批抽出 `LSnout::end_centers()`，OCC 与 truck 两条路共用它，
  约定不留第二个版本，并因此变得不依赖 `occ` / `truck` feature 就能测。
  活库实测有 1 件 2 实例（`poff = 12.06`，错位 6.03mm ＝ 容差的十二倍），不是纯防御。
- 「椭圆碟」（DISH 且 `RADI > 0`）建的曲面族换对了（specs/009 T038a）。libgm 的
  `GM_EDish` 是**托里球形封头**——一段球冠加一圈与它相切的环面拐角，本仓画的是半个
  旋转椭球。补经向段数补不上这个差，`gen_elliptical_dish` 整个重写，形状三量
  （拐角半径 / 球冠半径 / 交接角）与三个方向的段数改由
  `libgm_discretise::elliptical_dish_facets` 一次解出。
  两处照抄陷阱写进了函数文档：`RADI` **只是开关**，Core3D 读了它却把数值丢掉、
  实参是现算的 `r_k = h / (1 + (a − h)/√(a²+h²))`；交接角是 `acos(1 − q)` 而不是
  `acos(q)`（后者是 Hex-Rays 吞掉 acos 实参之后的伪码假象，抄错会让碟身留一道折痕）。
  绕轴喂的是底半径而不是球冠半径。活库实测 15 / 17 行是椭圆碟、102 个实例。
- 三条 `gm_Create*` 的参数顺序补上外部权威（specs/009 T011 / T013 / T016）。两头对照
  libgm 构造函数的字段序与 Core3D `CSG_Basic???::getPrimGeom` 的调用点，**顺序全部
  与实现一致**；`gm_CreateSlopeEndedCylinder` 的 wrapper 内部会把两对剪切角换位，
  对外签名是底面在前，本仓写法正确。顺带记下两条现成欠项：CTOR/RTOR 的内半径
  Core3D 会夹 `fmax(RINS, 0)`、SSCL 的四个剪切角 Core3D 会折进 (−90, 90]，本仓两条都没做。
- 上面那两条欠项当日销掉（specs/009 T054 / T055）。SSCL 剪切角折叠落在 vendor
  `SCylinder` 一处（`fold_shear_angle_deg`，Core3D `0x107272D0` 的单次折叠，非取模），
  身份哈希、落库规范值、occ/truck 后端与 gen-model 三角化臂消费同一份，135° 与 −45°
  自此同键同网格，折完仍出界响亮失败；180° 的剪切角折回直柱身份，不再白拆复用。
  CTOR/RTOR 的 `From` 构造点补上 `fmax(RINS, 0)`；顺着 validate 反编译
  （`GM_CircTorus` `0x10030bb0` / `GM_RectTorus` `0x10030780`，判据 `rIns ≥ −1e-6`）
  发现 `RTorus::check_valid` 的 `rins > 0.0` 比 libgm 还严、错拒合法的 RINS=0 构件，
  已放宽为 `>= 0.0`。`rins = 0` 的喇叭环面 / 实心扇柱由新增的贴轴收拢
  （`mesh_primitives::collapse_onto_axis`，仅 `rins < rout·1e-5` 档启用，吸附哲学同
  T035）收成可布尔的闭合实体，体积对帕普斯过门。vendor 改动仍未发布，
  与 snout 一批推上游（收官计划 Phase V）。

### 新增

- 斜切延伸段的长度规则落成纯函数（specs/009 T021）：`sweep_mesh::mitre_extension_reach`
  与 `mitre_extension_length`，出处是 Core3D 的扫掠段构建器 `sub_107318E0` 及它调的
  `sub_10733720`。规则是「轮廓每个顶点 + 每条弧上 9 个内点，逐点算在切面法向上的
  伸出量，取最大绝对值；超过 1 再加 1，最后与端点间距相加」。那 9 个点是本仓遇到的
  **第四套离散口径**，与容差无关、只服务这个包围盒，已收成具名常量并写明「别拿去铺
  三角」。还没接生产（挂到段 CSG 是 T023，依赖斜切墙 RVM 门）。
- 两份活库盘点证据。`docs/evidence/2026-08-24-eccentric-snout-census.md`：偏心 Snout
  两库一个 0 一个 1，**翻掉了上一轮「两库定性一致」的隐含预期**——再有「预期为 0」的
  专项，一个库查出 0 不能当证明。
  `docs/evidence/2026-08-24-unit-normalised-curved-primitives.md`：碟 / 两种环面 /
  不偏心 Snout 在 `inst_geo` 里**全是单位几何**（`pdia` / `rout` / `pbdm` 恒为 1.0），
  于是 WP-G 那套「段数由真实半径算出」的权威规则拿到的是单位半径，`tol/R = 1` 直接撞
  45° 下限——**不论多大都是 8 段**。碟的实例尺度跨 13mm 到 48.9m，最大那件应当 492 段，
  弦高差出容差的 3700 倍。  规则对了但现场还没生效，挡在前面的仍是单位网格身份键：
  **G3 / T041 的范围不是「柱与球」而是所有参与复用的曲面原语**，已开 T053。
- 第三份活库盘点证据 `docs/evidence/2026-08-24-negative-rins-census.md`（T055 收口）：
  负 RINS 两库均为 **0**，夹取在两库范围内是纯防御；但库 A 有 **1 行 `rins = 0` 的
  PrimCTorus（12 实例，`scale = 33` 的 90° 实心弯）**——旧生成器对它产出的是退化
  网格，T055 的贴轴收拢治的正是它（现役修复，不是防御），同时它是 T049 圆环面
  抽检的现成样本。库 B 用的是 init 快照（3,631 行，比 T052 在跑实例少 6 行增量），
  结论只覆盖到快照点。
- ADR-044 补 2026-08-24 修订：`GM_Sphere` 的权威面片规则进正文——绕轴
  `n = circle(r, tol)` 硬截 1000，经向带数**恒 = n/2**（`GM_Facets` 构造实参反推，
  与球碟同构），对决策 2 收紧为**球的身份键只需混一个 n**；顺带把「现状 16×36 的
  36 从来没对过」钉死在口径对照表里。出处 `0x100A20F0` 反编译，证据同上一条 IDA
  复核文档。
- 收官计划四个决策点 D1–D4 拍板并落进计划正文：整库重建不设独立窗口（V1–V4 同批、
  混合期目标为零）；球/SSCL/多面体缺样本走 **E3D 造样本**而非找第三个库；RVM 基准
  单跑库 A（7997 副本），偏心 Snout 与球两条例外走库 B 运行态或造样本；双会话分工
  以 specs/009 文件面为界，vendor `wire.rs` 归净窗口线，Phase V 推上游前对表。
- IDA 复核证据 `docs/evidence/2026-08-24-ida-occ-retire-audit.md`：T050 的 ±shift/2、
  T013 的 `fmax(RINS, 0)`、T016 的单次角度折叠三条依据在 idb 里逐位复核属实；
  T021 的 `t = k/10` 从推断升级为实证（乘数常量读出 = 0.1，调试日志就叫
  `"/10 point of span"`）；新钉 `GM_Sphere` 面片规则——绕轴 `n = circle(r, tol)`
  硬截 1000、经向恒 `n/2`，球的身份键只需混一个 n，且现状 16×36 里的 36 在
  「幸运尺寸」下也从来没对过。SSCL 折叠与 RINS 夹取两条欠项复核仍欠，已开
  T054 / T055 排进 WP-K。
- IDA 补齐最后一个没读过的形状臂：证据
  `docs/evidence/2026-08-24-ida-gm-collar-ruled-solid.md`——`GM_Collar`
  （= `gm_CreateRuledSolid`，斜切墙那一支）在 libgm 3.1 逐位读出，此前手上只有
  teach 记录里一行 2.10 转述。四条与本仓有落差：**两端点数一一对应是 libgm 的前置
  条件**（`validate` 比两端跨度数，不等报 −61 拒建），不是算法凑出来的；`setSpanSteps`
  的配对表与步数表是**两端外环加全部孔环共一份**（初值 8、逐 span 取
  `max(自身半径, 配对半径)`、容差逐轮廓、写回只增不减），而**「只增不减」正是 collar
  强制两端同段数的机制本身**——`polygonForFacet` 冷路径会主动清掉「已设定」标志重算，
  统一值全靠取大活下来；摆位是 **z = 0 → z = height** 而不是 box / snout 那套 ±h/2，
  两个盖各是一个 n 边形面；侧壁是双指针归并，步长与硬边共用 `polygonForFacet` 第二
  出参——**该出参一参两用，T040b 只做法向那一半补不齐**。
  同一份证据里把 **T040b 的规则也反完了**（实现仍不在本期）：`getPolygonForFacet`
  的第二出参逐顶点记「有几条 span 在这里收尾」，相邻段不平滑时取负，**闭合处一次负
  两个**；判据 `D2_Span::leadsSmoothlyTo` 是 `1 − 点积 ≤ 1e-6`（≈0.081°），
  **与布尔那边的 22.5° 和归一化那边的 48.3° 是三个不同判据，不得互顶**；
  切线里 `0 < |bulge| < 3.06e-5` 走一条非单位的退化分支，效果是「极小非零 bulge 必判
  硬边」，照抄时别顺手归一化。
  顺带撞出 T056（当日已修，见「修复」一节）。specs/009 的 T020 / T022 / T047 / T048
  已补上依赖与验收门。
- `CONTEXT.md` 补上 ADR-041~045 各自引入的那个概念，五条一次到位，每条按主题进它该在的
  分节而不是堆在文末：**libgm 面片口径**（紧跟「单位网格身份」，因为 ADR-044 改的就是那把
  身份键）、**切片流水线** 与 **insts_flat 失效协议**（模型生成执行）、**分块解析基线**
  （暂存与写回）、**refno 反查**（紧贴「空间树」）。
  `_Avoid_` 那行写的是各决策真正要挡的叫法，不是凑同义词：切片流水线禁「ZONE 并行 / 按片
  并行」——ADR-041 第一节整段就在拒绝这个说法，并行单位是生成根，切片只决定一次拉多少数据
  进来；分块解析基线禁「暂存窗口」——它只取「实例 + 读路由」那一半，基线不开窗口；libgm
  面片口径禁「默认段数 / 细分级别」——段数不是画质旋钮，共面抵消只消全等重叠，差一段布尔
  就留一层内壁；refno 反查禁「自建 refno 索引」，理由留在正文：第二份「树上现在有什么」的
  真值漂了不是变慢，是把已经不在那儿的构件算进某间房，而这类错误没有任何东西会报出来。

### 移除

- 删除 `truck` 死接线（收官计划 Phase X 的 X0a / X0b）。gen-model 侧 `truck = []`
  是零挂点的空壳 feature，`gen_brep_shell` 在本仓零调用；vendor（aios-core）侧
  `truck` 的依赖在 Cargo.toml 里整组被注释，**开了 feature 也编不过**，17 个文件
  90 处 `#[cfg(feature = "truck")]` 门是纯死文本，还一直在吃同批维护（T050 / T054
  都给它做过等价改写）。两侧一并删除：feature、注释依赖、全部门与所属条目；
  `LSnout::end_centers()` 无 feature 依赖，保留。vendor 半区与 snout / T054 / T055
  同批推上游。
- 删除浸水插件 `src/plug_in/water_calculation.rs`（业务上不再需要），连同
  `plug_in` 的模块声明与 `consts::AQL_WATER_CALCULATION_COLLECTION`。它是 ADR-030
  背景一节点名的「第二个 OCC 抑留点」，这条说法自此作废——抑留点只剩扫掠体
  （PrimLoft）一处。删除时它对 OCC 的依赖其实早已不存在：唯一那处 BRep STP 导出挂在
  从未定义过的 feature `opencascade_rs` 下，历来所有构建编的都是写死字符串的占位，
  死分支已于 2026-08-23 删掉。剩下的 `save_stp_data_to_arangodb` 与四个 ArangoDB
  查询函数在仓内**零调用点**。`aios_core::water_calculation` 自此在本仓无引用。
  见 ADR-030 决策 11。

## 2026-08-23

### 修复

- 模型生成收口三个「有意回退 OCC」的口子（specs/009-retire-occ WP-F，依据 ADR-030
  修订二的 IDA 实证）：出平面回转轴改硬失败并带出实际轴向（libgm 的轴参数是
  `D2_Point`，E3D 在 API 层就表达不出出平面轴）；`CurveType::Spline` 按其 OCC 权威
  实现的本义落地为**弧形墙截面**（三点圆 + thick 内外偏移的环形扇区，弧折线走
  libgm 角度格子），点数≠3 / 三点共线 / 出平面 / thick 吃穿半径一律硬失败；
  `Unknown`/`CompoundShape` 直接标 `bad`。`gen_inst_meshes` 自此只有 manifold
  一台形状引擎，OCC 形状回退整段拆除（`occ` feature 仍在，只服务对拍参照与
  历史断言）。活库盘点三类口子出现次数均为 0，收口不动任何现存实例。
- 回转轮廓补上 libgm `movePointsOntoYAxis` 的轴心吸附：半径坐标在
  `GM_User::normtol_ = 1e-6`（实测初值，运行期无人改写）内的顶点精确置 0，
  贴轴轮廓带浮点噪声时不再在轴心留一圈纳米级针状面。
- 弦高容差收成全库唯一一份 `libgm_discretise::FACET_TOL_MM = 0.5mm`（绝对量）。
  扫掠体（墙）此前用的是 `SweepSolid::tol()` = 0.01 × 轮廓外接球半径的**比例**容差，
  于是 `tol/R` 恒定、段数与构件尺寸无关——同一个半径的弧在墙上和在与它相交的原语上
  分成不同段数，而 `cancelFacets` 只消全等重叠，共面处就留一层壁。**这会改变墙的
  弧段段数**，而 RVM 门要等 `mesh_compare` 从 `occ` 解绑（T043）才跑得起来。
  `the_facet_tolerance_has_a_single_source` 按源码扫住第二个容差来源的回流。
- 曲面生成器不再自带默认段数：`mesh_primitives` 删掉 `DEFAULT_CIRCULAR_SEGMENTS`，
  `unit_sphere` 的经纬段数改由调用方给。柱与球仍锁死段数是**单位网格身份键**的欠账
  （ADR-044 / T041），现在收成 `manifold_tessellate::unit_mesh_identity` 一处具名常量，
  不再是散在三个 match 臂里、看着跟旁边算出来的段数一模一样的裸字面量。取值一位未动。
- 弦高容差删掉三处兜底默认值。折线化那三条路（挤出轮廓、回转轮廓、弧墙截面）各写着
  `if chord_tol > 0.0 { chord_tol } else { 1.0 }`，常量只定义一处并不等于「唯一一份」——
  第二个值藏在分支里，而且只在非正容差时现身。今天不可达（生产喂的都是
  `FACET_TOL_MM`），可一旦容差接成配置项或按构件算，它就会把 0.5mm 静默换成 1.0mm：
  段数减半、`cancelFacets` 的共面抵消随之失效，现场只看得到布尔结果多一层内壁，
  没有一行日志指向容差。改为 `libgm_discretise::chord_tol_is_usable` 判定后硬失败。
- 椭圆碟的经向段数从调用处的 `(around / 2).max(4)` 收进具名
  `libgm_discretise::elliptical_dish_meridional_segments`。它是 T038a 的欠账（libgm 把母线
  拆成球冠段与过渡角段两次分算，`knuckleRadiusToUse` / `radiusOfHub` 还没反完），
  混在 `d.pdia, d.pheig` 中间时跟旁边真按规则算的段数长得一模一样，改动它一位不会有
  测试变红。段数自此只有三个出处：权威规则、点名的身份键欠账、点名的口径欠账。
  （该函数 2026-08-24 随 T038a 落地删除——欠账还上了，具名占位一并撤走。）
- 结构楼板极端倒角那条护栏改量真实路径（specs/009 T044）：`loop_model` 的整体断言
  原挂在 `occ` 的 BRep 后端上，CI 口径不编 `occ`，它是一条从来不跑的断言；
  现改走生产同款 `tessellate_libgm_param` + `compute_aabb`，顶点与包围盒的有限性
  在 CI 里真被验到。
- 删除浸水 STP 导出的死分支（specs/009 T044，ADR-030 决策 6）：真 BRep 导出挂在
  从未定义过的 `opencascade_rs` feature 下，发布二进制历来只编「写一句固定字符串」
  的占位；其唯一调用点 `test_water_calculation_stp.rs`（1382 行）的 `mod` 声明本身
  是注释，连测试都到不了。死分支与死测试文件一并删除，占位实现的文档写明重启路径。

### 新增

- 新增源码断言 `the_curve_primitives_are_not_shape_arms`（specs/009 T017）：libgm 的
  曲线/标记图元（`gm_CreateNull` / Mark / Straight / Arc / Bezier 走
  `calcFacetsWithoutSurfaces` 出折线，不产实体）不得成为 `tessellate_libgm_param`
  的成功分支——五个名字不许出现在生产半区，分发臂集合钉死为 14 形状 + 2 非形状，
  新变体必须先过清单再进 match。
- `geom_error` 扩展 `primitive` 基本体错误：缺失/非法 BREP 与 NaN 变换按参考号持久落库，
  保存 noun、尺寸诊断、累计次数和首末时间；成功生成后精确销账。
- health 新增 `model_update_pending` 单查询快照与 `blocking_conditions`：模型/房间死信会把
  顶层状态降为 `degraded`，普通可重试积压不降级；查询失败仍沿用 2 秒预算并只进入
  `degraded_sections`。
- 新增零尺寸 NCYL 现场修复编排、严格守卫 E3D 宏和成对基线回滚脚本；默认 dry-run，
  只在参考号、owner、noun、空名称、尺寸与位姿全部匹配时删除 `24381/38635`。

### 修复

- `CYLI/SLCY/NCYL` 生成无效 BREP 时在诊断中带出参考号、noun、`DIAM` 与 `HEIG`，
  零尺寸根因不再只显示“invalid BREP shape”。
- 坏基本体不再按请求模式升级成生成失败：缺失 BREP、非法 BREP 与 NaN 变换一律记进
  `geom_error` 后跳过这一件，剩下的照常生成；账本写不进去也只发一句 warn，与布尔
  那条链的 `note_skip` 同一纪律。此前这三处按 `targeted`
  （`debug_root_refnos.is_some()`）分叉 bail 掉整个生成根——请求模式不是正确性边界，
  源库里那个没名字也没尺寸的空 NCYL `24381/38635` 因此让 FRMW `24381/38614` 常驻
  500、`regen_root` 连撞 5 次成死信，同一份数据走全量入口却只是少画一件。世界变换
  与属性**查询失败**仍然硬失败：读不到与读到坏数据不是一回事。
- 模型死信公告改为状态指纹驱动：首次/变化立即输出，相同内容最多 300 秒一次，清零仅
  输出一次恢复消息；30 秒 worker 退避和 Model→Room 阻断顺序保持不变。

## 2026-08-20

### 新增

- 新增独立的数据批次停滞看门狗：任务在 `queued` / `held` 超过 60 秒时，不再只靠
  在线 `/health` 与 `/queue` 排查，而是把暂停、上弦、阶段屏障、epoch、worker 存活、
  会话窗口和初始化 blockers 组成一条 `AIOS-QUEUE-STALL` JSON，同时写 stderr 与
  `logs/queue-stalls-YYYY-MM-DD.jsonl`；同一任务每 5 分钟续记一次。看门狗与唯一 worker
  分属两个 Tokio 任务，因此 worker 卡在数据库 await 或已经退出时仍能留下异机可带走的
  离线证据。

- 复验 issue-019 的 T11b 固定存量删除 live：在仅含 SYST+8000 的隔离项目副本中，
  `test_net_window_agrees_on_a_stock_deletion` 以 `1 passed in 20.65s` 通过；窗口
  25..=26 的净三态为 `0/1/2`，固定 EQUI/BOX 从活行准确变为墓碑，写回水位为 26，
  finally 恢复后的 db8000 SHA 与起点一致。证据与 live-test ledger 已同步。

- 复验 BRAN/TUBI 房间计算合成 live 链：`live_room_tubi_row_enters_tree_and_tracks_regen`
  在 8071 一次性空库通过，确认 BRAN 重生成后的隐含 TUBI 进入空间树并新增正确的
  `room_relate` 成员边；证据见 `docs/evidence/2026-08-20-bran-room-tubi-live.md`，并已
  回填 live-test ledger。

- `PrimRevolution` 接进 `tessellate_libgm_param`（specs/009 的回转支）：此前它掉在
  `_ => Ok(None)` 里，而 `Ok(None)` 就是「回退 OCC」的信号——PANE 的负实体大量是
  NREV，正体走 manifold、负体走 OCC，这一减整条设计布尔又被拖回 OCC。新增
  `tessellate_revolution` 语义逐条对齐 OCC 权威实现 `Revolution::gen_occ_shape`：
  倒角复用挤出那份 `gen_polyline_original` 离散（`flatten_extrusion_loop` 改名
  `flatten_profile_loop` 两边共用），角度按「≈360 / >360 / ==0 一律当整圈」归一，
  回转分段数按弦高容差 `R(1−cos(π/n)) ≤ tol` 算并夹在 [12, 512]。换算进
  「(半径, 轴向)」二维系那一步行列式是 −1 会翻绕向，而 `FillRule::Positive` 只填
  逆时针环，所以按外环有向面积统一翻一次（所有环一起翻，保住外环与孔的相对绕向）；
  摆回本地系的变换刻意保持 det = +1，轮廓落在轴负侧时半径方向与出平面基向量一起
  取反，否则网格被镜像、负实体法向朝里，减出来是反的。**出平面的轴仍回 `None`**：
  manifold 的 revolve 只认一种摆放，PDMS 的 REVO/NREV 都满足，不满足的宁可走 OCC
  也不硬凑形状。三条单测：=24381/36945（1RX-RM13 穹顶）那颗「圆柱 − 半球」负实体
  与 ⅓πR³ 对拍（轮廓含「倒角吃光两条腿、四顶点里两个坐标重合」的退化写法）、
  轴负侧顺时针轮廓不镜像、出平面轴回退行为钉死。

- 新增 `src/fast_model/libgm_discretise.rs`：圆怎么分段，全库唯一一份，逐条移植
  libgm 的权威规则（IDA 逆出 `d2_numberOfSegmentsForCircle` / `d2_numberOfSegmentsForPartRev`，
  两者都是 libgeom 导出、libgm 只是导入方，跟 `leftShadow` 同一类；全文与常量记在
  `plant-4/libgm-boolean-algorithm.md` §7.9）。原先两处各写各的：
  `manifold_tessellate::circular_segments_for` 的弦高公式 `π/acos(1 − tol/R)` 与
  libgm 一致但漏了**段数取到 4 的倍数**与**步长封顶 45°（整圆最少 8 段）**（下限写的
  是 12）；`sweep_mesh::arc_segments` 则是拿扫角直接除步长，而 libgm 是**先算整圈段数
  再按角度等比例缩、最少 2 段**——两种算法的结果会差一段。
  那个「4 的倍数」不是凑整：它保证 0/90/180/270 落在网格上。少了它段数会跟 E3D 差
  1~3 段，而 §6.11 的 `cancelFacets` **只消全等重叠**，共面两层侧壁段数一差抵消就整个
  放弃，结果里留一层内壁。这是布尔收不收敛的问题，不是画质问题。
  单测按 Core3D 主初始化实际用的 `gm_SetDefaultFacetTolerance(0.5)` 钉一张表
  （R=25→16、100→32、250→52、3000→176、23400→484），另按 `arctol_` 初值 0.1 钉一张，
  并断言段数恒为 4 的倍数、恒落在 [8, 512]。R=100 恰好是 32 —— 那正是本仓一直写死 32
  的来处，也说明它只在那一个尺寸上对。512 上限是我们自己的护栏（libgm 没有），
  差异写在常量注释里。
  **两件还没做的写在模块文档里**：(1) 容差口径未对齐——libgm 是一个全局 `arctol_`
  绝对量，我们是每个原语按自身尺度给 `tol()`，比例容差会让段数与尺寸无关；
  (2) 其余曲面原语（圆柱/球/碟/锥台/两种环面/切角柱）仍写死 32 段，尚未走这条规则，
  因为改段数会打断「所有普通圆柱共用一个 `CYLINDER_GEO_HASH` 单位网格」的复用，
  得先把段数并进 hash。

- 补齐 libgm 截面弧的**取点相位**：`GM_Extrusion::calcFacets` 实际逐 span 调
  `D2_Span::getApproxPolyLine(tol)`，并非把每段弧按扫角均分；它先按整圆容差求 `n`，
  再只插入落在弧角区间内的固定格点 `k·2π/n`，首尾保留真实端点。现将
  `manifold_tessellate::flatten_profile_loop` 与 `sweep_mesh::flatten_loop` 同时切到
  `libgm_discretise::span_polyline_by_tol`，避免挤出端面与扫掠截面对同一 PAVE 得到不同
  顶点。回归钉住 `R=100, tol=0.5, 5°→95°` 的 10 点格子、正反 bulge、跨 0°、
  近零 bulge 与 RM12 两道大圆弧；部署后 `24381/36931` 的单位网格为
  4392 顶点 / 1464 三角，Plant UI 中弧面恢复且 ERROR=0。

- `PrimLoft`（SweepSolid，结构件扫掠体）接进 `tessellate_libgm_param`：内核
  `sweep_mesh::sweep_solid_mesh` 早已三支齐全、纯函数单测全绿，卡着没接的是各自的
  RVM 门（T019/T020/T022）。这一轮先接线、RVM 门欠着——它是活库里数量最大的一类，
  不接等于 OCC 退不掉。分派仍走 `do_solid_segments()`（Core3D `DB_Gensec` 的权威
  三支），与 `SweepSolid::gen_occ_shape` 同一份输入、同一个局部坐标系：直脊无斜切
  → 挤出，真斜切 → 放样（端面变换用 OCC 那边 `Solid::loft` 用的同一个
  `get_face_mat4`），弧脊 → 回转。核对过 OCC 那条路上唯一看着像分歧的地方——SANN
  的 btm/top 两条 wire 其实是同一条（`gen_occ_sann_wire` 里区分二者的那段是注释掉
  的），所以直脊 SANN 走 loft 与走挤出同形。新增两条接线测试：三支都不得回 `None`
  且各自过实体体检、未知截面响亮失败（OCC 那边同样是 `Err`）。
  **至此 `PdmsGeoParam` 的 14 个 Prim 变体全部由 manifold 路径覆盖。**

- `tessellate_libgm_param` 的兜底从 `_ => Ok(None)` 改成穷举
  `Unknown | CompoundShape`：变体全覆盖之后，`_` 的意思就从「还没做的类型」变成了
  「以后新增的类型自动悄悄回退 OCC」。往 `PdmsGeoParam` 加变体现在是一条编译错误。

- `PrimPolyhedron` 接进 `tessellate_libgm_param`（specs/009 T010）：面片壳本来就是
  现成的封闭壳，**不需要任何 CSG**，没有理由把整条链路拖回 OCC。解析阶段已带
  `mesh` 的直接用，否则逐面剖分——每张面按 Newell 法向定面内二维基、`loops[0]`
  外环其余是孔走 earcutr（面片常有孔，扇形三角化会填实），顶点不跨面复用所以
  平面片按面着色，最后 `sweep_mesh::orient_outward` 按有向体积把整壳翻成外向。
  容错口径与 `Polyhedron::gen_occ_shape` 一致（单张面建不出就跳过），但一张都剖
  不出来是 hard fail，不回 `None` 悄悄溜去 OCC。五条单测：立方体六面对拍体积与
  包围盒并过闭合可定向体检、整壳反绕向被体积兜底翻回、带孔面积 = 外环 − 孔、
  自带网格优先、无面片与全共线两种响亮失败。

### 修复

- 跨项目 DICT/CATA 裸 dbnum 选主改为「显式名单 + `included_projects` 顺序」两段式，
  与全量同步侧对齐。`select_catalogue_candidates` 的 rank 此前只从
  `catalogue_project_priority` 建，没被点名的项目一律 `usize::MAX`，一组候选里全是
  `usize::MAX` 就抛「没有 catalogue_project_priority 选主」——于是**配置里漏写一个项目
  就阻断整个 Catalogue 相位**，而 ADR-025 的相位屏障会把它后面每一条 Design 批次一起
  钉在队列里（现场：`blockers:["catalogue: 跨项目 CATA/DICT dbnum=7000 冲突且没有
  catalogue_project_priority 选主"]`，同时 `blocked_by_phase:"catalogue"`、
  `design:{total:1,pending:1}`、`CATA 依赖清单已激活：0 个选中文件`）。这既不是文档口径
  ——2026-08-04 配置变更记录里这个键的默认值一直写着「与 `included_projects` 同顺序」
  ——也不是另一条路径的口径：`versioned_db::database` 的全量同步本来就是先推显式名单、
  再把剩下的 `included_projects` 按书写顺序接在后面。现在 rank 补上同样的尾巴
  （偏移量从 `priority.len()` 起跳，保证点名的恒压过没点名的；`or_insert` 让
  `included_projects` 里写重的名字只认第一次）。**打错字仍然阻断**：名单里出现
  `included_projects` 之外的项目或重复项目照旧记 blocker——漏写是「没意见」，写错是
  另一回事，两者处置相反。落选方仍照打 `[manifest] … 被项目优先级遮蔽`，只是不再
  停下来等人。三条单测：空名单按 included 顺序选主（候选刻意倒序传入，钉死「赢家来自
  配置顺序而不是遍历顺序」）、半份名单里点名的在前其余按 included 排、未知项目仍阻断。

- 房间增量的四条清边语句与纠正窗口的 `pe_owner` 清理改走图遍历边目标：
  `render_room_relate_statements`、`render_room_panel_relate_write`、
  `render_panel_room_topology_statements`、`render_element_relate_write` 以及
  `existing_members_of_panel` 此前都是 `WHERE in =` / `WHERE out =` 的谓词形式，
  `window_repair` 的硬删除则只把成对语句改了一半（`increment_pipeline` 那条早已是
  `DELETE {pe}->pe_owner`）。**DELETE 拿不到二级索引**：8009 现场三张边表其实都有
  `(in, out)` UNIQUE 索引（`unique_room_relate` / `unique_room_panel_relate` /
  `unique_pe_owner`，前两条在仓库源码里找不到创建处——索引口径不能只看源码），但
  10 万条边、索引在场的隔离实例实测 `DELETE … WHERE in = X` 仍是 3.132s，
  `DELETE X->room_relate` 244.973ms；`out` 侧 2.953s vs 24.455ms，删除行数逐对相同。
  面板重算按面板发、元素分支按元素发——全量重建就是面板数乘边表全扫。
  读侧另算：`out` 侧连 SELECT 都够不着索引前缀（8009 只读实测 1.12s vs 392µs），
  而 `existing_members_of_panel` 是按 `in` 的 SELECT，本来就走索引，改成边目标
  **没有收益**（791.9µs vs 1.1236ms），只为四条房间语句形状一致。取证见
  `docs/evidence/2026-08-20-edge-scan-sweep/`。排除子句从
  `AND in NOT IN [..]` 变成挂在边目标后的 `WHERE in NOT IN [..]`，形状仍过
  ReplaySafe（`is_bounded_target` 认边目标）。新增两条回归：一条钉住五条语句的边目标
  形状并禁止谓词写法回流，另一条在 mem 库上实测「边目标 DELETE 真的删掉
  `INSERT RELATION` 写入的边、且不误删同面板的其它成员边」——这是本次唯一会静默出错
  的地方，若引擎只在 `RELATE` 时维护邻接索引，先清后写会退化成只写不清且不报错。

- 同一条纪律补到另外两处：`query_service::spatial_bounds` 的
  `FROM inst_relate WHERE in = pe:{refno}` 改成按记录 id 直接寻址
  `FROM inst_relate:{refno}`（写口 `pdms_inst` 用同一对 `to_inst_relate_key()` /
  `to_pe_key()` 渲染 id 与 in，两者选中同一行；数组 id 的版本化历史行的 `in` 是
  版本化 pe，本来就不在命中集里）——`inst_relate` 是唯一连 `(in, out)` 索引都没有的
  边表（只有 `anc` / `dbnum`），`LIMIT 1` 也救不了没有 aabb 的 refno，那要一路扫到
  表尾；8009 现场只读实测 968.4ms vs 直址 121µs。`staging::preload` 删除子树的 `pe_owner`
  拓扑拷贝从 `WHERE in IN [..] AND out IN [..]` 改为按成员出边走图再过滤 `out`
  （`IN` 不是 `=`，`unique_pe_owner` 用不上），与它上面那条「`pe` 按记录 id 直接
  寻址、895 万行 64.4s → 0.5ms」的注释同源。`preload` 的 `pe_owner` 夹具顺带从
  `RELATE` 换成生产写口的复合 id `INSERT RELATION`：子树闭包与删除桶拷贝都走图遍历，
  夹具用 `RELATE` 的话「邻接索引是否也对 `INSERT RELATION` 维护」这个区别测不出来，
  而它一旦不成立就是删除级联在暂存里走不到后代、`status = OK` 地少删一片。

- RVM 基准对拍工具的四处边表全扫一并收口：`compare::render_children_select` 的
  `FROM pe_owner WHERE out IN [..]` 改成 `{owner}<-pe_owner`——它在
  `load_subtree_refnos` 的 BFS 里每层每块各发一次，是全仓边表全扫**次数**最多的地方，
  一次子树对拍要扫几十遍整张 `pe_owner`（8009 现场 912 万条边）；`compare::load_gen_side` 的
  `FROM inst_relate WHERE in IN [..]` 改成按记录 id 直接寻址；`mesh_compare` 的
  `gen_world_mesh` 与 `ensure_booled_mesh_files` 的 `WHERE in = {pe_key}` 改成
  `{pe_key}->inst_relate`（这里拿到的是字符串键，走图不必再做 id 字符串手术）。
  mem 库实测确认两件事：`INSERT RELATION` 写入的 `inst_relate` 行经
  `{pe}->inst_relate` 取得到、计划为 `Iterate Edges`；直接寻址一组 id 时不存在的那些
  **不出行**，与 `WHERE in IN [..]` 逐字同一份结果——`load_gen_side` 拿整棵子树当输入，
  绝大多数节点没有 `inst_relate` 行，多出 NONE 行就会让 `GenRow` 反序列化整批失败。

- 边表全扫收口完毕：诊断 bin、夹具与 live 集成测试里剩下的谓词写法一并改为边目标
  或记录 id 直址——`test_tubi_inst_relate`（嵌套两层全扫）、`l3_suite` 的场景快照
  （`inst` / `owner` / `room` 三项，每场景拍三次）、`room_fixture` 的清理与两处
  `WHERE out =`、`room_live_issue7` 七处、`issue7_e2e_increment` 四处、
  `staged_transform_e2e`、`increment_pipeline` 与 `cata_closure` / `cata_model` /
  `model_update_plan` / `fork_surreal_compat` 的 live 断言、`spatial_tree_8000.py`。
  `room_fixture` 那四条 `WHERE in IN [..] OR out IN [..]` 换成 `{pe}<->{table}`
  （mem 实测确认 `<->` 一次删掉两个方向）。

  两处**刻意没动**：`l3_suite` 快照里的 `geo_relate WHERE in IN [pe:..]` 传的是
  `pe:` 键，而全仓每个 `geo_relate` 写口（`pdms_inst`、`occ_generate`、
  `manifold_bool`、`increment_manager`）的 `in` 都是 `inst_info:`，照此它应当恒空，
  可 m1 的 I-3 断言又要求它恰好是 5——两者不可能同时成立，得连断言一起判，不是性能
  收口该顺手改的；`staged_regen_e2e` 与 `Run-RoomE3DE2E.ps1` 的 `WHERE in = X OR
  anc CONTAINS Y` / `OR in.owner = X` 带解引用与 `OR`，图遍历不等价。

  `test_tubi_inst_relate` 的重写踩到一个坑，记在这里：从**一组**记录出发的
  `$var->inst_relate` 会把每个字段裹一层数组（`refno: [pe:xxx]`），必须
  `array::flatten(...)`，否则下游 `RefnoEnum` / `String` 反序列化整批失败。

- 修复 `node_gen_room_probe` 在大表上用 `WHERE out IN (...)` / `WHERE in IN (...)`
  扫描 `room_relate`、`room_panel_relate` 的性能问题：改为从 `pe` 记录执行
  `<-room_relate`、`->room_relate`、`->room_panel_relate` 边遍历。8019 实测执行计划
  从 `Iterate Table` 变为 `Iterate Edges`，BRAN 根 `24384/23257` 的 10 元素子树汇报
  即时完成；新增源码回归测试，禁止旧扫描 SQL 回流。

- 整库快删与 ADR-021 回退重建的 `pe_owner` 清理改走 OWNER 复合 id 区间：每个权威
  Ref0 一条 `DELETE pe_owner:[pe:{ref0}_0, NONE]..=[pe:{ref0}_9999999999, ..]`，取代
  原来 `array::flatten(SELECT VALUE ->/<-pe_owner FROM pe:{ref0}_0..)` 那两句图遍历。
  边 id 固定是 `[OWNER_PE, 槽位]`，owner 在区间内的边本就是 id 连续的一段，不必先把
  边 id 全捞进内存——百万级 PE 的库上那是清库耗时的大头。少掉的 `->pe_owner` 方向
  在整库清理里是空扫（所有权链不跨库，且本 dbnum 的每个 Ref0 各出一条），**部分
  裁剪不满足这个前提**，`prune_above_watermark` 一行未动。这个跨 owner 的区间形状是
  `staging::replay_safe` 明令拒绝的写法，该限制不放宽，边界就地写在 `fast_delete.rs`。
  顺带换来幂等：新语句不读 `pe`，上一轮清库半途失败留下的边下一轮能清掉（遍历从空
  区间出发永远够不着）。后置条件同步加严，逐 Ref0 数 `pe_owner` 残留必须为 0，删边
  语句写歪时当场报错而不是报成功。三条回归测试 + 隔离库实测（目标区间 3 → 0，上下
  两侧相邻 Ref0 与前缀延长型 Ref0 全部保留）留证于
  `docs/evidence/pe-owner-range-fast-delete-20260820/`。

- 修正 `db8000_two_delete_fixture`、`db8000_session_pairs` 的 CI 依赖图：移除
  `legacy_session_replay` required-feature，所有窗口采集改走生产权威入口
  `IncrementPipeline::collect_window`。跨会话断言改为“窗口右端每个 refno 恰好一个
  终态操作”，索引差分与 vendor 对拍直接比较净窗口，不再把逐会话操作并集当生产语义。
  固定工具链下默认 feature 命令通过 6/6 与 21/21，Cargo metadata 确认两个 target
  `required_features` 均为空。

- 为 `/health.room_build` 增加 2 秒硬超时。现场新版启动期 SurrealDB 繁忙时该新增
  查询曾让整个健康接口超过 10 秒无响应；现在返回 `rebuild_required=true` 与超时原因，
  不再让可观察字段拖死探活，并有 pending future 回归测试。

- 修复布尔成品已经写入 `booled_id`、但 `insts_flat` 尚未回填时 Plant UI 仍加载正体
  原语的问题：Manifold/OCC 两条布尔成功路径现在与 `booled_id` 同步写入单位变换的
  成品实例，平表补扫也优先使用该成品；Plant UI 的旧库回退查询同样优先读取
  `booled_id`。`=24381/36945` 不再加载带 `Z×234` 变换的正体圆柱，右视图恢复为
  宽高比 2:1 的半球。新增写入不变量、平表优先级和读侧 identity transform 回归。

- 修复 Plant UI 中 `=24381/36945`（1RX-RM13 穹顶）与 E3D 外观不一致：首先发现
  Plant UI 实际读取 `test-worklspace/bin/assets/meshes`，其中仍部署着 2026-07-20 的
  旧网格，而本轮生成结果在仓库 `assets/meshes`，两者 SHA256 不同；已按哈希暂存并
  原子替换部署网格。其次把 `manifold_to_plant_mesh` 从无条件逐三角面法线改成按 f64
  精确位置归组、面积加权的折痕感知法线：夹角小于 10° 的曲面片光顺，端盖/侧壁和
  箱体棱边继续拆组。新增球面共享法线与箱体三组硬边回归。定向重生成后 AABB 保持
  `46919.106 × 46919.106 × 23400 mm`；网格逐点球面方程回归也已补齐。最终界面实例
  选路问题由上一条修复；部署前网格、源码、补丁和可执行回滚均已留证。

- **`=24381/36945`（1RX-RM13 穹顶）现在能端到端出一个干净的半球。** 新增
  `rm13_dome_pane_minus_nrev_is_a_hemisphere`：按活库里存的两个参数三角化，再走生产
  那条 manifold 差集，判据是 `Manifold::genus() == 0` 加 f64 体积对 ⅔πR³（0.1%）加
  包围盒。这颗构件把两层「倒角把直边吃光」的把戏叠在一起（PLOO 四角 FRAD 等于半边长
  → 方变圆；NREV 四个顶点里两个坐标重合 → 倒角吃掉两条腿只剩一段圆弧），是整条链路
  最硬的压力测试，一口气挖出四个独立缺陷，逐条记在下面。

- 修复 `NREV` / `REVO` 同时命中 LOOP owner 与 primitive noun 表时被重复派发的问题：
  这两类参数必须先由子 LOOP/PLOO 拼装，现统一从 primitive worker 路由中排除。
  定向生成不再因 `NREV` 的预期 `None` 误报 hard fail；`24381/36945` live 强制替换
  返回 `Generated`、可渲染 1、写入 2，几何/AABB/空间树计数保持稳定。

- 挤出的顺时针轮廓建不出任何东西。`tessellate_extrusion` 靠 `FillRule::Positive` 挖孔，
  而它只填逆时针环——PDMS 的轮廓**不保证绕向**（那颗 PANE 的 PLOO 就是顺时针），
  于是截面直接是空的、`bail!`、回退 OCC。按外环有向面积统一翻一次，所有环一起翻，
  外环与孔的相对绕向不变。回转支 2026-08-19 就修过同一个坑，挤出支漏了。

- 布尔结果的法向在 f32 上算，23400mm 处只剩三四位有效数字：轻则法向歪，重则两个顶点
  舍入到同一个 f32 → 叉积为零 → `normalize()` 给出 **NaN 法向**，一路写进 `.mesh`。
  `manifold_to_plant_mesh` 改为在 f64 顶点上算法向，并丢掉 0.1µm 内塌陷的三角
  （丢它不开洞：设 A、B 重合，这个三角贡献的是一条自环加上互为反向的两条边，
  自己跟自己抵消，其余边的配对一条没动）。

- **共壁负体的差集会碎成一地薄片。** PDMS 里负体常与母体共壁（那颗穹顶的 NREV 就是
  跟 PANE 同一个圆柱，只在内部多挖半球）。两个圆柱各自离散出的 484 边形只要不是逐位
  相同，差集就沿着共壁碎开——实测 **亏格 −131，即 132 个互不相连的壳**：一个半球加
  131 片碎屑。碎屑总体积只有 1e-7，体积对拍根本发现不了，但它们会进 `.mesh`、会
  z-fighting、会让后续布尔更难收敛。`subtract_negatives` 现在把每个负体按**自身包围盒
  中心**放大 1e-6（那颗穹顶上是 0.023mm）再做差，亏格回到 0，体积偏差 0.018%。
  E3D 不需要这一步，它靠共面反向面逐面全等抵消（§6.11），而那条路要求两侧段数与相位
  完全一致。

- 挤出与回转的弦高容差改用**绝对量** `FACET_TOL_MM = 0.5`（Core3D 主初始化传给
  `gm_SetDefaultFacetTolerance` 的就是这个值）。原来用的是 `BrepShapeTrait::tol()`
  ——按自身尺度给的**比例**容差，`tol/R` 恒定，段数于是与尺寸无关：同一个圆在挤出侧
  和回转侧只要包围盒不同就分成不同段数（那颗穹顶正是正体 60 段、负体 84 段）。
  改成绝对量之后两侧同为 484 段，配合 §7.9「段数取到 4 的倍数」保证相位也一致
  （顶点都落在 0/90/180/270 上）。**其余曲面原语仍是写死 32 段**，见上面 `libgm_discretise` 那条。

- 修复提交查询超时被当成「确定性拒绝」、一次尝试就把暂存窗口永久判死的问题。
  等满 `COMMIT_QUERY_TIMEOUT` 没等到服务端回话，语句既没被接受也没被拒绝，这是
  活性问题；但非 outcome-sensitive 的那条分支只报「终止本查询」，
  `staged_writeback_failure_is_transient` 认得的三个标记一个都不沾，于是走确定性
  出口直接落终态阻断——现场 dbnum=8000 的 `242..=243` 就是这样卡在一个 32 行 /
  1.6 KB 的块上、水位停在 241。两个超时分支现在共用 `COMMIT_QUERY_TIMEOUT_MARKER`，
  分类器据此把它归进瞬时桶；journal 块本来就按幂等重放设计，重放安全。预算是有限的
  （`STAGED_COMMIT_TIMEOUT_ATTEMPTS = 3`）：无限重放会走到另一个极端，一条真的跑不完
  的语句抱着 `STAGED_COMMIT_SERIAL` 每 30 秒烧一个 120s 死线。`is_transient` 判据
  因此多收一个「这是第几次」。

- 修复同 dbnum 多文件的阻断一律记在 Design 阶段的问题：CATA/DICT 的同号冲突也会
  因此把 design 拽成 blocked，而 design 侧可能一个问题都没有，阶段就绪判定从此
  说谎。`blocked_dupes` 现在带着 `DataPhase::of_db_type` 判出的阶段入账，与同一段里
  `manifest_totals` 的口径一致。顺带修掉 `dependency_manifest_version` 连着被赋两次、
  第一个直接被遮蔽的问题——那行日志此前数的是依赖清单的文件数、打的却是身份清单的
  版本号；两份清单各有独立计数器，现在各报各的。

- `boolean_generation_refreshes_aabb_after_final_relations_exist` 这道顺序门自己红了：
  它按源码文本找 `apply_insts_boolean_manifold(&target_visible_refnos`，而那处调用早已
  被 rustfmt 拆成多行，带实参的针扎不中，`expect` 直接 panic。针改成只认函数名。
  值得记一笔的是失败方向：源码断言找不到锚点时是「测试红」而不是「守卫失效」，
  这次运气好；反过来写（找不到就当通过）会安静地把不变量丢掉。

- 修复增量看门狗在共享盘抖动时可能整体死锁、且此后不再发现任何增量的问题。
  三件事凑成一个锁环：notify 8.0.0 的 poll 线程**同时持有 `watches` 与
  `data_builder` 两把互斥锁**同步调用事件回调（整轮 rescan 都在锁内）；回调过去是
  往容量 1 的 futures channel 上阻塞发送，而该 channel 的真实容量是
  `buffer + sender 数` = 2，一轮 rescan 里变化文件超过两个就把 poll 线程堵在发送上
  （与「单轮重扫比轮询间隔慢」无关，光靠数量就能触发）；唯一能腾出容量的是
  `async_watch` 的事件循环，而它同一时刻可能正走重挂分支，`MountState::mount` →
  `PollWatcher::watch()` 要的正是 poll 线程手里那两把锁。回调现在只做一次非阻塞
  `try_send` 置位，永不等待；`the_poll_watcher_callback_never_waits_on_the_event_loop`
  钉住这条（复现它要真的把共享盘挂掉，只能钉源码）。

- 修复一次 SAVEWORK 会换来 K 轮背靠背完整重扫的问题：notify 的 `rescan` 对**每一个**
  变化 path 单独发一条事件、事件之间零合并，而每条候选事件此前都独立触发一轮完整
  清单重扫，K 轮扫出来的还是同一份结论。现在事件只把 `WatchSweepGate` 置脏，首事件
  立刻开一轮，最小间隔（`AIOS_WATCH_MIN_SWEEP_GAP_SECS`，默认 5s）内的后续变化攒成
  一位脏，闸一开补**恰好一轮**。这是限频不是丢弃——闸内到达的变化完全可能是上一轮
  重扫**之后**才发生的新保存，脏位只准被真正跑出去的那一轮清掉，否则它就只能等下一
  拍周期对账（300s）。两条纯状态机回归钉住这两半语义。与「事件只标脏、判定必须走
  完整清单」不冲突：闸只改什么时候调那条权威路径，不新增任何按事件的局部判定。

- 修复 PollWatcher 轮询线程死亡时无人知晓的问题。此前 `async_watch` 把「信号流关闭」
  当作看门狗死亡的判据，而那个条件永远不会成立：发送端活在事件回调里、回调活在
  `watcher` 的 `data_builder` 里，只要 `watcher` 对象还在栈上，poll 线程死透了发送端
  也不会被丢弃。notify 8.0.0 又把 `thread::spawn` 的返回值直接 `let _ =` 丢掉，线程
  起不来与 panic 掉一样没有声音——外在表现就是「服务在跑、文件改了没反应」，只有
  周期对账还在兜。现在每一拍对账都主动 `PollWatcher::poll()` 探一次（控制 channel
  的 Receiver 正是被那条线程拿着的，线程没了 send 就失败），失败即按永久性故障逐拍
  告警并说明当前退化成多少秒一次的发现。探不了「线程活着但卡死」：从外面看它和
  「厂里没人动」一模一样，需要 notify 自己吐进度。

- 共享盘重挂轮的 `MissedTickBehavior` 补成 `Delay`（此前是 tokio 默认的 `Burst`）：
  本循环里每一拍都可能 await 一轮完整重扫，被挡久了以后 `Burst` 会把欠下的拍一次性
  连补出来，等于在最忙的时刻再排队几轮重挂。周期对账那一拍早已是 `Delay`。

- `async_watch` 的两个间隔默认值抽成 `DEFAULT_WATCH_POLL_SECS` /
  `DEFAULT_WATCH_RECONCILE_SECS`，并加一条守卫断言子集轮的默认值必须比全集对账轮密
  （调反不会有任何东西报错，但最贵的那一轮会变成主路径）。**两个值本身都没动**：
  子集轮询仍是 30s，周期对账仍是 300s。曾计划把子集轮询降到 5s 以缩短发现延迟，
  评审后撤回——决定负载的不是「单轮重扫多快」（本地实测 0.35~0.83s），而是「一次
  保存产生多少轮重扫」，而后者在共享盘上一个数都还没量到；上面两条修复正是把这个
  乘数压掉的前置条件。顺带更正一处此前写错的算式：`MAX_ATTEMPTS = 5` 撑出的
  `5 × 300s = 25 分钟`故障容忍窗口只在**文件安静**时成立，park 计数的条件是
  「同 dbnum 窗口右端没前进」、不区分是哪种来源把它重新入的队，watch 重扫一样在烧
  这份预算。

- 修复相邻构件方向不共线时直段被静默丢弃、三维里只剩一段空白的问题：轴线判定不再决定
  「写不写这一行」，只决定这一行是不是实体管。不可成管的连接照样落库，带
  `invalid` 与 `invalid_reason`（`direction` / `no_bore`），位姿改用局部 `+Z` 贯穿两个
  连接点，让查看端既有的虚线中心线正好画在 E3D 画点线的位置。口径解析提前到判定之前，
  拐死的连接也报得出它本该有的口径。查询层的诊断标记从「端点已删除」扩成
  「记录自带标记 or 端点已删除」，缺字段的历史行仍按可成管处理。

- 修复 BRAN 直管关系在增量重生成后保留旧高位索引、且新关系引用未落库
  `trans` / `aabb` 内容的问题：每个 BRAN 现以单个事务删除旧出边、持久化内容寻址记录并
  写入完整 `tubi_relate` 集合，空集合同样生效；直接写与 ADR-017 暂存写共用
  `execute_model_write`。元素级联清理同时删除其直管出边，并补充变短、空集合、幂等重放、
  内容可解引用及事务失败保旧回归。

## 2026-08-19

### 新增

- 引入 ADR-037 dabacon 快照完整性契约：`pdms-io` 以单一打开句柄生成稳定文件身份
  `SnapshotToken`，净窗口、冻结会话和模型计划终态存在性复核共享同一文件世代与
  target root；`parse_pdms_db` 增加不依赖属性字典的最小元素身份解码。

- `watch_dbnums` 限定模式增加权威 CATA 引用清单与元素级引用闭包：完整扫描继续裁决
  CATA 身份、重复文件和项目优先级，但不排全量 Catalogue 批次；模型窗口只索引被监听
  DESI 与裁决后的 CATA 文件。任务与健康接口新增依赖阶段、文件、解析/缺失计数和
  300 秒停滞截止时间。

- 暂存窗口按 sesno 拆窗（ADR-017 修订二，形态 C：预算式定窗 + 触顶收窄兜底）。
  第一层：`AIOS_STAGING_WINDOW_MAX_SESSIONS` 会话预算在**执行侧**收窄应用窗口右端，
  切点只落在真实 SAVEWORK 边界（会话号稀疏，取第 N 个真实会话而非按号算术）；预览与
  看门狗仍看整段待应用窗口。第二层：资源废弃档位触顶不再只记阻断，把该 dbnum 的会话
  预算收窄一档（减半、地板 1 个会话）再交还，预算 1 仍触顶才是真阻断；收窄记录追平
  `file_latest` 时清除。截断窗口提交成功后余量**立即重排**（不等下一轮重扫），
  `file_latest_sesno` 与并入基线沿用冻结批次那份——余量不是新观察，上界也绝不改写
  `FileObservation.file_latest_sesno`（源码钉）。相位纪元批次（`epoch_id > 0`）一律
  不参与收窄（ADR-025 phase totals 按批次记账，截断算不算相位完成未厘清）。提交侧
  一行未改：水位部分推进与 `align_end_sesno` 本来就容纳更窄的右端。原子单元变小
  （子窗口间可见真实保存点的中间态、跨子窗口的根会重复生成）待业主签字。

- 增量链增加独立阶段控制：`data_incremental`、`model_incremental`、
  `room_incremental` 分别控制数据、模型、房间消费，缺省均开启，并支持对应
  `AIOS_*_INCREMENTAL` 环境变量覆盖。关闭数据阶段仍扫描入队；关闭模型阶段时数据、
  水位与 durable 模型计划照常提交，模型积压留待恢复；下游不得越过未完成上游。
  `/api/v1/health` 同时暴露三个最终生效值，供单阶段调试直接确认运行姿态。

- 净窗口收集下沉到 pdms-io：`session_index_diff`（会话索引双根差分）与 `net_window`
  （净三态 → 操作流合成）整体从 `src/data_interface/` 迁到 `pdms_io::session_index_diff`
  / `pdms_io::net_window`，上层按新路径直接引用，`data_interface` 不做转发。理由是
  **被它替代的逐会话回放本来就在那一层**（`PdmsIO::collect_increment_eles` 一家，
  `legacy_session_replay` feature 门后），把替代品建在上一层等于同一个问题两份实现
  分居两个 crate；更要紧的是 `walk_tree` 复刻的是 pdms-io 的 `btree_search_optimized_recursive`
  路由规则（同键首见者胜、升序前缀、哨兵最左分支、`[本键,下一键)` 区间），复刻件与
  正本隔着 crate 边界时编译器帮不上忙，正本一改这边不会红、只会悄悄口径漂移（后果是
  漏报删除）。两个模块本就零 `crate::` 依赖（只用 `aios_core` + `pdms_io`），迁移是
  平移不是解耦。纯单测 24 条跟着下沉，在 pdms-io 里原样通过。
  **留在本仓**的是批次层的关切：`IncrementPipeline::collect_window`（会话页清单截取、
  `net_caliber_warning` 口径标注、`CollectedWindow` 形状、唯一入口源码断言），以及三条
  跨结构 live 对拍——它们的参照臂 `collect_changes` 是 legacy 回放外加两个 Save Work
  终稿补丁的包装、计时对象是生产入口 `collect_window`，在 pdms-io 里够不着，现迁入
  `increment_pipeline.rs` 的 `cache_tests`。性质 h/i（`db8000_session_pairs`）不动，
  改调 pdms-io 的收集器——它是这次平移的验收面：20 条全绿，其中
  `index_diff_matches_replay_folding_on_every_case_window` 与
  `net_window_collector_matches_replay_ops_on_every_case_window` 证明净口径逐案例
  仍与回放折叠一致。行为零改动，不涉及口径变更。

### 修复

- 修复 dbnum=8000 复制 BRAN 的增量数据已进入但 plant-ui 无节点/模型的链式故障：
  Watch Scope 现保留 PROP 与多项目 CATA 的 Ref0 身份，无关项目 DESI 副本不再阻断
  8000；CATA OWNER 替换按硬行数上限语义分块；Add 的旧入边清理改用图遍历目标，
  避免 `DELETE ... WHERE in` 全边表扫描卡满 120 秒。LSnout 的单元参数现与三位小数
  mesh hash 使用同一归一化值，消除 `0.5555555/0.5555556` 共用同一
  `inst_geo` id 却内容冲突的模型死信。直管段现在只写 `tubi_relate`，不再通过
  `ShapeInstancesData::insert_tubi` 用 `leave_refno` 覆盖 ELBO/TEE/VALV/BEND 的
  `inst_relate` 元件库关系。现场窗口 `242..=243` 已提交到水位 243；新 BRAN
  `24384/26229` 强制重算后保留 17 条元件实例关系和 7 条直管关系，可绘制网格实例由
  25 恢复为 57。

- 修复 manifold 路径生成的 `.mesh` 法线数组为空，导致 plant-ui 将同一 EXTR 端盖
  按三角形渲染出随机明暗的问题：Manifold 输出现在展开为带面法线的硬边网格；回送
  CSG 前按变换后坐标焊接顶点，兼顾 E3D 外观与闭合拓扑。增加 dbnum=8000 会话 239
  V 形 EXTR 回归，验证法线、绕向、闭合性和 CSG 往返。

- 修复 D: AMS 直接图形文档中 `Limits CE` 无响应：该文档暴露的视图类型为 `G3D`，
  原命令只接受 `GM3D` 并静默返回；启动修复现在同时接受两种 3D 视图，并在本地
  Drawlist 为空时先加入、更新当前元素，再执行 limits 与刷新。进一步统一 Model
  Explorer Add 与 Limits 的直接 AMS 路由：`ViewId=0` 时固定选择最新注册的 Drawlist，
  不再让已写入内容的旧 `DL[0]` 通过“成员最多”启发式夺走命令。相机 limits 现在也
  使用该 Drawlist 实际 attachment 的可见 G3D gadget，避免元素已加入但仍只在视口边缘
  露出一段。启动后不再 kill/re-register 已注册命令，因为 Ribbon 的托管委托会继续
  指向被销毁实例；改为保留并验证启动时实例。活动与 shadow PMLLIB 同时幂等修复，
  Frida 临时目录移至 E:，避免系统盘耗尽导致图形 finisher 半途退出。新增 CE 图形物化
  刷新：`drawlist.update()` 后先刷新 attachment view，再读取 limits，修复冷启动第一次
  点击灰屏、第二次才显示的问题；现场清理 5 个并发 AMS 实例后，以唯一 PID 61168 验证
  复制 BRAN 首次点击即可完整显示并居中。

- 修复 D: AMS 启动后 Steelwork 命令刷新反复报告 PML `(2,751)`：shadow
  `PMLCOMMANDMANAGER` 先创建 `STEELWORKGSETTINGS`，并将自动命令事件注册移到支持命令
  装载之后；Design 完全就绪后再运行带 trace 的全局初始化宏，启动失败时直接阻断而非
  把缺失变量留给每次 CE 刷新重复报告。

- 非白名单终稿解析失败、已选中索引 child 读取失败和层级未下降现在硬失败，不再构造
  可推进水位的不完整窗口；MNUM 仅按代码白名单记录结构化诊断。基线 chunk 失败会
  停止后续调度、等待 writer 收口并清空本轮全部已调度 dbnum；冻结 token 跨队列传递，
  同文件 append 仍只读冻结长度与 target，同 sesno 路径换代和空模型计划旁路均在提交前
  拒绝；初次捕获用前后 length/header 复核阻止 append 期间混合世代。CATA 扫描失败、
  空扫描、旧格式空缓存、Required 未解析根或 closure `missing` 均不再形成成功缓存，
  下一次触发会重新扫描。

- 修复复制大量节点后暂存写回把 167 条 journal/869 行合成单事务而永久停在
  `commit`：改为按条数、字节和预计行多维分块，增加块级进展、SQL 指纹和 120 秒
  单查询停滞边界，水位仍只由尾事务推进。

- 修复 dbnum=8000 净窗口删除漏判：父元素 `children_changed` 净减少现在使用
  目标会话 OWNER 成员表仲裁，区分删除与跨 OWNER 搬迁，并沿基会话展开
  不可达子树；控制台新增“成员补删”计数。新增停服维护工具
  `db_window_repair`，以独立 staging journal 纠正已提交窗口并保持水位不变。

- 增量 worker 控制台补齐可观测信息：检测保存时打印保存时间与 sesno 会话区间，
  各执行阶段打印当前窗口，完成时打印新增、修改、删除及合计数量；暂存提交超过
  10 秒时持续输出等待心跳，避免长事务期间看起来没有响应。

- 修复 `scripts/e3d/run_ams_gui.bat` 只启动裸 `des.exe`、未完成 AMS 模型树与
  DrawList 初始化的问题：启动流程现复用 repaired AMS 链，显式绑定
  `D:\AVEVA\Projects\E3D3.1\AvevaMarineSample` 的项目 evars，隔离用户/工作目录，
  跳过串入 YCYK 的通用修复宏，并以 graphics finisher 与 runtime health probe 验证
  MDB、模型树、3D 文档和编辑运行时。

- 收口 Oracle 二次审核缺陷：CATA 缓存使用实际分窗右端并按 Ref0 冲突精确阻断；
  维护纠正复用冻结快照；epoch 激活与任务冻结共用激活门；尾事务使用稳定 commit token
  幂等确认并区分 `commit_tail/commit_reconcile`；planner 对包装后 SQL、行数和条目数执行
  硬上限。manifold 失败改为双策略：暂存窗口 `Required` 阻断水位，窗口外
  `BestEffortFallback` 保留诊断与旧几何。三个依赖仓库和 manifold-csg 均固定已发布 revision。

- 新增布尔降级账本 `geom_error` 表：跳过之后 `model_update_pending` 那一行不再产生，
  控制台那句也会滚走，于是「哪些件的洞没切」事后无从查起。现在每次跳过按
  `(kind, target)` 归行落库——`kind` 为 `bool_pos` / `bool_neg`，带栽在哪块几何
  （`geom`）、累计次数、首末时刻与最近一句错误，同一元素下次布尔做成时自动销账。
  `SELECT * FROM geom_error ORDER BY last_seen_at DESC` 直接可查，`/api/v1/health`
  新增 `geom_errors` 摘要（表空是 `null`）。写的是 `SUL_DB` 而不是暂存路由：诊断账本
  要在窗口回滚后依然留着，累加计数本身也不满足 journal 的幂等要求。

- CATA 引用元素改用确定性 `CONTENT` replacement，UDA 与 owner 集合先清后写；缓存键
  同时包含源窗口右端和 CATA 文件指纹，且只在 DESI 水位提交成功后发布。依赖错误、
  `missing > 0` 与连续 300 秒无实质进展现在使暂存窗口失败并保持水位。周期 reconcile
  遇到运行任务时保留活动 epoch，新的 manifest 延后到任务终态后的重扫激活。

- 修复启动 epoch 延后模型阶段时绕过 CATA Required 依赖，以及 CATA replacement 用
  `FOR $row` 被 staging journal ReplaySafe 拒绝的问题；replacement 改为显式记录目标，
  UDA 以 500 元素为上限合并事务。8000 live 重放解析 404、缺失 0，水位 33→232，
  journal 由 1284 条降至 404 条，失败重放与成功提交均保持水位/暂存原子语义。

- 修复会话预算枚举对 `pdms_io::PdmsIO::get_nearest_large_sesno` 现行 `Option<i32>`
  返回值仍按旧 `Result` 形状解构造成的测试编译失败；缺少后继会话时现在按既有语义
  结束稀疏会话枚举，预算切点仍只落在真实 SAVEWORK 边界。

- 修正 issue #5 真实房间 live 用例的计划断言：生产计划在 BRAN `RegenRoot` 后还会为
  被移动管件排 `PostRegenAabb`，旧断言只接受单个重生成工作项，导致正确计划被误报；
  现在同时钉住整根重生成与靶件 AABB 后处理，仍拒绝管件自己的 `Transform` 路线。

- 修正 `test_cal_rooms` 把旧 AMS 切面的 124 间房/147 块面板写死为永久不变量的问题：
  常规 live 对拍只要求房间与面板集合非空，精确切面计数改由
  `AIOS_EXPECT_ROOM_COUNT` / `AIOS_EXPECT_ROOM_PANEL_COUNT` 显式钉住；核心判据仍是单构件
  增量重算与同轮全量基线逐边相等。测试的空间树前置也改走生产启动同款
  `load_project_tree_verified`，让空间状态机进入 Ready；旧 `load_aabb_tree` 只读文件却不
  完成发布门，当前状态机会正确报 `SPATIAL_TREE_NOT_READY`。

- 生产布尔的空差集不再静默吞件（specs/009 T025 的静态半、ADR-029 决策 3 对齐到
  manifold 生产路径）：`apply_insts_boolean_manifold_single` 与
  `apply_cata_neg_boolean_manifold` 在 `subtract_negatives` 之后各加一道门——差集
  网格为空（verts/idx < 3）就不写 `.mesh`、不写 `booled_id` / `inst_geo`，标记
  `bad_bool` 并出声，与 OCC 对拍路径「切洞结果为空不覆盖已有 booled_id」同一条
  不变量。顺手拆掉同函数里恒假的 `found_need_occ` 分支与恒真的 `success` 死代码。
  源码断言 `empty_difference_is_bad_bool_not_a_silent_swallow` 钉住两条路径。

- 挤出截面的 FRADIUS 倒角不再静默变直角：`tessellate_extrusion` 之前把轮廓顶点 z
  （倒角半径）直接丢掉，带倒角的 `PrimExtrusion` 全部出直角、无警告、不回退。
  现在每环先过 aios-core `wire::gen_polyline_original`（OCC 路径同一份权威实现，
  z>0 顶点换成圆弧）再按 `Extrusion::tol()` 的弦高容差 `arcs_to_approx_lines`
  折线化；首环建不出即失败，孔环建不出跳过（与 `gen_occ_wires` 同一容错口径）。
  样条轮廓（`CurveType::Spline`）没有 libgm 等价实现，回 `None` 走 OCC，不再拿
  控制点连线冒充。新增体积对拍（四角 r=20 方截面 vs 解析值 1%）、带孔轮廓不破、
  样条回退三条单测。

### 新增

- 增量监听限定域：新增 `DbOption.toml` 的 `watch_dbnums` 与 `aios-database serve
  --watch-dbnum 7998,8000`（命令行压过配置），把增量摄入的数据批次圈到指定 dbnum，
  调试时不必再让整个项目 287 个 DESI 陪跑。SYS meta（SYST/DICT/GLB/GLOB）不受限
  ——MDB 的成员名单就存在那些库里，圈掉只会得到「什么都没发现」的假现场。两者都
  没给时判定与本特性引入前逐位相同（`an_unset_watch_scope_leaves_the_scope_verdict_untouched`
  钉住）。它与 `--debug-dbnum` 各管各的（那个额外带链路追踪、刻意进不了配置文件），
  与早已被剥夺增量否决权的 `manual_db_nums` / `exclude_db_nums` 也无关。
  **因为形状与坑出 issue #10 的手写名单一样，护栏比 `--debug-dbnum` 只多不少**：
  跳过理由、重扫聚合、回执声明三处都点名 `watch_dbnums` 并说清限定来自配置还是
  命令行（配置里的名单能躺一个月，命令行的进程一停就没了，两者的处置不同），
  与 MDB 范围判定、调试限定两种嗓音两两无交集；启动横幅同时挂在 `run_cli` 与
  Python `full_init` 上，`/health` 新增 `watch_dbnums` / `watch_dbnums_origin` 两栏。
  新增单测 12 条，含两条源码顺序断言（重扫三个桶的分桶次序、两个入口回执的声明
  位置）。

- 元素 diff 与 core.dll 的对照收口成两条显式已知边界（ADR-032 +
  `docs/evidence/2026-08-19-core-element-diff-boundary-audit.md`），行为未变。原先怀疑
  的五条分歧查证后三条不成立：OWNER 走 `elementIncluded` 我们已按 ADR-009 做了；属性
  宇宙用 schema 表还是键并集在结果上等价；core 按 `DB_Attribute::type()` 分十二类的比较
  **每一类最终都是精确比较**（`D3_Vector::operator==` 就是三个 double 逐个比，没有
  epsilon），分类只因为它拿到的是 typed 值。UDA 的 `isUdaUnset` 需要
  `hasAttributeChangedBetween` 第八参为真，而 `elementsChangedBetween` 传 0——core 自己在
  这条链上就关着。剩下两条记为边界：**A** 成员差分只有整表三态，没有 `DB_MemberCompare`
  的逐成员 kind（`kind == 3` → `elementReordered`），不实现是因为 `ChangeBucket::Reordered`
  今天既没人产也没人读，新增守卫 `the_reordered_bucket_has_no_producer_and_no_consumer`
  钉住这两头、任一被打破即红并指回 ADR（两条回退各自实测变红：`src/` 下多一处提及会被
  点名，`user_change_buckets` 里多一行产出会报「提及数 2 ≠ 1」）；**B** 没有 `DB_Uda::oldToNew` 的旧键归一化——它
  不是值的语义归一化而是键迁移重映射（值 > `0x171FAD39` 时查 `DB_Attribute::findOldKey` /
  `DB_Noun::findOldKey` 换成当前 id），门是 `ityp ∈ {51, 52}`，且同一调用挂在
  `DB_Element::getAtt` 七个重载与 `getInt` 上——**是读路径归一化而非 diff 语义**，真要对齐
  落点在 parse 层。受影响的属性是可枚举的九个（ityp 51：`GTYP`/`USYSTY`/`QUES`/`ATNA`/
  `AKEY`/`CURTYP`/`ATTSET`；ityp 52：`BASETYPE`/`DBELET`），全是 `TYPE=6`/`SIZE=1`，其中
  `GTYP` 就挂在我们解析用 schema 的 55 个 noun 上；但重映射只在**值** > `0x171FAD39`（即指向
  用户自定义的 UDA/UDET）时才动手，暴露面收窄到「用了 UDET/UDA 且定义重编号过」的项目。
  现有 db8000 语料是常规模型数据、本就不含这类事件，探针跑出 0 也只能证明「本语料未观测
  到」，故按 ADR-002「仅在发现与 core.dll 分歧时才对齐」两条都不预先实现。配套把
  `AttrInfo` 加上 `#[serde(default)] pub ityp: Option<i32>`（`None` = 尚未采集，不是取值 0）：
  `all_attr_info.json` 是 JSON 而非 bincode，旧文件不带该键照样反序列化，行为零变化；仓外
  唯一构造点 `noun_layout.rs` 填 `None`。ityp 的数据其实已经在
  `output/noun_attr_fields.json`（`NounLayoutExport.cs` 顺带产出的 57 字段字典转储，4271 个
  属性、ITYP 零缺失），不需要 live E3D 采集。同时记下一条待决项：
  `core-primary-list-e3d31.json` 的 `core_sha256` 与本机 E3D 3.1 `core.dll` 实际哈希对不上
  （字节数精确相同、文件早于采集两个月未改动），而现有断言把该字段钉成硬编码字面量，
  结构上抓不到这类溯源漂移。

- 逐会话实体回放升级为跨仓编译隔离：`old-pdms-io` 与主仓新增默认关闭的
  `legacy_session_replay` feature，生产构建不再编译回放 API；Python 调试绑定、诊断
  探针与 replay oracle 显式启用。以无 feature compile-fail 和生产 check 取代可被
  helper 绕过的 `include_str!` 字符串禁调，净窗口、HTTP DTO、水位和暂存协议不变。

### 修复

- ADR-017 phase-1 审计的四处收口。① **写回的确定性失败原来没有出口**：它无条件走
  `retry_until_recovered`，那个函数返回 `(T, u32)` 而不是 `Result`——4 次快重试之后每
  30 秒重放同一份 journal，永远不返回；而这一整段持 `STAGED_COMMIT_SERIAL`，于是一条被
  持久层确定性拒绝的语句会把 `fast_delete`、提交后空间收敛与其余 dbnum 一起停摆，外在
  表现是「增量跑了、模型没变、重启还是同一区间」，控制台每 30 秒刷同一行。现在按
  `staged_writeback_failure_is_transient` 分流：断连与写冲突照旧无限等（必然自愈），其余
  判死——记 `window_block`（reason 带原始错误）、DROP 窗口、放锁、批次转 Failed；水位没动、
  持久层零痕迹，journal 随窗口丢，恢复路径与崩溃同一条。journal 入口早有
  `ReplayUnsafeRejection` 判死确定性拒绝，这是它在写回端缺失的对偶物。② **而「确定性写回
  失败」不是假想**：暂存库刻意不装 `update_dbnum_event`、持久库装，这道有意的不对称制造了
  一类「暂存全绿、写回逐条被拒」的语句——pe 上一旦生效的是那版对数组形制 record id 用
  `array::at` 的旧实现，整窗口写不回去，而发现它的时刻是解析与生成都已白跑之后。
  `create_window` 现在进程内一次性读回事件定义验指纹（好版含 `string::split`），坏版直接
  拒绝开窗、不建实例；读不到按瞬时故障放行，只缓存成功，排毒后下一次开窗自动放行。
  ③ **资源废弃档位没有可执行的出口**：`Abandon` 只在语句入口 bail，冒泡上来与普通失败无从
  区分，而这类失败重算不会好（同一会话区间必然再次触顶，窗口只被吸收扩大、没有按 sesno
  拆窗的机制）。现在按档位单独记 `window_block`，reason 带测得的字节 / 行数、生效上限并点名
  `AIOS_STAGING_ABANDON_BYTES` / `AIOS_STAGING_ABANDON_ROWS`。④ 退役
  `sweep_orphan_staging_databases_on`：每窗口一个独立 `mem://` 实例之后跨窗口孤儿库不存在，
  生产侧无调用方，留着只会让人以为还有一层兜底回收。回归四条（三条实测过回退变红）：
  `a_deterministic_writeback_failure_returns_instead_of_holding_the_lock`、
  `only_transport_and_conflict_count_as_transient_writeback_failures`、
  `a_rejected_writeback_records_a_block_and_releases_the_window`（源码钉：记阻断 → 放窗口 →
  交还终态）、`the_writeback_schema_gate_runs_before_the_window_instance`（源码钉：兼容门
  必须排在建实例之前）。ADR-017 同步修订，并把两处「文档与实现不一致」写进正文：规则④的
  水位行只读拷入例外，以及 §6 之外那档更宽的模型让位口径（`epoch_id > 0`，重扫排出来的稳态
  DESI 也在内，这批的模型走窗口外逐根直写——决策 1 的整窗口原子在这条路径上按 ADR-025 §7
  被交换掉了）。已知仍未解决并留在台账：拆窗机制不存在；资源计量用 SQL 文本字节做摄入代理，
  `estimate_write_rows` 对带 WHERE 的集合写按 1 行计，两处都低报。

- 去除跨仓构建对宿主 `protoc` 的依赖：`dpcsync` 删除 `build.rs` 与
  `prost-build`，检入字节一致的 `prost` 生成文件；`old-pdms-io` 钉住该提交，主仓
  删除未使用的直接 `dpcsync` 依赖。PROTOC/PROTOC_INCLUDE 均未设置时三仓构建与
  回归通过，依赖树中不再出现 `prost-build`。

## 2026-08-18

### 新增

- 增量窗口收集统一到净窗口单一口径（ADR-031）。`collect_window` 不再读灰度开关、不再走逐会话回放；预览与执行共用 `collect_net_window`（与 core.dll `elementsChangedBetween` 同思想的双根 B+ 差分）。`net_window_collection` / `AIOS_NET_WINDOW` 退役，残留设置在 `run_cli` 与 `aios_db.full_init` 打显式告警——配置层没有 `deny_unknown_fields`，删字段会让 `net_window_collection = false` 被安静忽略，字面意思与实际相反。`collect_changes` 降为 legacy 诊断入口（性质 h/i、live 对拍、逐会话取证），生产链四个函数体的源码断言禁调。口径升级对外可见（原 ADR-022 §5，现无条件生效）：改了又改回不再触发 regen；加了又删不再留墓碑行；删了又建判净修改；逐会话明细退出主口径。回退手段是 `git revert` 单路径提交，不是解开配置键。T11b 改净臂单跑；执行层双臂 A/B 退役为历史证据（2026-08-13 两轮全绿仍在档）。T18 release 完整收集按协议入档（记录项，非门）：testbed 8000 高复触窗 warm median 10ms vs 53ms ≈5.3×，Add 地板窗 128ms vs 908ms ≈7.1×；SYST 250206 列为上线后现场复测。

- 两条启动检查 live 用例（`increment_manager::tests`，live 8019），钉住「已解析的库旁边新增一个库」这个现场形状。此前只有 08-17 那条单库用例，够不着的正是下面两件事：
  - `live_startup_sweep_routes_a_new_db_to_baseline_beside_an_applied_one`——**两条路由在同一份清单里不串味**。存量库（默认 8000，`applied=file_latest` 且 pe 有数据支撑）与从未解析的新库（默认 7998）放进同一个一次性监控目录：一轮启动重扫里存量库一行都不排（`discover_batch` 的水位早退），新库排 `apply_window` 窗口 `1..=file_latest`、worker 走 `needs_initial_load` → `initialize_dbnum_baseline`、回执含「首次按需初始化完成」，收尾断言存量库水位一格没动。存量库那侧的两个前提（追平、有支撑）写成断言而不是注释——差一个会话是普通增量、pe 零行是幽灵水位，两种都会入队，「一行都不排」也就无从断言。库号可配（`AIOS_STARTUP_APPLIED_DBNUM` / `AIOS_STARTUP_NEW_DBNUM`），默认走秒级的 7998，真靶按现场口径设 7999。
  - `live_scope_refresh_baselines_a_db_the_mdb_just_declared`——**MDB 才是增量范围的定义**。库文件全程躺在监控目录里，变的只有 MDB：把目标库从当前 MDB 的 `CURD` 里摘掉 → 重扫一行都不排，而且**连观察值都不写**（范围门排在 `record_observation` 之前，这条断言正是它的证据）；装回 CURD → `resweep_for_scope_change` 的 `scope-refresh` 重扫把它发现出来，照样走首次导入基线。复刻的是生产里「有人往 MDB 里加一个库」——那些刚进范围的设计库自己没有任何文件变更事件，不重扫就得等下次重启。夹具只动 `CURD`（`DBLS` 不碰，摘除态仍是「库存在、只是本期不在成员名单里」的合法现场），原样 CURD 存进 `queue_control:test_mdb_curd_backup` 再按原样写回；主体断言包在 `isolate_panic` 壳里，保证无论红绿都先扶正 MDB，用例开头另无条件还原一次以自愈上一轮的中断。
  夹具助手一并抽出共用（`locate_watched_db` / `isolated_watch_dir` / `queued_row` / `restore_registered_path`）：隔离目录这一手不是图省事——沙箱里躺着二十多个范围内却从未解析的 DESI 与一批 CATA，整面重扫会把它们一起排进多相位清单，而相位屏障要靠生产 worker 的重扫循环才走得完，`drain_queue_until_empty` 单独消化不了。两条各跑两个靶、**2026-08-18 四轮全通过**（默认 7998 靶 13.13s / 13.23s，现场口径 7999 靶 75.86s / 72.37s，窗口 `1..=120`，@8019），跑后实测沙箱完全还原（CURD 71 项、备份行 0、7999 水位回到 120 且 pe 有支撑、8000 停在 209 未动），台账已同步。

### 修复

- core.dll 净窗口的第二层“候选元素两端属性/成员 diff”收敛为单一实现：将
  `diff_ele_data` 提取到本地 `old-pdms-io`，legacy 会话回放与生产净窗口共同调用，
  删除 `net_window.rs` 的复刻分支并保留 re-export 兼容原符号路径。共享纯函数覆盖
  普通属性、显式属性、UDA（按 hash）和有序 children；vendor 新增 2 条纯单测，
  主仓净窗口 13 项、pipeline 48 项、会话夹具 20 项、两条真实文件对拍及 issue-019
  全链 `2 passed` 均通过。以后任一桶语义修正只改一处，不再产生净路径/回放漂移。

- core.dll `DB_Noun::primaryList` 门控从恒 `true` 改为权威快照驱动。通过已初始化
  E3D 3.1 进程直接调用 `db_get_element_info(noun_hash, 297853135)`，冻结 1931 个
  noun：1879 resolved（true 1142 / false 737）、52 unknown；resolved false 现在
  真正抑制 DB_UserChanges 成员事件，unknown 单列并保守为 true。快照钉住 core.dll
  SHA，采集脚本可复跑；B-EVT-03 同时覆盖 DAMP=true、TP=false、ROD=unknown 与
  `user_change_buckets` 实际调用。净窗口三态、children 两端持久化、模型 Regen 与
  公开 DTO 均未改变。

- 净窗口审核缺口闭环：退役键探测从 `DbOptionExtFields` 整体反序列化中拆出，独立读取原始 TOML，键值无论布尔/字符串/整数都报警；配置缺失、读取或语法错误显式报告，`AIOS_NET_WINDOW` 用 `var_os` 覆盖空值与非 Unicode 值，CLI/Python 接线断言收窄到函数体。Python 全链签名与 T11b 改用已跟踪 issue-019 baseline@24/final@26 作为固定真值，严格钉住 `changed_elements=3`、会话 `[25,26]`、水位 26、a/m/d=`0/1/2`、精确两个墓碑与活行恰减 2；移除运行时同源 oracle 和可变切点。正常档 `2 passed`，强制空跑在起点活行断言准确变红，清变量立即复跑通过，原 db8000 SHA 无损恢复。固定窗口还揭示 preview 会漏掉“冻结页存在但净操作为空”的会话，现改为从冻结会话页清单预建 `sessions[]` 并补纯单测。live 台账、ADR-031 回执、Web API 规格与证据同步。

- `rebuild_dbnum_info_from_pe` 把整个库的 pe 行拉回客户端再在 Rust 里按 Ref0 分组（`SELECT record::id(id) AS key, sesno FROM pe WHERE dbnum = N`），目录库量级直接把 ws 连接打死：2026-08-18 现场 ams7351 有 3,345,853 行，语句吊了 9 分钟后 router 任务连同 channel 一起没了，报 `read PE stats dbnum=7351 failed: Internal error: receiving from an empty and closed channel`，把前面那趟 **2.6 小时**的全量解析整个作废（数据其实已经全部落库、统计也由事件维护到位，死在的是这次纯多余的回读），批次 failed 后按相位门连坐把 catalogue 之后的 design 相位一起堵死。两处一起改：① 聚合下沉到服务端 `GROUP BY ref0`，返回行数等于 Ref0 个数（个位数），传输量与内存与库大小无关——SQL 抽成 `pe_stat_groups_sql()` 单一事实来源供测试与生产共用；② `sync_total_async_threaded` 结尾不再无条件重算——这条路径**不摘事件**（摘事件的是 `sync_pdms` / `sync_sys_only`），统计一路由 CREATE 分支维护到位，先用两条索引支撑的便宜计数问一句「对不对得上」（`classify_stats_settlement`），对得上就只补事件写不出的身份字段（`UPDATE`，无 DELETE，事件那条行原地留着），对不上才付全量重算的代价。回归测试三条：`stats_rebuild_aggregates_server_side_one_row_per_ref0`（两个 Ref0 五行 pe，聚合必须返回 2 行；退回逐行回读即报 `missing field 'ref0'`）、`stamping_identity_keeps_event_maintained_stats_in_place`（靠事件写的 `updated_at` 区分「原地补写」与「删了重建」；退回无条件重算即红）、`absent_or_mismatched_stats_still_pay_for_a_full_rebuild`（统计整体缺席时两侧和同为 0，不许被认成「对得上」）。三条都实测过回退变红。

- 初始化批次让位模型相位时，`defer_model_phase` 分支无条件打「初始化数据与水位已收口」，**失败批次照打**：2026-08-18 现场 ams7351 数据批次 failed，日志里却先宣告收口、下一行才是失败记账，排查只能绕到 `/api/v1/tasks/<id>` 回执里才拿到真错。改为按 `applied` 分叉，没推上水位的批次在同一行说清楚「未收口、模型工作不领取、原因见回执」。

- `pe` 表统计事件 `update_dbnum_event` 的 DELETE 分支在统计行缺席时把整条删除语句打死：写法是 `UPSERT type::thing('dbnum_info_table', $ref_0) MERGE { count: count - 1, ... } WHERE count > 0`，而 UPSERT 在目标行不存在时走的是**创建**路径，`WHERE` 拦不住，`NONE - 1` 当场报 `Cannot perform subtraction with 'NONE' and '1'`。统计行缺席是常态而非异常——`sync_pdms` / `sync_sys_only` 为了性能先 `REMOVE EVENT` 再写 pe，那批行天生没有统计行（2026-08-18 现场：ams5100 有 236 条 pe、零条统计行）——于是 `fast_delete` 的 Ref0 range DELETE 必炸，首次按需初始化与整库重建都过不去，批次 failed 后还按相位门连坐阻断同相位其余库（现场 dbnum=5100 卡死 meta 相位，design 相位的增量窗口排在后面永远轮不到）。改为 `UPDATE ... count?:0 - 1`：`?:0` 对齐 CREATE 分支 2026-08-06 审计定下的 NONE 免疫惯例，`UPDATE` 在 SurrealDB 2.x 只改已存在的行、缺行空转——不能让删除事件凭空造一条统计行，那条 MERGE 压根不写 `dbnum`，造出来的行连 `DELETE dbnum_info_table WHERE dbnum = N` 都清不掉；缺席的统计交给 `rebuild_dbnum_info_from_pe` 重算。事件 SQL 抽成 `dbnum_event_sql()` 单一事实来源供测试与装载共用，回归测试 `deleting_pe_rows_without_a_stats_row_neither_fails_nor_fabricates_one` 走 mem 引擎复刻现场形状（事件装载**前**写 pe，再删），已实测：回退成旧写法即报同一个 `TrySub("NONE", "1")`。

- 净窗口收集（ADR-022）在一切真实库文件上必现失败：cea58087（08-14）把三种真实文件常态升成了整窗硬错误，回退为「跳过 + 记账」并把回归钉进单测。三处分别是——① 索引树下降时**子页读不动**、② **子页层级不低于父层级**：这两种都是回收页残留的形状，生产点查 `filter_index_data` 对它们同样静默跳过，点查到不了的分支本就不参与触达集，跳过整枝不损完整性，现在计入 `unreadable_child_pages` / `level_anomalies`（索引**根页**读不动仍是硬错误，那是证明不了完整性）；③ **终稿记录解析失败**：全窗 1..=230 实测 64 条字典缺项的系统记录必现（首例 `16192_1` 报 `MNUM not exist in attr_info_map`），而回放路径对同一批记录同样以 `None` 操作落空、从未入库，硬失败等于每个含系统段的窗口整批打死，现在跳过 + `unparseable_finals` 计数 + **聚合**警告（逐条刷屏会把回执淹掉）。三处容忍都不许静默：形状进 stats、明细随回执透出。单测 `level_regressions_and_routing_anomalies_are_counted_and_flags_stay_blind`、`unreadable_child_pages_are_skipped_with_a_count_and_a_bad_root_is_fatal`、`an_unparseable_final_is_skipped_counted_and_aggregated` 各自带回归背景注释，改回硬错误即红。ADR-022 与 specs/003（Edge Cases + FR-8）同步修订并附修订说明。两条 ams8000 纯文件 live 用例复验通过、台账已同步。

- 增量更新审核（2026-08-18）五项收口。① watcher 重扫把被 `--debug-dbnum` 圈掉的库混进「不在 MDB 声明名单」聚合——对它们那句是**事实性错误**（在名单里，只是被调试限定圈掉，范围判定本轮根本没问过），正是 issue #10 的嗓音混同：D7 护栏一只钉了 `skip_reason` 的两种说法无交集，没钉 sweep 真的走它（`skip_reason` 在生产路径上无人调用）。现分成两个聚合桶各说各话（保留聚合防刷屏），调试桶点名 `--debug-dbnum` 并自证「不是 MDB 范围判定」，分桶判定复用 `debug_scope_admits` 与 `skip_reason` 同序（调试门在前），源码形状回归 `the_sweep_keeps_debug_exclusions_out_of_the_scope_bucket` 钉住。② 净窗口两种容忍形状 `unreadable_child_pages` / `level_anomalies` 只进 stats，而 stats 在 `collect_window` 拼完口径 warning 后即被丢弃——批次回执上「跳过整枝」实际静默，只有 python 探针的 `to_json` 能看见，违 spec-003 FR-8「任何一种容忍都不许静默」。现随口径标注透出 `不可读子页(t/b)` / `层级异常(t/b)`（t/b = target/base，与台账叙述口径一致），口径标注抽成 `net_caliber_warning` 纯函数，单测 `the_net_caliber_warning_carries_the_tolerated_shape_counts` 钉住，从 warning 里删掉计数即红。③ `debug_scope::trace` 的载荷改为闭包惰性构造：json! 实参是急切求值的，此前八个追踪点在未启用时也逐次构造 JSON（扫描点每轮全面重扫逐文件走到），调度器入队点还无条件取 `InitializationCoordinator` 的 allows+snapshot 两把锁——与「未启用零成本」的自述不符；闭包在 debug_scope 锁外执行以免锁序问题，惰性由 `tracing_is_inert_until_the_switch_is_set` / `tracing_ignores_dbnums_outside_the_debug_scope` 用会 panic 的载荷闭包钉住。④ `mode_notice` 说「其余 DESI 一律跳过」但豁免名单是 COLD_START_DB_TYPES（SYST/DICT/GLB/GLOB），CATA 一样被圈掉——措辞改为如实点名「DESI、CATA 等非 SYS meta」（行为未变：调试限定本就只该放行 SYS meta，CATA 是 ADR-025 正式数据阶段，要不要豁免属设计裁决，本轮不动）。⑤ `aios-database trace` 子命令写死 `curl.exe`，在 CentOS 7 部署目标上必挂——按平台 cfg 选 `curl`/`curl.exe`。

## 2026-08-17

### 新增

- live 用例 `live_startup_sweep_baselines_a_never_parsed_db`（`increment_manager::tests`，live 8019）：钉住 ADR-023 §4 生产缺省形状——范围内**从未解析**的库（无水位行、无统计行、无 pe 行，`delete_dbnum_fast` DropRow 制造，对应「新库文件第一次进入监控目录」）被启动重扫自动发现入队（上弦后 queued 不挂起、窗口 `1..=file_latest`），worker 冻结点 `needs_initial_load(0, latest)` 路由进 `initialize_dbnum_baseline`，终态 succeeded 且回执含「首次按需初始化完成」，水位推到 `file_latest`、pe 有数据支撑。与既有幽灵水位用例的分界：那条留着撒谎的登记行，这条连登记行都没有。watcher 指向只含目标库副本的一次性目录以收窄清单（全目录重扫会把沙箱里其它未解析库一并入队，多相位清单要靠生产 worker 的相位重扫循环才能走完，`drain_queue_until_empty` 单独消化不了——首轮红跑实测确认）；结尾以正本路径补一次扫描裁决，`PathMigrated` 自动迁移还原登记路径。**2026-08-17 通过**（测试体 10.0s），证据 `docs/evidence/2026-08-17-never-parsed-auto-baseline-live.md`，台账已同步。

### 修复

- db7999 设备九场景夹具首次全绿（9/9 PASS，四平面断言全过，隔离副本 `test-increment`）。此前两轮全红于 `saved session N is absent from data task merged_sesnos`，`--debug-dbnum 7999` 追踪定位为**夹具缺陷而非引擎回归**：`execute_fixture_and_wait` 在 SAVEWORK 之后轮询 `POST /update/preview` 等窗口张开，而预览唯一的写操作 `record_observation` 会把 merged_sesnos 冻结基线推到本窗口右端（红轮 trace 里全部 19 条入队记录 frozen_prev==右端），并入名单按规格恒空。就绪门改为本地读镜像文件头（`file_latest_sesno`，镜像拷贝本就同步、无需轮询），门票与预期会话号落盘 `execute-gate.json`；删除只剩这一个调用方的 `find_observed_window`。源码形状回归测试 `the_execute_gate_reads_the_file_header_not_preview` 钉住「就绪门不得咨询 preview」。红轮诊断与绿轮证据：`test-increment/runs/fixture7999-20260817-{145932-trace,154724-gatefix}/`，台账已同步。
- 初始化执行过程审核（ADR-025 链路）四项收口：
  1. F6 重扫读不出 DESI 最新会话号时此前只 warn 就跳过——清单缺着这个库照样宣告 `data_ready`、模型门照开，库持续读不动时外面毫无痕迹（DICT/CATA 头不可读却是阻断 Meta 的，同一种「观察不完整」两副面孔）。现在读失败记进对应阶段 blockers，该阶段保持可见地不就绪，共享盘瞬态靠周期对账重扫（默认 300s）恢复即解。源码钉 `sweep_skips_always_leave_a_phase_blocker`。
  2. 批次终态阻断数据阶段的判据从任务标签改为数据窗口本身（`batch_failure_blocks_data_phase`）：数据 Applied 而模型/副作用失败的 Partial 不再 `mark_failed`——那些失败在 durable pending 的重试账与死信门槛里，再关数据门只会让同阶段其余库连坐一个对账周期；数据批次 Failed 折成的 Partial（有单元成功）照旧阻断。单测钉 `only_an_unsettled_data_window_blocks_the_data_phase`。
  3. 新增数据批次连败账本（进程内，`/health` 的 `batch_failures`）：确定性失败此前会被周期对账重扫以每 300s 一次的节奏无上限重跑。同 dbnum 同右端连败到 `MAX_ATTEMPTS` 后重扫停止自动重跑（park，记阶段 blocker 可见），文件长出新会话或人工执行（POST /update/execute）清零复活，成功即清零；panic 路径记同一本账。单测钉 park/复活/重数三条出路。
  4. 启动主线等待模型阶段收敛时每 60s 播报一次仍在等什么（收敛在空闲轮里，任一环失败按 30s 退避，此前主线干等像挂死）；worker 里与 `run_cli` 重复的队列暂停恢复播报静默化（独立入口兜底保留，失败仍出声）。并入名单的基线（上一次扫描观察值）过去由 worker 执行体到冻结点再现读，而入队扫描早已把 `file_latest_sesno` 推到本窗口右端，「相对预览新增合并的会话」于是永远算不出来（2026-08-16 pipeline-f5 现场，`restore-execute-receipt.json` 里 `merged_sesnos: []`）。改为在入队时冻结基线并随队列行传递：发现方（execute 端点 / F6 sweep）在 `record_observation` 覆盖之前从裁决取 `previous_file_latest_sesno`，经 `DiscoveredBatch → DataBatch → FrozenBatch` 一路带到 `execute_one_dbnum`；同 dbnum 排队合并时基线只认最早那一次观察（取最小值）。回归测试三层钉住：批次队列纯规则（`the_baseline_freezes_at_the_earliest_observation`）、调度器冻结快照（`the_frozen_job_carries_the_earliest_enqueue_baseline`）、执行体源码断言（`the_merge_baseline_is_frozen_at_enqueue_not_reread_at_execution`，执行体再出现 `previous_file_latest_sesno` 即红）。

## 2026-08-16

### 新增

- 扫掠体自建网格器 `src/fast_model/sweep_mesh.rs`（WP-C 的 C0/C1/C2/C3 内核）：截面 → 2D 闭合环 → 三角网格，三支分派直接用 `SweepSolid::do_solid_segments()`（Core3D `DB_Gensec` 的权威判定，本模块不另立一套）。截面语义不重写——倒角与弧段复用 aios-core `wire::gen_polyline_original`（OCC 路径用的同一个函数），弧转折线用 cavalier_contours 的 `arcs_to_approx_lines`，端盖用 earcutr 做带孔多边形三角剖分（结构截面 L/C/I 是凹的，扇形三角化会填掉凹口）。摆放变换逐行对应 `gen_occ_spro_wire` / `gen_occ_sann_wire`，斜切端面沿用 `get_face_mat4`。360° SANN 按外环加内孔一次成形，两段半圆弧拼（`bulge = tan(θ/4)` 在 360° 处发散，单段圆表达不出来），另有一条测试与两个半环之和对拍以满足 FR-006。**尚未接进 `tessellate_libgm_param`**：生产接线要等直墙/斜切墙/弧墙的 RVM 门，本轮只到纯函数验证。
- `src/fast_model/mesh_assert.rs`：网格体检断言抽成 test-only 共享模块，`mesh_primitives` 与 `sweep_mesh` 共用同一套判据。
- 依赖新增 `cavalier_contours`（与 aios-core 同一 gitee fork）与 `earcutr`，两者本就在 `Cargo.lock` 里，只多了两条依赖边。
- `mesh_primitives` 单测从「非空」升级为「可用于布尔的实体」：结构体检统一走 `assert_solid_mesh`（法线齐备且是单位向量、无零面积三角、随附 AABB 与顶点一致、顶点焊接后每条有向边正反各一次即闭合可定向、散度定理算出的有向体积为正即三角朝外），再逐个原语与解析包围盒和解析体积对拍（球冠 `πh²(3R−h)/3`、半椭球 `⅔πr²h`、圆环 `2π²Rr²`、棱台 prismatoid 公式等）。7 种原语共 22 条。

### 修复

- `gen_elliptical_dish` 母线参数写反：代码用了 `x=r·cos t, z=h(1−sin t)`，与自身注释相反，生成的碟上下颠倒——底圈半径落在 z=height，z=0 处只有一个点，而底面圆盘仍按半径 r 铺，网格既破洞又带零面积三角。改回 `x=r·sin t, z=h·cos t`，随之修正法线、三角绕向（母线自下而上，绕向与球体相反）与顶点处的退化三角。
- `gen_rectangular_torus` 侧面法线全是 `Vec3::ZERO`（注释写着「后面按面片计算」但没有下文），着色会全黑。改为四个侧面各自持有顶点，法线按面给，硬边不再被平均。
- 两个环面的端面 cap 绕向与外法线反了：起始端面本该朝 −φ、末端朝 +φ，实现把两者对调，切出来的扇环端盖朝内。同时补上负角度扫掠——`CTorus::check_valid` 只要求 `angle.abs() > 0`，负角沿 −φ 扫掠会让整体内外翻转，现在按扫掠方向翻转绕向。
- `gen_pyramid` 顶面退化成一条棱（`xtop=0` 而 `ytop>0` 的楔形）时会留下零面积三角和破洞：改为四边形统一出三角、逐个丢弃退化面，点退化与线退化走同一条路径；亚微米级边长先归零，避免留下狭长三角。
- `tessellate_libgm_param` 各分支加 `covered()` 收口：`check_valid()` 放行但仍然出空网格时报错而不是返回 `Ok(Some(空))`——空网格传下去只会表现为「模型悄悄少了一件」。

- ADR-028 抽取树收尾三件：`pe_owner` 批写入补上 `INSERT RELATION IGNORE`（边 id 显式，父层补缺重放叶子已写过的边必须幂等，否则重复 id 会把整次补缺同步打成失败）；`collapse_extract_families` 输出按（项目, 库号）排序钉住跨进程扫描序（原 HashMap 迭代序随机）；F6 重扫给「抽取树父层被叶子代表」补日志、给「文件名库号与文件头不一致」单独文案（原先与多副本共用一句「多个抽取/副本」，单文件 mismatch 时误导）。
- ADR-028 父层补缺静默空转：`collect_project_db_files` 归并抽取家族时会把被叶子 shadow 的主库从解析清单里删掉，而基线的父层补缺（`included_db_files` 点名主库）恰恰要解析它——补缺同步一个文件都不解析却返回 Ok，缺口留在库里。现在被 `included_db_files` 显式点名的 shadow 主库回到清单；补缺同步返回值里若没有目标 dbnum 直接报错、不推进水位。附回归测试 `collect_project_db_files_keeps_explicitly_named_shadowed_master`。（同批核实：pe/属性批写入本就是 `INSERT IGNORE`，父层同步天然只补叶子缺号，不会用父层旧会话覆盖共享 refno。）
- 订正三处与实现相反的默认值注释：`startup_autorun` 与 `room_incremental` 的缺省均已是 `true`（分别在 2026-08-14 与 2026-08-12 翻正），但 `options.rs` 的字段注释与 `/health` 的两条字段注释仍写着「默认 false」。同批把 `startup_autorun` 关闭时的机制描述改准：它挂起的是重扫排出的队列行与空闲轮的持久积压，不是「队列消费者启动即暂停」——那是另一道跨重启保留的暂停闸门。

## 2026-08-15

### 新增

- ADR-029：设计/目录负体布尔改走本地 `manifold-csg`（path `../manifold-csg`），不再调用 aios-core 的 `ManifoldRust` / 旧 `manifold-sys`。OCC 只保留三角化（扫掠体尚无替代）；OCC 布尔不进生产。
- ADR-030 据 Core3D.dll IDA 修订：三角化权威是 libgm `gm_Create*`（挤出/旋转/ruled + 目录原语），不是 OCC BRep；OCC 只是翻译层。
- `specs/009-retire-occ/plan.md` / `tasks.md`：按 libgm 符号拆工作包与带路径任务（B 目录原语 / C 扫掠三支 / D CSG / E 离散）。
- `manifold_tessellate`：单位箱/柱与挤出轮廓走 manifold-csg（对齐 `gm_CreateBox/Cylinder/Extrusion`）；空挤出 hard fail。aios-core 的 `gen_model` 不再捆绑旧 `manifold-sys`，避免与 manifold-csg 链接冲突（需本地 vendor patch）。
- T005/T007：`gen_inst_meshes` 无 `occ`/`manifold` 时 `bail!`（禁止静默跳过）；箱/柱/挤出先 `tessellate_libgm_param`，斜端柱回退 OCC；libgm 路径 AABB/`pts` 取网格八角。

### 修复

- OCC 布尔若切出空网格，不再覆盖已有 `booled_id` 文件。`116569` 的空 60 字节结果已用 manifold 重切恢复（p95=137）。

## 2026-08-14

### 新增

- 抽取树叠加（ADR-028）：同项目主库与唯一 `_NNNN` 归并为一个逻辑库，叶子为水位权威、父层只补缺号；兄弟抽取与人手副本仍 Duplicate。
- Python 解析层暴露 `parse.collapse_extract_files` / `parse.parent_gap_refno_count`；`python/tests/test_extract_tree_offline.py` 离线钉住归并，本机 AMS 7355 实文件再钉头/会话/parent_only=0。
- 新增 ADR-024 与 `specs/005-shape-save-coalescing`，定义模型实例保存的有界合批、先计划后修改、确定性分包和成功后计入产出约束。
- 新增 ADR-026：扫掠体公开步骤按 `DB_Gensec` 蛇形命名；可复用直线无斜切时单位网格身份只键目录截面。
- Python RVM AABB 对拍：`python/scripts/rvm_aabb_compare.py` 先打 AMS 1112 CWALL `/1RS-WF03-W-C-RR001` 的 WALL/STWALL（SweepSolid），口径对齐 `rvm_gate_c_or_1r345_c.ps1`（3mm / 3%）。
- Mesh 级对拍：`fast_model::shared::two_sided_surface_distance` 双向采样表面距离（mean/rms/p95/Hausdorff，parry3d，无新依赖、进 CI），三角化无关；`rvm_baseline::mesh_compare` 用 rvm-rs `Tessellate` 取 RVM 世界三角、gen 侧 `inst_geo.param` 就地 `gen_occ_shape`+`gen_occ_mesh`（param 为空的布尔/复合几何回退磁盘 `.mesh`）取世界三角。对 1112 CWALL 4 堵 WALL 与 8000 C-OR 管系（FTUB/BEND）做 mesh 对拍（live 8009+occ）：墙与 FTUB 几何忠实；BEND 逐元素多约 100mm，但 BEND+相邻 FTUB 的 union 与 E3D 只差 5.8mm——是元素边界拆分口径不同、装配无害。gen 几何经 mesh 级验证在装配层正确。
- Mesh 对拍扩到 1112 直线 STWALL（双向 ≤0.06mm）与 8000 `/C-IY-1R330-B`（ACP1000 槽盒，35 构件 union 的 gen→rvm p95=4.14 / max=24.9mm）；E3D 槽体外壳比 gen 管段大约 100mm 高，rvm→gen 大是范围差。
- Mesh 对拍 `gen_world_mesh` 与生产 `query_valid_insts` 对齐：有 `booled_id` 时加载切洞后网格，不再回退未开洞正挤出。1112 三堵大体量 GWALL 重跑 NXTR 布尔后 gen→rvm p95 从 870/786/591 降到 0.1/9.3/137。

### 修复

- 管道 FTUB 续测关闭三处增量缺口：staged 初始化尾事务保留未执行的 `RegenRoot`，模型 drain 优先最新真实保存而非历史更新时间，Add 复用旧世代 Refno 时先清理全部旧 owner/children 边；F5 同步换成当前文件可达的 FTUB 夹具并增加工程控制库与依赖 Refno 前置检查。
- 管道增量续测修复两处可重放缺陷：L3 变更宏现在识别带保存注释的 `SAVEWORK`，TTY 夹具显式传递目标 DB 与项目并按会话事实分类；水位仍为 0 的中断基线会先清除未提交 PE 再全量解析，避免 `INSERT IGNORE` 永久保留陈旧行。
- 模型实例纯数据写入的源码顺序守卫改为跟随当前 SavePlan 入口，并统一 CRLF/LF 后再切片；Windows 与 CI 现在验证同一段生产保存路径。
- 四份 e2e 探针（`staged_pane_replay_probe` / `staged_regen_e2e` / `staged_transform_e2e` / `issue7_e2e_increment`）的 `DiscoveredBatch` 补上 `phase`/`epoch_id`，`Run-LiveBatch.ps1` 的 `cargo build --lib --tests` 预编译不再被挡住。
- 模型持久工作改为每页至多 16 根、逐根执行，并在认领前及每根前后检查初始化 epoch、模型门和数据队列；数据到达时以 `model_drain/yielded` 收口，未执行行不改变状态、错误或重试次数，健康状态补充让位原因、耗时和 attempts 变化量。
- E3D 变更宏按 DONE、ALIVE、退出码及保存前后会话号分类；已保存但未确认的运行继续验证而不重放 apply，只有已知的保存前启动失败允许一次重试。L3 夹具统一采用 `preview → SAVEWORK → execute`，并验证保存会话进入 `merged_sesnos`。
- L3 的 Plant UI 运行改用隔离设置文件和仓库根工作目录；自动化按 `tree_item + refno` 定位，数据批次后等待关联 `model_drain` 与 pending 收敛，不再把数据任务的 `units` 当作模型完成证据。

- `inst_relate` 世界包围盒对 `PrimLoft` 圆弧扫掠按环扇取样（含世界轴交叉），不再把局部 AABB 当盒子做 8 角变换。
- 扫掠体斜切改为相对该段切向的垂直/平行抑制（`1e-6`），不再用世界 `±Z`；BANG/PLAX/镜像/路径方向进实例变换。目录路径组合 `get_trans` 旋转，SavePlan loft 夹具改用非 Z 切向方切。

- 合并生成器小尾批的重复元数据查询与 SQL/journal 写入；NaN 和持久化 ID 冲突在删除旧模型前响亮失败。
- 启动、回退重建及同轮稳态更新统一为 `SYS/DICT → CATA → DESI → 模型 → 房间`：
  完整清单按 epoch 安装，Watcher 事件只触发防抖全量重扫；早期阶段失败、身份阻断
  或目录不可读会关闭后续数据与模型门。队列、健康状态和手动回执新增阶段/epoch/
  blocker/shadowed 观测字段。
- 新增 `catalogue_project_priority`，跨项目 DICT/CATA 同 dbnum 由显式项目顺序选主；
  同项目重复、未知配置或无优先级候选整阶段阻断，被遮蔽文件不写 observation 与水位。
- 全量 `sync_pdms` 改为全局 Meta、Catalogue、Design 三次 await；启动提前开放 Web 与
  data-only worker，最后一个 DESI 水位完成后才执行全量模型、持久模型工作、AABB 与房间。
- 启动重扫现在会在应用水位恰好追平文件时校验数据支撑：`pe` 零行且没有匹配
  空基线凭据的库按首次导入入队，由 worker 重建基线；合法空库凭据与水位同事务
  收口。生产缺省 `startup_autorun` 同步翻为 `true`，未解析库和异常水位库无需再等
  下一次文件保存触发。
- 增量正确性阻断：数据队列新增 `apply_window` / `reinitialize` 显式意图，回退到
  零会话也会以 `0..=0` 到达 worker；排队重建意图占优、运行中保留后继，冻结点
  复核仍判 Rollback 才清库。空文件清库后直接 Applied 且水位保持 0。
- `fast_delete` 的统计/持久队列、spatial epoch 与水位清零纳入同一显式事务；
  关系和 Ref0 区间删除继续独立幂等，水位更新保持事务末句。
- 幽灵水位清库会从 `dbnum_info_table` 的 record id 恢复真实 Ref0，并与 PE 前缀
  取并集后删除对应区间；不再把 dbnum 数值冒充 Ref0，PE 零行时也能清理派生行。
- 净窗口把不可读子页、层级不下降、终稿解析失败和 last-touch 缺失升级为整窗
  错误，杜绝残缺触达集落库推进水位；Modified 基版本失败仍保守降级为 Add。
- `ref_rev_maintain` 补偿载荷改为非空、全量严格解析，任一非法 refno 均进入失败
  记账并保留队列行，不再以空修复调用静默销账。
- 收集接口统一为 `CollectedWindow`，把冻结窗口的实际会话页清单与操作流、口径和
  warnings 一起贯穿预收集、崩溃重放及成功回执；空保存、自抵消和稀疏会话现在
  正确进入 `merged_sesnos` 及平行保存时刻。Replay 清单与操作共用一次文件打开，
  两种模式首条 warning 固定自报口径，且后续计划失败也会保留该 warning。
- 基线入口开始硬消费共享 `ScanGate`：身份阻断和回退重建均在计数/解析/水位前退出；
  范围外 CATA 改为跨 scope 收齐全部候选后复用 watcher 的同项目 dbnum 判重，重复组
  零 observation、撤销旧 locator 路径，并在预览/入队回执列出全部路径。

## 2026-08-13

### 新增

- **会话索引差分：db 文件 sesno 窗口净增删改秒级判定**
  （`data_interface/session_index_diff` + `aios_db.parse.net_changes` +
  `python/testbed/net_changes_probe.py`）。每个会话页都带当时的索引根
  （copy-on-write B-tree），取窗口两端的根做双根差分：目标树只下降「页号 >
  base 会话末页」的新页、共享子树整枝剪掉，base 树按共享根集合剪——IO 正比于
  变更量，与窗口内会话数解耦。判定**纯文件**：不查库、不逐会话解析记录，窗口
  由调用方显式给定（源码断言钉死零 `SUL_DB`）。存在性口径与生产 B+ 点查逐字
  对齐，三条规则均由真实 ams8000 实测逼出并钉成回归单测：同键子指针首见者胜
  （Save Work 重写子树留下的陈旧指针，跟进会捞出 1.9 万条已被发布抛弃的临时
  记录）、路由不看 flag（flag=0 的首见指针才是发布后的子树）、键范围路由
  （回收页残留条目键在本叶范围之外，点查不可达）。验收：模块纯单测 11 条 +
  `db8000_session_pairs` 性质 h（差分 ≡ 回放折叠，台账腿由性质 e 闭环）+
  Python 离线档 3 条（issue-019 夹具）+ live 对拍 4 窗口差分 ≡ 生产点查零分歧
  （全窗口 695ms vs 回放 10.8s，debug 15–34×）；探针 `--verify` 全量窗口审计
  154 条差异全部点查仲裁归因为回放旧口径盲区（漏报存在 67 / 孤儿腿误报 86 /
  误判 1）。证据 `docs/evidence/2026-08-13-session-index-diff-net-changes.md`，
  live 台账 D 组两条已登记。同日补测 amssys（SYST 8191，169 会话）：10.6×，
  回放折叠净集 **43%（818 条）与生产点查仲裁的两端状态不符**（孤儿 Deleted 腿
  653 为主）；点查是**同源判定基准**（列出/归类分歧，非独立证明全是旧口径盲区），
  删除判据的独立性另由 core.dll 键集差佐证。
- **ADR-022 + specs/003：增量窗口收集改用会话索引差分（净窗口；已接受，P0 已落地、默认 off 灰度中，核心机制层已由 live IDA 闭合，翻默认余下受验收 5 结果层门阻断）**。
  工具层对拍收口后的引擎采纳决策：执行体与预览的收集阶段由
  `collect_net_changes` 接管（逐会话回放退为诊断工具），输出形状兼容——每
  refno 恰一条 `EleOperationData`，净修改由 base/终稿两端版本**一次 diff**
  合成（属性差量 + children 两端 + old/new owner，diff 实现与回放同源单一
  权威）；下游模型计划 / ref_rev / MySQL / 渲染零改动。灰度开关
  `net_window_collection`（默认 off）+ `AIOS_NET_WINDOW`，预览与执行同谓词。
  四条明示行为变化：改了又改回不再 regen、加了又删不留墓碑行、删了又建判净
  修改、逐会话明细退出预览主口径。窗口起点仍由水位给出（ADR-001），回退/
  幽灵水位仍走 ADR-021 整库重建，跨库级联仍走 ref_rev（ADR-003）。
  - 实现落地（同日晚，P0 引擎接线）：`net_window::collect_net_window`（净三态 →
    同形状操作流合成；`diff_ele_data` 忠实复刻 vendor 内联 diff，九桶 + children
    两端 + noun）+ `IncrementPipeline::collect_window` 唯一派发点（预览、执行体、
    崩溃恢复重收集、worker 尾段重收集四处接入，源码断言禁直调回放）；
    `NetEntry::base_loc` 让净修改直读两端版本不付点查；净口径回执首条警告自报
    口径与计数。真实文件逼出第三条口径对齐：字典缺项系统记录（ams8000 全窗口
    64 条）终稿解析失败按回放同口径跳过 + 计数 + 聚合警告，不整批硬失败。
    验收：`db8000_session_pairs` 性质 i（净收集 Modified 负载与回放**逐桶相等**，
    全部案例窗口全绿·样本为各窗口实际 Modified 条目非 test binary 计数）+ live
    负载对拍（6,499 条 Add 渲染逐字符相等，全窗口净收集 1.24s vs
    回放 10.9s）+ lib 710 passed + 离线档 65 passed。已知偏差记 evidence
    「引擎接线」节（`merged_sesnos` 会话页清单口径留待翻默认值前落地）。
  - **live A/B 全链路执行（同日深夜，切默认值前的最后一道证据，已收口）**：
    `python/tests/test_net_window_ab.py`（房间增量档，opt-in
    `$env:AIOS_NET_AB='1'; .venv\Scripts\python.exe -m pytest
    tests/test_net_window_ab.py -q -s`，@8071 一次性内存库）。testbed 8000
    （基线 6,542 行）同一起点、同一窗口 105..=209（净三态 +6/-51/~16，其中原样
    重写 7），off/on 各走一遍完整执行（暂存窗口 + 窗口内生成 + 提交 + 水位收口）：
    **终态逐维等价**——水位 / 共同活行 6,543（逐字段）/ noun 属性表 / pe_owner
    6,542 边 / pending / dbnum_info 记账恒等式全部相等；仅有的偏差全部归因：
    净臂多持 2 个文件真值元素（回放连同旧基线的最终索引漏报，点查仲裁站净一边）、
    13 条 ref_rev 边为回放对 7 个原样重写元素的顺手重建（§5.1 家族，重置后空
    ref_rev 店放大）。窗口全链路耗时回放 35.0s vs 净 11.0s（3.2×），收集阶段
    差分自报 154ms。连续两轮全绿（各 3 分 16 秒）；全量绑定档 83 passed +
    1 skipped（36.4s）。证据同文件「live A/B 全链路执行」节。
  - **M1 正确性闭环（同日，T20 / T11b / T19 / T18a 落地；T13 阻塞未闭）**：
    - **T20 合成器纯单测**：`collect_net_window` 抽出纯合成内层
      `synthesize_net_window(net, resolve)`（`NetChangeSet` 按值接收、resolver 收窄成
      `FnMut(RecordLoc) -> Result<EleData>`、解析上下文错误文案留在合成器），**七条
      纯单测**覆盖三形状 + 基版本失败按新增 + 终稿失败跳过计数聚合 + `base_loc`
      缺失硬失败 + 原样重写计数（原样重写**不是降级**，是正常判定的正常结果）。
      纯提取不伪称先红：安全网是性质 i + 既有 live 对拍，新测试有效性由**逐分支变异
      抽检**证明（5 处准确红，变异代码不入库）。`net_window` lib 13 passed / 0 failed /
      1 ignored（ignored 是需真实 ams8000 的 live，**本轮未跑**）+ `db8000_session_pairs`
      集成目标 20 passed（含性质 i，是用例数不是覆盖窗口数）+ Python 离线
      66 passed / 20 deselected。ADR-022 验收 1 就此满足。
    - **T11b 存量库删除等价直证**：补上「起点早于删除会话、库内确有活行」的形态——
      原 A/B 删除腿是空跑（被删元素在基线本就无行）。切点 K=24、窗口 25..=209，
      文件层净删除 oracle 4 条，起点确为活行且净口径**真立碑 2 条**
      （`24384_24778`/`24384_24779`，⊆ oracle），共同活行 6,536 逐字段一致、
      **0 未归因**，live 118s 全绿；`AIOS_T11B_FORCE_EMPTYRUN=1` 强制空跑变异准确变红。
      存量基线由 `python/tests/_session_snapshot.py`（`session_cut.rs` 的 Python 镜像，
      与 Rust `db_session_fixture inspect` 双向对拍）切 @K 得到；文件换入换出走**同卷
      临时文件 + fsync + `os.replace` 原子替换** + `pristine` 备份 + `finally` SHA 校验，
      收尾源文件 16,504,832 字节无损恢复。**删除判据是纯文件**（core.dll
      `elementsDeletedBetween` 键集差的复刻）；**DB 查询只验证窗口前活行与窗口后墓碑
      两个状态，不作删除判据**，也不用 `search_latest_refno` 点查自证。
    - **T19 qualifier 恢复对拍（非阻断，CLOSED）**：断言落 `db8000_session_pairs.rs`
      性质 i 的 Modified 分支，两臂 `qualified_changes` 逐项相等，集成绿，**未扩公开
      DTO**。强度如实标：当前 issue-019 夹具两案例都是删除、数组属性零变化，这条现在
      是 **empty == empty**，**不是 qualifier 语义已覆盖的证据**，价值只在防回归。
    - **T18a release 方向性单点测量（n=1，非性能门）**：高复触窗 104..=209（106 会话，
      a/d/m = 6/51/16，回放 `ops_total` 215，复触率 2.95）完整净收集 3ms vs 回放 53ms
      ≈ **17.7×**，该窗 raw 两臂发散 72 条全部归因回放旧口径盲区、点查零分歧；对照
      Add 地板窗 1..=209（复触率 1.05）126ms vs 792ms ≈ 6.3×（形态决定，不作判定）。
      **结论仅限**「在动机形状上 ADR-022 决策 4 不需修订」；T18 正式统计
      （1 warmup + ≥5 次 / median·min·p95 / warm 判定 cold 另报）与 **250206 SYST
      现场硬门仍未完成**。另：A/B probe 的 4.4× 已明确降级为「净差分 vs 回放完整收集
      的混层下界参考，非门证据」。
    - **T13 Added 夹具 BLOCKED（不得标完成）**：仓内**不存在**同时满足「Added > 0」
      且「raw 净集 == 回放折叠集」的真实窗口——带 Added 的窗口都伴随回放旧口径盲区，
      raw 两集不等，性质 h/i 指过去必红。须用受控 E3D 录 `scratch-create` 案例
      （新建 SITE/ZONE → 建元素 → Save Work，窗口内无删除无临时态）；**不得**为点亮
      它放宽 h/i 断言。**M1 Exit gate 因此仍未通过，M2（T17/T12/T18/T15）不得启动。**
  - **决策澄清（同日，评审后最小补写，不改决策主体）**：ADR-022 新增「算法来源
    与正确性边界」——会话索引差分**不是** core.dll `DB_DB::elementsChangedBetween`
    的复刻，而是 gen-model 吃 dabacon 追加式 CoW B+ 树形状推出来的加速。证据边界
    同时写死：core31-retrace 证据只显示其**外层语义**是元素 /（属性, qualifier）
    级的三阶段六桶差分、外层未见索引根双根页差分，但
    `attributesChangedBetween` / `elementsDeletedBetween` /
    `elementsInsertedBetween` 的页级实现**未逆向**，故**不断言**内核内部绝不触及
    索引根；core.dll 继续是属性/桶语义的唯一权威，本路径的索引差分不援引它作为
    算法来源。正确性契约写明：端点存在性以生产 B+ 点查可达性为 oracle、净三态
    由两端 leaf `(pgno, offset)` 集合差定义、净修改仍用两版本 `diff_ele_data` 对齐
    core.dll 语义，正确性靠三重对拍而非「复刻内核」。同时记两条机制层未闭合风险
    （叶 `flag` 的取值语义与取值全集未逆向、当前口径本就不依赖 flag；删除是移除
    leaf 还是墓碑 flag；`is_start_page` 只是索引条目起始哨兵行为、底层位定义未知
    ——三者均无 live IDA 证据，现有零分歧只证结果层，且**差分≡生产点查是同源**
    （二者都不看 flag），不能当 flag 机制的独立证明），并把翻
    `net_window_collection` 默认值的门写成验收 5：要么 (a) 在可达 core.dll/idb 上
    闭合机制；要么 (b) **显式接受机制层未闭合的残余风险**并补一份**结果层**样本——
    其独立 oracle 必须是**生产可见终态**（E3D/权威库侧对同一元素在删除/重建后的
    在场与否），而非同源的点查仲裁或带旧口径盲区的回放，样本覆盖已观察 flag 取值/
    删除重建/Added-Deleted-Modified 三态，走 (b) 机制层仍标未闭合。默认 off 的
    诊断与灰度不受此门阻断。
    **⚠ 本条的「机制层未闭合 / 无 live IDA 证据 / 只证结果层」口径已于同日晚被
    live IDA 逆向推翻，以下一条为准。**
  - **机制层闭合（同日晚，live IDA 逆向，推翻上条保守口径）**：
    `docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`
    （ida-bridge / idalib，core.dll 3.1，SHA `3c1f…417d`，符号系二进制自带 MSVC
    修饰名、非猜名）证实 core.dll 会话变更枚举（`DB_DB::elementsChanged /
    Deleted / InsertedBetween` → `DB_IndexTableCompare`，dabacon 比较引擎 opcode
    266/270，主索引表 `13387743`）**本就是双根 B+ 索引归并差分**——与 gen-model
    `session_index_diff` **同思想**（gen-model 是纯文件重实现 + 共享子树剪枝，
    非逐指令复刻内核代码）。三处旧「机制未闭合」悬案就此闭合：① 删除 = 键在旧根
    不在新根的**集差、非墓碑 flag**（kind=3）；② 变更检测**全链路**（页取 + begin +
    双根归并）**不读 / 不按 flag 过滤**（flag 在链路外是否另有可见性门未闭合，不写
    功能性否定，report §4.5/C3/C4）；③ `0x80000001` 是**页内键哨兵**（核内以
    `-2147483647` 作键边界识别）。**残留（不阻断翻 on，仅登记）**：`flag` 自身位
    编码（存在 / 偏移 / 位宽 / 枚举）与 **flag 在变更检测链路之外是否另有可见性 /
    过滤门**均未逆向（report C3/C4，有意收口）——可断言的只是「权威变更检测链路
    不以 flag 作门」，不写「flag 全无功能」。据此把 ADR-022 / spec / plan / tasks
    的翻默认门从「(a) 闭合机制 / (b) 接受残余风险」改写为**结果层门**（存量库删除
    A/B、Added 独立夹具、批次冻结快照、会话页清单、SYST 性能——性能门当前**未达**，
    debug 完整收集仅 8.8× / probe 4.4×）。qualifier 维：core.dll 变更粒度含
    `(attr, qualifier)`，gen-model `ModifiedElement` 按属性名聚合会丢 qualifier；
    这是回放与净路径**共享的既有形状限制**、切臂不新增，翻 on 不阻断但**非无条件
    安全**，待评估（tasks T19）。

- **ADR-021 + specs/002：水位必须有数据支撑，回退默认整库重建（去档位）**。
  ADR-001 的「失败不推进水位」管的是写的一侧；读的一侧有两个洞。其一，
  `needs_initial_load` 只问水位不问数据，「水位非零、`pe` 零行」被判成正常
  增量，从 `applied+1` 起重放，`1..applied` 静默缺失（看得见数据的
  `baseline_needs_full_parse` 在 `initialize_dbnum_baseline` 内部，够不着
  路由）。其二，文件回退默认只阻断等人，而阻断会静默消失——`file_latest`
  一旦涨回 `applied` 之上，被替换的那段差异永久丢失。
  - 现场实证：8009 上 dbnum 7350 / 7353 / 7741 的 `applied_sesno` 为
    208 / 101 / 94 而 `pe` 零行；同日 8 个库因文件在 08-12 19:04 被整批换成
    更旧副本而回退阻断。证据 `.scratch/realign-20260813-114321`。
  - 决策要点（评审决议 2026-08-13）：回退**默认整库重建**——扫描只分类入队
    重建批次，worker 冻结点复核仍判回退才 `wipe_dbnum_for_reinit`（整库清空 +
    水位行清值不删行 + 统计与队列残留清空 + spatial epoch 递增），随后按首次
    导入全量解析；`watermark_realign` 档位、`AIOS_WATERMARK_REALIGN`、
    `POST /dbnums/{dbnum}/realign` 端点与 `aios_db.sync.realign` 绑定全部
    移除。幽灵水位（`applied>0` 零数据）由 `needs_initial_load` 的数据支撑
    维度路由到基线（判据落在路由、不落入队门——空库会无限重解析）。
    `TypeChanged` / `Duplicate` / `Missing` / `ForeignProject` 照旧阻断。
  - 实现落地（同日）：`needs_initial_load` 增加数据支撑维度 +
    `dbnum_has_any_pe_row` 存在性探针（只在有增量窗口要跑时付一次，读失败上浮为
    批次 Failed）；`scan_and_check_file` 返回三态 `ScanGate`（放行/阻断/重建），
    sweep 与 watch 对回退构造 `reinit_batch`（applied=0 形状）入队；
    `fast_delete::wipe_dbnum_for_reinit`（与快删同源的三阶段删除，元数据阶段改为
    统计清空 + epoch 递增 + 水位清值不删行且置尾作提交点）；执行体
    `execute_one_dbnum` 冻结点复核仍判回退才清库，清库失败计 Failed 幂等重放；
    预览 `blocked`/`initialization_required` 与执行体同谓词。
    `FileAnomaly::auto_realignable` 更名 `requires_reinit`。拆除面：
    `WatermarkRealign` 档位与环境变量、`realign_rolled_back_dbnum` /
    `realign_dbnum_checked`、HTTP `POST /dbnums/{dbnum}/realign`、
    `aios_db.sync.realign` 绑定、`AiosClient.realign_dbnum`、
    `python/tests/test_watermark_realign.py`（由 `test_rollback_reinit.py` 接棒，
    见下）。
  - Python 闭环用例 `python/tests/test_rollback_reinit.py`（房间增量档，@8071
    一次性内存库）：走与服务同一台机器（`incr.execute_manual` 子集），模块级
    引导一次（SYS meta 解析撑起 MDB 范围 + 7998 首次基线），三条用例分别钉
    回退整库重建（幸存位/幽灵位标记行都必须物理消失）、幽灵水位路由到基线
    （行数回到完整基线，增量重放做不到）、类型变更照旧阻断（水位与数据纹丝
    不动）。conftest 导入期补 `RUST_MIN_STACK=16777216`（执行链在默认线程栈
    上溢出，与 testbed 脚本同一惯例）。全套 `pytest -q` 80 绿（36.5s）。
  - live 首跑抓出并修复一个真缺陷：增量形状（start_sesno>1）的批次先开 ADR-017
    暂存窗口、执行体改道基线后窗口缺 finalize plan 而 failed——`batch_worker`
    开窗前新增冻结点预判 `batch_reroutes_to_initial_load`（applied=0 / 回退 /
    幽灵水位一律不开窗），与执行体共用同一个数据支撑探针。
  - 验证：CI 口径受影响模块单测 155 绿；live
    `live_rollback_wipe_clears_the_dbnum_for_reinit`（4.7s @8019）与
    `live_rollback_and_ghost_watermark_reinit_end_to_end`（22.3s @8019，两幕）
    通过，台账与 `docs/evidence/2026-08-13-adr021-rollback-reinit-live.md` 留痕；
    Python 离线档 62 绿。
  - 「在水位行上记录来源（基线收口 / 增量收口 / info 表播种）」与
    「`applied_sesno_time` 交叉核验（停机窗口内回退又长回去）」记为后续项。

- **增量模型生成单元测试总纲**：重写
  `docs/2026-08-06_model-increment-unit-test-plan.md`，把 S0–S13、U1–U13、
  暂存窗口 I1–I9、房间 RI-1–RI-15、离线夹具与 live / E3D 边界收进同一入口；
  明确 P0/P1/P2 待补项、具体文件落点、“回退即红”条件、Constitution Check
  和分波次门禁。当前源码枚举快照为 765 项（82 ignored），`http_api` 为 776 项
  （82 ignored）；长期状态仍以源码枚举和 live 台账为准，不再复制漂移总数。

### 修复

- **`inst_geo` 几何参数双变体深合并毒化共享单位行（live A/B 抓出的真缺陷）**：
  `render_inst_geo_merge`（2026-08-13 `276aa5f6` 用 `UPSERT … MERGE` 替换
  `INSERT IGNORE`）忽略了「不同 `PdmsGeoParam` 变体可以合法共享同一记录 id」——
  普通 LCylinder 与非切角 SCylinder 的单位网格同为单位圆柱，
  `hash_unit_mesh_params` 按设计同返 `CYLINDER_GEO_HASH`，两个变体先后 MERGE 把
  `param` 深合并成 `{PrimLCylinder, PrimSCylinder}` 双键对象，enum 反序列化永久
  失败，**所有**引用该共享行的根从此生成不出来（A/B run4 实测：2,229 根批量重
  生成全灭 + 逐根重试全灭，`decode mesh parameters failed`）。改为
  `render_inst_geo_upsert`：`param` 整值 `SET` 覆盖——行缺失补齐、半成品修复、
  meshed/aabb/pts 派生字段保留，且对已被旧写法打坏的双键行**自愈**（下次参数
  刷新整值盖掉即恢复可解，2026-08-13 后跑过生成的持久店无需手工修）。回归：
  `a_variant_switch_on_a_shared_unit_row_replaces_param_wholesale`（回退 MERGE
  写法当场红）+ 半成品修复用例改跟新入口 + 源码钉
  `production_inst_geo_writes_replace_param_wholesale`（禁 MERGE 回潮）。受影响
  面：lib 定向 12 条全绿、`db8000_session_pairs` 20/20 全绿、全量绑定档 83
  passed + 1 skipped。
- **`room_model` 无 project 特性构建编译修复（响亮拒绝）**：`configured_match_room_fn` /
  `load_room_panel_map` / `load_room_panel_map_from_pe` / `build_room_panels_relate` /
  `build_room_panels_relate_common` / `load_room_panel_groups` 此前只有 `project_hd` /
  `project_hh` 两条 cfg 分支，两者皆未启用（CI 单测组合 `ws,gen_model,manifold`）时
  `configured_match_room_fn` 无返回值（E0308）、`let sql` 门控外的取用点找不到 `sql`
  （E0425×2），整个 lib 编译不过。按宪法「禁止填近似值」改为**响亮拒绝**：无 project
  特性时各入口 `anyhow::bail!` 明示「需要 project_hd 或 project_hh」，原实现体整体入
  `cfg(any(...))`；`configured_match_room_fn` 同样门控（无 project 时其调用方已全部
  bail，不再被引用）。附回归单测
  `room_subsystem_loaders_loudly_refuse_without_a_project_feature`（仅在无 project 组合
  编译运行，断言两个 loader 返 Err 且提示特性名）。
- **增量流程文档一致性修复（2026-08-13 流程审计定案，纯文档面）**：
  - 宪法 v1.0.0 → **v1.1.0**（`.specify/memory/constitution.md`）：I 条回退语义按 ADR-021
    改写（回退默认整库重建、仅 `TypeChanged`/`Duplicate`/`Missing`/`ForeignProject` 身份
    歧义阻断、补「承诺必须有数据支撑」读侧对偶），附加约束「并发模型」按 ADR-011
    2026-08-09 修订改写（一个派发器 + 至多 8 个在飞批次）；Governance 增修订记录
    （动机 / 受影响 ADR / 迁移路径），Last Amended 2026-08-13。
  - AGENTS.md 水位段与配置段对齐（消除「回退阻断」与「回退默认整库重建」同文矛盾，
    补数据支撑一条），队列门控段的「同一个 worker」补派发器限定。
  - ADR-021 状态「提议中」→「已接受（2026-08-13 评审决议）」。
  - ADR-011 2026-08-09 修订下游同步：`docs/specs/web-service-api.md` §2/§4.3/§6、
    `specs/002-watermark-data-backing/spec.md` Assumptions、ADR-021 引言——「单 worker /
    一个消费者」措辞补「一个派发器、默认 1、可配至 8 在飞」限定，行为描述不变。
  - `specs/002-watermark-data-backing/` 补 `plan.md`（含 Constitution Check：I 条冲突
    处置 = 本次修宪）与 `tasks.md`（按已落地事实事后补记留痕，每条带文件路径）。
  - live 台账（`docs/2026-08-12_live-test-ledger.md`）：合计修正 86→**92**——A 27→28
    （08-13 新增的端到端用例漏计）；新增 E 组补录 tests/ 目录 5 条集成 `#[ignore]`
    待验行（staged_regen / staged_transform / staged_pane_replay / room_rebuild_repair /
    gen_one_root；`db8000_session_pairs` 的命中经复核是文档注释、无真实 ignore 用例），
    口径行同步扩展到 `tests/**`；C 组 issue7_e2e 行加注「旧语义现场，ADR-021 后需按
    新语义重估重跑」。
  - 房间增量测试计划（`docs/plans/2026-08-12-room-incremental-live-test-plan.md`）§7
    增 08-13 重估行：db1112 的 F6 阻断判词被 ADR-021 取代——部署新二进制后首轮重扫
    将排整库重建批次，Phase C 前置需纳入重建时序、代价与重新定标。
  - CONTEXT.md 增词条：数据支撑 / 幽灵水位 / 重建批次（各带 _Avoid_ 清单）。

- **fix(incr)：副作用补偿队列补齐死信可观测 / 人工复活（`/update/side-effects/retry`）/
  done 行清扫三出路，并将 `room_panel_relate` 纳入整库重建的 Ref0 区间清库（补
  `room_relate` 漏删的姐妹边）**。逆向核实确认：`room_panel_relate` 的 id 形态
  `{room_refno}_{panel}` 可按 Ref0 区间寻址，而房间重算只对现存实体先清后写、从不
  整表清空，整库重建后的孤儿边无人回收（ADR-010 D4 幽灵同类）；修复为
  `fast_delete.rs` `RANGE_TABLES` 增表 + 回归测试
  `the_wipe_deletes_room_panel_relate_alongside_room_relate`，ADR-021 §4 口径同步补记。

- **fix(incr)：重扫路径读不出文件最新会话号时不再吞成 0（消除假回退告警与失实的整库
  重建播报）；CATA 定位器读登记表失败上浮、缺 `db_type` 的库计入 missing 并告警；
  MySQL 镜像 NAME 改参数绑定、DBNO 缺失告警；MQTT 同步去重查询插值统一过
  `escape_surql_str`；各附回归测试**。

## 2026-08-12

### 新增

- **db8000 会话对回归进 CI：夹具格式的七类性质断言，数据驱动**（方案
  `docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md` 阶段三 + 四）：
  - 新增 `tests/db8000_session_pairs.rs`（18 passed）：对 `aios-session-fixture-v1`
    夹具里的**每个案例**跑七类断言——档案完整性、窗口切片、时点一致性、并集律、
    净变化折叠、快照差分对账、历史还原。不硬编码任何 sesno 或 refno。
  - **不等真实录制**：夹具来源两条，`AIOS_SESSION_FIXTURE` 指现成目录，缺省则
    从 issue-019 的 final 现场 `pack` 一份。后者是真实 db8000 会话链上的真数据
    （阶段一自检已证明现切台账与当年独立录制逐字节相等），只是案例集小。真实
    录制到货后改指环境变量即可，断言不动。
  - a) 与 g) 直接复用 `pipeline::verify_fixture`——它本就在做那两件事，重写会
    产生两套必须永远一致的口径。
  - f) 快照差分是新增的通用 oracle：`read_raw_records` + `parse_raw_ele_data`
    在 before/after 两份现切快照间逐元素比对（存在性 + noun/name/owner +
    **children** + 属性表），与净结果互证。**children 必须进比对**：实测
    issue-019 的删除序列里，父件与祖父的属性表一个字节都没动，Modified 的信号
    全在 children 列表上；只比属性会把它们误判成「增量说变了但文件没变」。
    噪声属性白名单目前为空——方案预判的 CACHID 类漂移在这条链上没有出现，
    常量留着是机制不是占位。
  - CI：`db8000_session_pairs` 接进同一 job；失败时 upload-artifact 传完整断言
    输出与夹具台账（新增 `AIOS_SESSION_FIXTURE_KEEP` 让合成夹具落在工作区，
    否则临时目录跑完就没了、远程红了无从对账）。
  - job 更名 `db8000-model-increment` → `offline-increment-regression`：它现在
    跑五步离线回归，早已不止 db8000 的模型增量。**配了分支保护必需检查项的话
    需要同步旧名字。**
  - 远程首跑（run 31572427572，2026-08-12 15:03 dispatch）：
    `offline-increment-regression` **首跑即绿，28m58s**——五步（issue-019 夹具、
    通用切割自检、session-pairs 回归、记录边界解析、删除清理 lib 用例）在
    GitHub Actions 上全过。同 run 的 `python-bindings` 在 wheel 冒烟一步红
    （runner 侧 DLL 解析，与离线回归无关），修复走 `4ddf32b9` 的 DLL 探针
    传递闭包，验证 run 另行跟进。

- **db8000 录制清单补齐到 6 类变更形态，并加了一道离线闸**（方案阶段二的离线前置；
  录制本身仍等生产空窗）：
  - 新增 8 个宏、5 个案例：`scratch-create`（added）→ `data-rename` /
    `transform-move` / `geometry-resize`（各 modified，apply+restore）→
    `delete-box`（deleted）。`-CheckOnly` 静态审查 12 条腿 / 7 案例全过。
  - **不动生产元素，改为自建 scratch 元素**：restore 腿必须把值放回原状，而
    生产元素当前的 POS / XLEN 离线不可知，照方案原文写就得先占一次空窗做探针。
    自建元素让每个 before/after 值都自决，整套宏离线可写可评审；末位 delete
    收尾后库在逻辑上回到原样。副产品是 `added` 净形态——原案例表里一个都没有。
  - 新增 `recording_manifest_survives_the_sesno_assignment_it_will_get`：按录制
    脚本的 sesno 分配规则预演清单，`plan_cases` 必须接受，`expected_net.element`
    必须已声明、宏文件必须在场。这类错原本要等占完空窗、录完一整轮才在打包时
    炸，现在 `cargo test` 就拦（防伪已验）。

- **live 用例台账 + 首批点亮**（7-27 测试计划 Gate 3 的首次执行）：
  `docs/2026-08-12_live-test-ledger.md` 给全仓 82 个 `#[ignore]` 用例建档
  （四类目标要求 + 最近通过记录），`scripts/Run-LiveBatch.ps1` 按清单逐项
  独立进程定靶批跑（`DB_OPTION_FILE` 与 `AIOS_LIVE_*` 两套寻址同源派生、
  恰一命中、逐项超时、JSON 报告）。批次 1（自建夹具类 26 项）全部得出结论：
  **23 项首次取得可复现通过**（12 @ testbed 8019；11 项 room_fixture @
  一次性空库 8071 专用清单——房间覆盖率闸门在带真实基线的库上对「只灌夹具」
  的树必拒，空库上夹具行自然对得上分母、闸门语义原样保留），3 项阻塞定性在案
  （积压前置 / 缺陷面板数据依赖 / 断言写死生产 MDB 语义）。顺带修三处测试
  腐化：白名单落地前的夹具命名（first/second 进不了判重）、状态机落地前的
  两个崩溃恢复用例缺测试装载模式声明。另补 IU-S8-05/S12-02 顺序钉（部分失败
  后缓存仍失效、水位不推进）与 IU-S0-05 的 warning 半边——7-27 矩阵点名的
  L0 缺口至此全部关闭或有台账去处。

- **房间增量默认打开（`room_incremental` 缺省值 `false` → `true`）**：`options.rs`
  的 `effective_room_incremental` 缺省翻真，`DbOption.toml` 同步写成 `true`，
  单测由 `room_incremental_is_off_unless_someone_asks_for_it` 改成
  `..._is_on_unless_someone_turns_it_off`，并补一条「认不出的环境变量值退回新
  默认」。
  - 2026-08-10 取假是为了压住现场那 2580 个查不到几何的房间目标（每页 256 个
    付两次全量查询、约 88 秒，把模型侧真正的失败埋进日志）。那批目标已经收干净
    （现场 `/update/pending-units` 的 `room_units` 为空），维持关闭的代价此刻更
    贵：关着时房间归属**只在删除路径**还会被清理（`helper.rs` 的
    `delete_room_membership` 从不看这个开关），搬家后的重算全靠启动全量重建
    回补，而那条兜底路径排在 `startup_autorun` 之后（`skip_startup_room_build`
    的三道门次序）——默认部署两个开关都关着，等于没有回补通道。
  - 门本身一处没动：两个写入点（直写事务的 `room_recalc` 语句、暂存窗口收口的
    `merge_room_recalc_changes`）与一个消费点（`room_round`）照旧读同一个函数，
    翻的只是缺省值。显式写了 `room_incremental = false` 的配置（`python/tests/
    DbOption-ci.toml`、`python/testbed/*`）行为不变；要临时关一次用
    `AIOS_ROOM_INCREMENTAL=0`，不必改文件。

- **空间树一致性闭环：V2 单文件快照、进程状态机、空间串行锁与降级自愈**（方案
  `docs/plans/2026-08-12-spatial-tree-consistency-closure-plan.md`，D1–D8 已定；
  ADR-010 2026-08-12 增补（二）、ADR-017 2026-08-12 补记）：
  - 快照介质：`accel_tree_{project}.bin` + `.meta.json` 退役，改为单文件
    `accel_tree_{project}.snapshot`（V2：树载荷 + SHA-256 自校验 + project/namespace
    身份 + 双字段指纹，原子 rename 发布）。读侧全套校验任一失败即指针重建，不回落
    旧格式；旧文件仅作一次性迁移候选（双指纹匹配且无 pending → verdict=`migrated`），
    首次 V2 发布成功后**删除**——旧二进制对 bin 缺失是无条件重建，任何回退自动安全。
  - 状态机 `spatial_state.rs`：8 态；房间消费者（启动全量重建、RoomRecalc、空闲
    房间轮）仅 Ready/ReadyEmpty 放行（`SPATIAL_TREE_NOT_READY`，durable 行保留），
    解析/生成/重放/重建/`model.spatial.bounds` 不受门禁。启动判据修正：pending
    优先仅对可读快照成立、进入 ReplayRequired 立即重放（不等派发门）、
    「树非空即 preloaded」收窄为显式夹具标记。
  - 空间串行锁 `SPATIAL_STATE_SERIAL`（`STAGED_COMMIT_SERIAL → SPATIAL_STATE_SERIAL
    → GLOBAL_AABB_TREE`）：staged 提交后收敛、direct 写路径、重建换树/发布、快照
    落盘、Python `spatial.*` 同一串行线（修掉 Python reconcile/persist 与 worker
    并发动树的竞态）；journal 写回与尾事务不持锁。
  - 指针重建：record-range 分页（fork 兼容套件双跑钉住页间无漏无重）；口径
    current-only（排除版本化数组 id 行、`in.deleted` 软删行，Rust 侧排除
    NaN/Inf/反向 AABB 并计数采样）；分页读锁外、stamp 前后比对 + 换树 + 发布锁内，
    三连漂移/查询失败进 DegradedBlocked。房间覆盖率分母同口径。
  - 降级自愈：后台 revalidator（30s 指数退避至 5min）只管 DegradedReuse/
    DegradedBlocked，恢复 Ready 唤醒调度器。
  - 崩溃注入：`AIOS_FAILPOINT=<name>` 五个注入点覆盖方案 §8 崩溃窗口。
  - **对外契约变化**：/health `spatial_tree` 九键作废换十五键（台账 G-02 契约
    迁移，形状钉随迁）；`startup_verdict` 枚举改
    reused/replayed/rebuilt/migrated/degraded/preloaded；Python
    `spatial.persist(force=True)` 在非 Ready/ReadyEmpty 拒绝。
  - 沙箱验收（testbed @8019，六场景：首启重建/快路径复用/截断/删除/rename 前
    崩溃注入/崩后收敛）全过，证据
    `docs/2026-08-12_spatial-tree-consistency-acceptance.md`；E3D 侧场景
    （TTY 复制恢复对拍、伪造旧 epoch、房间边对拍）留 runbook 待跑。

- **db8000 会话快照夹具通用管线阶段一**（方案
  `docs/plans/2026-08-12-db8000-session-snapshot-fixture-test-plan.md` §1）：
  切割（`session_cut`）/格式（`aios-session-fixture-v1`）/打包（`archive_util`）
  /管线（`pipeline`）四个模块接上 `db_session_fixture` bin——`pack` 把
  「recording.json + 源 DB 文件」打成只入库最终文件的夹具（台账 sesno 逐切过
  sesno+存在性验证闸、散列入 manifest、6 MiB 预算、收尾即复验），`verify` 对
  夹具目录零外部依赖离线复验（解 zip → 逐台账**现切** → SHA256/大小对账 +
  验证闸，与阶段三回归同一套裁决）。阶段一验收由
  `tests/db_session_fixture_selfcheck.rs` 钉住：用通用模块从 issue-019 zip 的
  final（sesno 26）现切 24/25/26，字节散列与该夹具 manifest 台账逐一相等——
  「任意历史可从最终文件精确还原」在真实 db8000 会话链上成立。同一测试还覆盖
  **pack 往返**（真实源文件 → 夹具 → 复验全绿；台账散列与 issue-019 独立录制
  的那份相等；台账改一位后复验必须变红），因为阶段二的 E3D 录制是一次性的、
  pack 出错要再占一个生产空窗重录。issue-019 专用实现保持冻结不动。该测试已
  接进 CI（`windows-tests.yml` 的 `db8000-model-increment` job，参数与
  issue-019 步骤逐字同款），同批把一直漏在门外的离线解析边界用例
  `--test pdms_record_boundary` 也接了进来。

- **阶段二录制工具**（同方案 §2，待生产空窗执行）：
  `scripts/e3d/Record-Db8000SessionChain.ps1` + 清单驱动的
  `scripts/e3d/db8000_recording_cases.json`（加案例 = 加一对宏 + 一行），投递走
  ADR-019 的 `l3_suite --check-driver`。录制一次性且占生产空窗，所以三道闸都当场
  验：触碰 E3D 前静态审宏（恰好一个 `SAVEWORK`、无 `QUIT`/`FINISH`/`MERGE`/
  `PURGE`、`ALPHA LOG` 成对、`Q REF`+`Q NAME` 齐全）、每条腿后要求 sesno 恰好 +1、
  refno 从宏日志的 `Ref`/`Name` 相邻对回读。配套给 `db_session_fixture` 加
  `inspect` 子命令（只读打印会话链 JSON，与切割同一份解析）。`-CheckOnly` 只读档
  已对真实 `ams8000_0001` 实跑通过（baseline_sesno=210），检查器的拒绝面亦验过。

### 修复

- **直写路径的空间树变更补上 epoch 痕迹，消除崩溃后的静默漂移**（方案
  `docs/plans/2026-08-12-spatial-tree-direct-mutation-epoch-trace-plan.md`，
  ADR-010 2026-08-12 增补）：
  - 钉死不变量——**凡是改变了「树应有内容」的已提交变更，都在同一事务内 bump
    `spatial_epoch:current`**。此前只有 durable 增量与暂存窗口尾事务 bump，
    全量生成 / `manual_update_aabbs` 的普通直写刷新与删除清理两条路既不写
    `spatial_reconcile` 意图行、也不 bump：树同步完、空闲轮落盘前崩溃，重启时
    sidecar 与库指纹相等，启动判据按 Reuse 复用一棵陈旧的树，而 /health 的
    `drift` 恒为 false，无人可见。删除路径的后果尤其重——启动全量房间重建会把
    被删构件按旧包围盒重新收编进 `room_relate`（ADR-010 D4 借崩溃复活，而
    `DeleteCleanup` 任务早已 done，没有重放会再清一次）。
  - `update_inst_relate_aabbs_by_refnos_mode` 的直写事务门控从
    `durable_room_trigger && !chunk_changes.is_empty()` 放宽为
    `!chunk_changes.is_empty()`；`durable_room_trigger` 从此只决定「要不要随事务
    发布 `room_recalc` 任务」，不再决定「要不要事务与 bump」。重算值与树上旧值
    逐位相等的重刷仍走普通写、不 bump——没动树的提交不该作废别人的树文件。
  - `delete_room_membership` 的窗口外分支改为按块「取写锁 → 锁下探测这些 refno
    在不在树上 → 在则把房间边删除与 bump 包成一个事务、不在则照旧普通写 →
    摘树 → 标脏」。探测在锁下做，「要不要 bump」与「树到底动没动」由同一个快照
    裁决。暂存分支不变（意图行 + bump 仍由窗口尾事务收口）。
  - 普通直写分支补上写锁，跨度 [变更判定 → 事务 → 树同步]（durable 增量的全跨度
    锁不变）。顺带关掉一个此前没盘到的交错窗口：并发的删除清理挤在事务与同步
    之间时，刚摘掉的条目会被这里同步回树上，成为要等下次指针重建才自愈的幽灵。
  - 行为变化：全量生成、`manual_update_aabbs`、删除清理的直写提交现在会推进
    spatial epoch（按块，一次全量生成约产生「条目数/100」次 bump）。多次 bump
    语义无害（判据只比相等），代价是这些路径跑过之后，下次启动的全量房间重建
    对账凭据（`room_build:main`）会判为「空间状态已变」而照跑一次。

### 变更

- **`aios_db.model.export_obj` 改为整树单文件导出，子树收集走 anc 索引**
  （2026-08-12 增量审查修复计划 P3）：
  - 对外契约变化：此前每个实例根一个 `{refno}.obj`，现在整棵子树合成一个
    `{refno}.obj`、内部按「实例_geo_hash」分 `o` 组，`files` 恒为单元素数组；
    交付单元根（EQUI/BRAN…）自身没有直接实例行也能导出整树。
  - 子树实例收集只走 `anc CONTAINS`（`idx_inst_relate_anc` 索引查询，anc 含
    自身故根自己的实例行同谓词圈住），不再 OR 无索引的 `in = …` 臂——那会把
    整条谓词退化回全表扫（preload.rs 实测账：1.57s vs 3.1ms）。
  - 响亮失败取代静默空集：refno 解析失败直接报错（此前静默成 0、谓词永不命中）；
    空结果时按 rs-core `inst_relate_anc_ready` 同口径探一次 `anc = NONE`，
    存量未回填的库给自愈指引（启动一次 gen-model 回填）而不是谎报「没有实例」。
  - testbed 全链路（`python/testbed/run_full_loop.py`）导出步骤补形状断言：
    单文件、`o` 组数 == 导出实例数、triangles > 0、无缺失 mesh。

- **`aios_db.full_init` 增加同工程活服务探测**（行为变化：以前能起的场景现在
  可能被拒）：拿锁之后探本机 `http_api_addr` / 8022 / 9099 的 `/api/v1/health`，
  响应是合法 health JSON **且** `project` 与本配置一致就报错退出，
  `full_init(..., force=True)` 显式跳过。动机是单实例锁按「项目根」隔离，两个
  部署包各持各的锁却写同一个工程时锁根本不挡（实测踩过：`test-worklspace`
  的包在 9099、本仓库在 8022）。判据只认 project 名——`/health` 不报「它连的是
  哪个 SurrealDB」，所以隔离沙箱若与生产**重名**会被误伤，用 `force=True` 放行
  （`python/tests/conftest.py` 就是这么做的，并在注释里写明三条资源如何独立）。

- **互踩探测精确化：/health 补报库端点，探测端按「同库」而不是「同名」判**
  （上一条落地当天就被自己的测试沙箱误伤，这是补上的另一半）：
  - `/health` 的 `sul_db` 新增第六键 `endpoint`（配置的 `v_ip:v_port` 原样
    字符串），形状钉与 spec §4.1 同步；`sul_db` 其余五键语义不变。
  - `full_init` 的探测升级为三层判据：`project` 不同 → 无关；服务端报了
    `sul_db.endpoint` → 端点（localhost↔127.0.0.1 归一后）或 `namespace`
    不同都放行——同名工程各写各的库不构成互踩；老版本服务端（≤0.1.18）不报
    端点 → 分不清仍按最坏情况拦，拒绝文案会写明是哪种判法。判定函数是纯函数，
    8 条单元测试钉住（`cargo test -p aios-py --lib`，含对实测 9099@0.1.13
    响应形态的老服务端分支）。
  - `python/tests/conftest.py` 的 `force=True` 暂留：本机 9099 还跑着 0.1.13，
    等同机部署升到带 `endpoint` 的版本即可撤。

- **`aios_db.spatial.tree_status` 的文档与存根不再复述键面**：改为「原样透出
  /health `spatial_tree` 那份渲染，键面以 Rust 侧渲染半边为唯一权威」。此前注释
  与 `.pyi` 各抄了一份九键清单，而 G-02 契约迁移正把它往十五键上带——两处各说
  一套，过期是必然。Python 面只钉判漂移要用的稳定核（`entries` / `file_epoch` /
  `db_epoch` / `drift` / `startup_verdict`），全集的形状钉留在 Rust 侧一处。

- **`aios_db.db.inst` 去掉全表扫，改三段式取边**：① `anc CONTAINS`
  （`idx_inst_relate_anc` 索引查询，anc 含自身故一跳圈住整棵子树）；② 空结果
  回落 `array::flatten(SELECT VALUE ->inst_relate FROM [pe:…])` 图跳，只取元素
  自己那一跳（preload.rs 的实测账：`in` 谓词全表扫 1.57s vs 图跳 3.1ms），兜住
  `anc` 未回填的存量库与直接 `RELATE` 出来的测试夹具；③ 两条都空且库里还有
  `anc = NONE` 行时响亮报错——「查不全」不能被读成「没有」。refno 解析失败也
  改为直接报错（此前 `unwrap_or_default()` 静默成 0，谓词永不命中）。与
  `export_obj` 的差别是多了第 ② 段：那边空结果本就是错误条件，这边空是合法答案。

### 新增

- **`aios_db` 补齐测试支撑面导出，并新开房间增量 pytest 轨**：
  - 新增 `aios_db.fixture`（`create` / `drop` / `move_body` / `refnos`），直通
    `src/fast_model/room_fixture.rs` 的合成房间夹具（1 间 `/ZZ-R-K100` + 2 块
    PANE + 5 个盒形构件，其一骑在重叠区，保留 refno 段 4000000001）——与 Rust
    `room_fixture` live 轨共用同一套数据，两侧断言可互相印证。会写 pe/FRMW/
    inst_*/geo_relate/aabb/vec3 多张表并落 `zzfx_*.mesh`，**只对一次性测试库使用**。
  - 新增 `room.enqueue(changes)`（按 `model.update_aabbs` 的返回形态入队房间
    重算，PANE 走整间分支、其它走元素分支，不受 `room_incremental` 开关门控）、
    `model.delete_subtree(refnos)`（DeleteCleanup 补偿任务同一入口的级联删除）、
    `spatial.tree_status()`（空间树九键指纹，与 /health `spatial_tree` 同源、
    现读现比）、`model.update_aabbs(..., durable=True)`（生产 TransformOnly /
    定向 regen 走的直写事务路径：AABB 指针、`room_recalc` 任务与 spatial epoch
    同事务提交）。
  - 新增 `python/tests/`：对 conftest 自起的一次性内存 SurrealDB
    （`bin/surreal.exe` @8071，进程退出零残留）跑「房间增量收敛 == 全量重建」
    的**逐边**对拍，覆盖构件搬家、面板整间、空刷负例、删除清边留痕、durable
    直写五条；配置 `tests/DbOption-roomtest.toml`（`room_key_word=["ZZ-R-"]`
    只圈夹具房）。conftest 会把仓库根同名空间树文件挪开再还原，不毁真项目产物。
  - 类型存根补齐（新增 `fixture.pyi`，`model` / `room` / `spatial` /
    `__init__` 同步新入口），`py.typed` 对外契约不再漂移。

- **绑定的离线测试档进 CI**（`python/tests -m offline`，60 条）：解析层对着仓内
  `issue-019` 的 db8000 会话快照（与 Rust `db8000_two_delete_fixture` 同一份
  数据、同一串删除序列）、三层硬守护在干净子解释器里逐条验、`.pyi` 与运行时的
  名字集合逐模块对齐、HTTP 客户端对着打桩服务验 12 条 REST 路由与报文形状。
  这一档不连 SurrealDB、不碰 E3D 装机、不扫项目目录，秒级跑完。
  - `.github/workflows/windows-tests.yml` 新增 `python-bindings` job：复用
    `windows-binary.yml` 的 OCCT / protoc provisioning（绑定按 Q7 钉死「与服务
    同一套默认 feature」，必须有 OCCT）→ `maturin build` → 装 wheel → 跑离线档
    → wheel 作 artifact 上传。原 `db8000-model-increment` job 不动。
  - conftest 按选中集合裁定本进程那一份 DbOption（进程级 OnceCell，换库只能换
    进程）：有房间档用例就用 `DbOption-roomtest`，纯离线档用新增的
    `DbOption-ci`。离线用例在任一配置下都成立，两档同跑不冲突。
  - 新增连接层行为用例（`test_connection_layer.py`）：`db.inst` 三段式的每一段、
    `owner_chain` 的自 own 终止、`members`、`spatial.tree_status` 十五键形状钉。

- **版本护栏自锁**：`aios_client.EXPECTED_SERVER_VERSION` 是手抄常量，此前
  `chore(release)` 升 `Cargo.toml` 时没有任何东西提醒同步它——护栏自己先漂移，
  从「提醒你版本不一致」退化成「对着新服务端瞎报警、对着老服务端不报警」。
  离线档新增一条对表用例，bump 忘改常量时 CI 立刻红，红处文案直接写修法。
  `sul_db.endpoint` 恰好是 0.1.19 起才有的键，这条对表马上就有实际意义。

- **空间树一致性闭环落地后的绑定侧跟进**（对方工作流 `445e3cd1` 合入后复核，
  绑定重建 + 全套复跑，77 条绿）：
  - conftest 的树产物搬挪清单补上 V2 单文件快照 `accel_tree_{project}.snapshot`
    ——房间档跑在一次性内存库上，但空间树落盘写的是**仓库根**、文件名与真项目
    同款；清单漏项就会拿测试产物顶掉真项目的快照（介质迁移期间已实际残留过一个）。
    并加源码钉：从 `aabb_tree.rs` 反查 `accel_tree_{}` 的全部后缀，与清单比对，
    下次换介质忘了跟进直接红。
  - `spatial.tree_status` 的断言从「稳定核子集」收紧为**逐键全等**。钉的不是
    契约本身（那在 Rust 渲染半边旁），而是绑定透传没掉键——最容易掉的是取值为
    null 的几个（`snapshot_sha256` / `pending`），掉了不报错，只会让照着 /health
    写的脚本在绑定上撞 KeyError。
  - 钉住「Python 夹具路径免标记通过消费者门」：Rust live 夹具要显式调
    `mark_spatial_tree_fixture_preloaded()`，Python 这条路不需要——`full_init`
    走正经装载器，空库上从指针重建出空树即进 ready 态。省得下次门禁收紧时对着
    一堆 `SPATIAL_TREE_NOT_READY` 猜是不是缺了那个标记。

- **`scripts/smoke_m1..m5.py` 标注退役**：五个脚本全部钉在仓库根 `DbOption` +
  8009 正式库 + `D:/AVEVA/...` 真实工程上，而 8009 的数据目录已决定不修，照原样
  跑必失败。不删（它们是 M1–M5 的验收口径记录），改为每个脚本头注写明「历史
  验收记录 / 为何跑不了 / 等价物在哪」，README 脚本表合并成一行同款提示。
  多数段落已被两档 pytest 覆盖；`parse.noun_dict` 依赖 E3D 装机的 `attlib.dat`，
  没有自动化等价物，头注里点名说清。

- **`aios_client` 版本漂移护栏**：`health()` 比对服务端 `version` 与内置
  `EXPECTED_SERVER_VERSION`（现 0.1.18），不一致抛一次 `AiosVersionWarning`
  （同一个 client 不刷屏），`AiosClient(..., expected_version=None)` 关掉。
  回应实测踩过的 0.1.13 绑定对着 0.1.16 部署包查半天的坑——只告警不报错，跨版本
  多数字段仍通用，硬拦会把「凑合能用」变成「完全不能用」。

## 2026-08-11

### 变更

- **层级查询优化 P3（gen-model 份额）：退役 `inst_relate`/`tubi_relate` 的
  `zone_refno` 列**（方案 `docs/plans/2026-08-07-inst-relate-anc-u64-hierarchy-query-plan.md`
  P3，as-built 记录 `docs/2026-08-07_journal-fn-dependency-audit.md` §4）：
  - 写侧全退：建行字面量（普通行 + TUBI 行）不再写 `zone_refno`，
    `ResolvedInstMeta` 的 zone 槽位与 `resolve_inst_meta` 的 noun 预取移除；
    回填 `backfill_inst_relate_anc` 与 OWNER 搬迁重算
    `render_anc_repair_statements` 不再连带 `zone_refno = fn::find_ancestor_type(...)`
    ——每行一次的 9 跳 owner 上溯从回填成本中整个消失，`fn::find_ancestor_type`
    自此离开 inst 写入链（函数定义保留给材料表等读侧）。
  - 收口探针收窄：`desi_finalize_preflight` / `selfcheck_surreal_functions`
    只探剩余的收口硬依赖 `fn::anc_u64`。
  - 索引迁移：`INST_RELATE_INDEX_SQL` 前两行 `REMOVE INDEX IF EXISTS` 在启动/建窗时
    摘除旧 zone_refno 索引的两个历史名字（`idx_inst_relate_zone_refno` 本仓 F1
    修复后建的、`inst_relate_zone_refno_index` plant-ui rs-core `define_pe_index`
    建的，AMS 实库两者并存实测在案）。存量行旧值保留不删，只是不再写入、不再被
    索引；「索引不存在 / 表都不存在 / 重复摘除」三种 no-op 情况由双跑用例
    `dual_inst_relate_anc_u64_contains_index_agrees` 连 INFO 终态一起钉住。

### 新增

- **`fn::zone_u64` / `fn::site_u64`（common.surql，P3 读侧便捷层）**：从 `anc`
  链尾 O(1) 定位 ZONE/SITE，与元素深度无关——链尾打包值 ref1==0 即 WORL 的
  自适应偏移，判据与 Rust 解析器同源；「含自身」语义与退役的
  `fn::find_ancestor_type` 口径一致，短链/空链返回 NONE。反向圈行（某 ZONE 下
  全部实例）仍走 `anc CONTAINS` 索引查询，不用它。两种链尾真实形态（悬空 WORL
  收尾 / 0_0 哨兵被滤止于 SITE）由 `zone_and_site_helpers_locate_from_the_anc_tail`
  与双跑用例 `dual_anc_u64_functions_execute_and_agree` 双引擎钉住。

## 0.1.18 - 2026-08-11

### 变更

- **空间树启动初始化改为分层判据**（方案与决策记录
  `docs/2026-08-11_spatial-tree-startup-init-plan.md`，ADR-010 2026-08-11 增补）：
  - sidecar 指纹从单一 epoch 数值扩成 **(epoch 值, 库侧 bump 时刻 `updated_at`)**
    双字段，两个字段都与库相等才直接复用树文件（评审要求：与数据库对时间戳；
    库快照回滚恰好撞回同一计数也认得出来）。旧版 sidecar 缺新字段按失配走，
    一次自愈后补齐。
  - 指纹失配但库里还有待重放空间意图 → 复用文件、交给 worker 出队前的意图重放
    自愈（不再像旧 epoch 校验那样每次崩溃重启都全量重建）；失配且无意图 →
    只读指针重建（直写崩溃 / 换文件 / 回滚库）。
  - 树文件缺失/损坏从「空树等人工」改为**自动指针重建**（决策 D1）；库侧诊断
    查询失败降级复用文件 + 告警（D2）；两处启动调用点统一为「告警降级空树、
    不阻断启动」（D3）。
  - /health 新增 `spatial_tree`：文件/库两侧指纹现读现比、`drift`、条目数与
    本次启动裁决（reused / healed_by_replay / rebuilt / empty / preloaded /
    reused_degraded）。

### 修复

- AMS/8000 房间增量灰度闭环：
  - `inst_geo` 的确定性落库从忽略重复改为 `UPSERT ... MERGE`，保留已有 mesh/AABB，
    同时补齐缺失参数并在显式重生成时清除 `bad`；重复执行同一生成批次可收敛。
  - 无圆角、共线回折的房间面板不再走一次性删点失败分支，统一使用逐交点修复器；
    加入 AMS 真实 PLOOP 参数回归。
  - `startup_autorun=false` 时，显式 `POST /update/execute` 即使所选 dbnum 已追平，
    也会为本进程上弦并放行 durable 模型/房间积压，避免人工 canary 永久停在
    `up_to_date`。
  - `Run-RoomE3DE2E.ps1` 新增复用现有 9099 服务的 `db8000-equi-copy` 案例；
    TTY 宏对 `=24384/24776` 执行 probe/apply/restore，并核对水位、新 EQUI、
    `inst_relate`、pending/dead-letter 与空间补偿。

- `AIOS_FORCE_SPATIAL_REBUILD` 只认明确真值（1/true/yes/on）：旧实现判
  `is_ok()`，部署模板写 `=0` 想关闭，实际每次启动都强制全量指针重建。三态解析
  收口在 `batch_worker::parse_explicit_flag`，与 `GEN_MODEL_DIRECT_INCREMENT`
  的 P2-1 纪律同款。

## 2026-08-06

### 新增

- **执行范围缓存 + 周期对账重扫**（现场：数据批次执行中 SUL_DB 连接抖动，watcher 的
  范围解析报 `receiving from an empty and closed channel`，整批文件事件被丢弃且无重试）：
  - 文件事件路径的 MDB 范围解析改走进程内单槽缓存（`UpdateScope::resolve_cached`）。
    名单只在 SYS meta 批次落库时才变，那一刻与 `SCOPE_DIRTY` 同点显式失效；TTL 兜底
    `AIOS_SCOPE_CACHE_SECS`（默认 300s，0 关闭）。SUL_DB 瞬时不可用时暖缓存放行并告警，
    冷缓存与配置错误（mdb_name 没填 / MDB 名不存在）维持 fail-closed 上抛。
    启动重扫、重挂补扫、周期对账与手动路径仍每次真查（fresh），它们就是缓存的刷新点。
  - watcher 事件循环新增周期对账重扫（`AIOS_WATCH_RECONCILE_SECS`，默认 300s，0 关闭）：
    按间隔整面重比「文件最新会话号 vs applied 水位」，把连接抖动、服务重启等一切来源
    丢掉的文件事件在一个周期内追回；入队按水位判定天然幂等，与启动重扫共用
    `sweep_watch_dirs`。

- **issue #10 复现套件**（`src/data_interface/staging/issue10_add_node.rs`，仅测试编译）：
  用真实渲染与真实暂存窗口（`stage_parsed_window` → `register_staged_finalize` →
  `commit_registered_to`）在 mem 引擎上模拟 E3D「复制 BRAN 并 SAVEWORK」的连续增量，
  钉住三条路径——连续多个窗口写回后新增节点必须出现在模型树（含父成员序边重建）；
  窗口因生成重试耗尽被阻断时「检测得到、树不动、水位原地」，吸收重置后重算收敛；
  journal 被持久层确定性拒绝（坏版 `update_dbnum_event` 对字符串 id 的 pe 行报
  `array::at` 类型错）时写回整体回滚、零半写，排毒后同一份 journal 重放收敛。
- **批次执行的阶段日志**（issue #12）：完成行补上 sesno 窗口与墙钟完成时间，在 E3D 里
  SAVEWORK 的人可以直接对上「屏幕上这批日志是不是我刚才那次保存触发的」；模型计划按
  action 分组计数（形如 `regen_root=3 transform=12`）而不再只报总数；交付单元、批量
  重生成、房间归属重算各自报耗时与成败；生成根列表超过 8 个截断并报出总量。

### 变更

- `DbOption.toml`：`manual_db_nums` 由 `[7998, 8000]` 放宽到 `[7997, 7998, 7999, 8000]`，
  纳入 issue #10 的 E3D 实测窗口（基线库 `.surreal/ams-7997-e3d-test-20260805`，7997
  applied=92）。取证结论就地记在配置注释里：issue 截图中的 `/1WCC0211` 属于 7999，而
  7999 一直被排除在手动窗口外（applied=3、file=41），库里的树是旧全量同步的残留。
  实测跑完可收窄回 `[7998, 8000]`。
- 房间轮次日志不再只在「队列跑空」那一轮打印，距上轮超过保底间隔触发的那轮同样报出
  目标数与死信数。

### 内部

- `attempts::record_window_block_at_on` 提升为 `pub(crate)`，`StagedFinalize` 增加
  `Debug`，供复现套件构造阻断现场。
- `src/bin/manual_scan_probe.rs`、`src/test/mod.rs` 仅 `cargo fmt` 排版整理。

---

## 历史记录

1、添加自动增量更新文件的修改，启动时会检查当前数据库和E3d数据库的一致性
## 2026-08-14

### 新增

- Python 解析层新增 `aios_db.parse.net_window(path, start, end, detail=False)`：复用生产
  `net_window::collect_net_window`，只读 dabacon 文件即可得到属性语义上的净增删改，
  并透出 `unchanged_rewrites` / warnings。与原 `parse.net_changes`（索引记录位置触达
  三态）分工明确：E3D TTY 的 apply + restore 合并窗口会过滤已恢复的业务属性，
  同时如实保留 E3D 自增的 `CACHID` 等保存期元数据，不需要反查 SurrealDB。
- 新增 `scripts/e3d/Test-TtyNetWindow.ps1`：自动执行 FTUB apply / 解析器断言 /
  restore / 合并窗口断言，`finally` 保证恢复腿执行，并产出 baseline 副本、语义 diff、
  命令退出状态与 rollback 验证记录。
## 2026-08-14

### 新增

- 定义严格的 SYS/DICT → CATA → DESI → 模型 → 房间初始化阶段与跨项目元件库优先级契约。

### 修复

- 初始化模型生成将受数据就绪门控，避免 DESI 或模型越过尚未完成的元数据、元件库阶段。
