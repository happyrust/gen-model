# AMS8000 目录实例增量验证

## 已验证范围

使用私有 AMS8000 的 PIPE `24384/26639`、BRAN `24384/26640`、VALV `24384/26641`，经原生 SPCO `/DR07C/MACB200:10` → SCOM → GMSE `15192/225911` 实例化目录 SREV。旧名 `/CODEX_TYPES_CAT_SBOX` 是初始试验标签，不代表验证了 SBOX；位移后已改为 `/CODEX_TYPES_CAT_SREV_UPDATED`。

证据目录：`output/rocksdb-8000-20260905/equi-increment/`。

| 快照 | 操作 | 结果 |
|---|---|---|
| catalogue-native-created | 规格 10 | 几何、NAME/POS/SHOP/BUIL、SPRE 的 API/持久层一致性通过 |
| catalogue-native-resized | SPCO 规格 10→25 | 同上通过；网格哈希与体积均变化，未调用 ensure |
| catalogue-native-moved | 位移 E120、改名、SHOP/BUIL=true | 阀件几何和数据通过；整支管失败，多出 RVM 没有的 TUBI |
| catalogue-native-moved-aligned | 支管头尾位置同步到 E120 | 完整几何和五个数据字段通过；不能据此关闭上一行的多管段缺陷 |

通过项详见各目录 `catalogue-comparison.json`。前三个成功几何对比的边界误差不超过 0.000001 mm，体积相对误差不超过 0.000000209。当前新目录实例尚缺无遮挡的 Plant UI 连续增量截图。

## 最初失败及调查：多出的头部隐式管段

首次位移后，运行时多出 BRAN 名下长 120 mm、直径 25 mm 的 TUBI，`is_invalid_tubi=false`；RVM 仅有阀件的两组几何，没有任何 TUBE。失败证据保留，未修改生成器来迎合试验。

已逐项探测：

- `catalogue-explicit-head`：HPOS 显式设为 0（LHEA=true），TPOS=E120。仍多 TUBI，RVM 无管。
- `catalogue-head-bore`：只将 HBOR 从 0 改为 25。RVM 仍无管。
- `catalogue-head-stub`：只补入现有真实 HSTU `/YBS3/CIP00001:P,7`。RVM 仍无管；生成器继续生成 TUBI。
- `catalogue-tube-export-probe`：REPRE level 0/6，分别导出 BRAN/PIPE，四份 RVM 均无管。排除了这两个导出设置的影响。

当时 HCON/TCON 仍 unset，PSPE unset；HREF/TREF 空。尚未证明是哪项原生规则阻止 RVM 管段，不能把缺少 HSTU 当作已确认根因。

后续 `catalogue-head-connection` 将 HCON 设为 OPEN，`catalogue-position-flag` 将设计构件 POSFLAG 设为 true，仍无 RVM TUBE。`catalogue-points-probe` 原生查询确认：APOS=E120、LPOS=E130、ADIR=W、LDIR=E、ABOR/LBOR=25，几何端点与生成器一致。

SCOM `/MACB200FF` 的 GTYP 实为 COUP。已另建匹配类型的 COUP `24384/26643`（BRAN `24384/26642`），其原生连接点相同，RVM 也只有两组构件几何，见 `catalogue-coup-control-retry/rvm.json`。该样本自动发布失败五次，原始服务日志与发布超时已存档；重启新版服务后目标会话从 491 推到 492 并恢复发布。这是恢复证据，不能计为原始自动增量通过。

原日志仅显示事务失败。`model_db_adapter` 现保留事务内全部语句错误及索引，新增实际回滚事务测试通过，服务编译通过。待复现时确认是否为旧 desired_target 与当前生成会话冲突，尚未修改 CAS 守卫。

Oracle 浏览器会话 `review-the-documented-extra-implied` 已完成交叉审查。建议从有 RVM 头管的正样本验证有效 PSPE 与端点连接状态的组合；这些仍是待验证假设，不作为已证明规则。尚未改动隐式管段算法。

## 剩余类型

### 正样本单条件增量与 Plant UI（后续复核）

复制真实 `/LV-CO-1RX-3062E` 为 BRAN `24384/26644` `/CODEX_TYPES_POS_HEAD`。副本包含 FTUB×2、BEND×1、头部 TUBI×1；原始支管有四条 TUBE，复制结果只有一条，不能将复制看作原始全部状态相同。

| 快照 | 修改 | 原生 RVM | 自动发布 |
|---|---|---|---|
| catalogue-positive-clone | 初始副本 | 四个实体 | 完成 |
| positive-head-no-pspec | PSPE 置空 | 四个实体 | 完成 |
| positive-head-no-href | 在上一步基础上 HREF 置空 | 四个实体 | 完成 |
| positive-head-no-hcon | 在上一步基础上 HCON unset | 四个实体 | 完成 |
| positive-head-no-hstub | 在上一步基础上 HSTU 置空 | 头管消失，只剩三个实体 | 完成，但生成器仍输出四个实体，失败 |
| positive-head-restored-hstub | 只恢复 HSTU | 头管恢复，四个实体 | 完成 |

各快照保留原生导出、API 模型、水位和属性，没有调用 ensure。这是逐步消除条件，并非每次恢复原始状态的独立试验。它证明该正样本的 HSTU 有无影响头管存在；不证明所有路由都可加统一 HSTU 门槛，也未解释已有非空 HSTU 的原始 COUP/VALV 为什么没有 RVM 管段。

`positive-head-no-hstub/route-comparison.json` 明确失败：FTUB×2、BEND 的边界及体积通过，运行时多出的正常 TUBI 没有 RVM 对应项。运行时由到达口径回落生成直径 500 mm 粗管。

Plant UI 已加载并聚焦该支管，连续观察 generation 13/14/15；未点击重新生成。HCON 从 CRS 自动变为界面值 0；HSTU 置空后出现粗圆柱，恢复后回到细管。截图：

- `screenshots/positive-head-before-hcon.jpg`
- `screenshots/positive-head-hcon-unset-auto.jpg`
- `screenshots/positive-head-hstub-null-extra-tube-auto.jpg`（失败结果）
- `screenshots/positive-head-hstub-restored-auto.jpg`

恢复后的属性对账 `positive-head-restored-hstub/attribute-comparison.json`：HSTU、HREF、PSPE 按引用身份核对通过；HCON API 返回字符串 `"0"`、持久层为空字符串，数据一致性仍失败。不能以 UI 自动刷新或 ERROR 0 代替数据正确性。

#### HCON unset 修复复测

`element_attributes` 将零 WORD 且无有效文本识别为空值，API 保留与持久层一致的空字符串，同时返回 `is_unset=true`、`display=unset`。非空词值继续正常显示。新增零 WORD 回归先失败、修复后通过，属性模块三项测试通过，服务编译通过并部署为隔离 PID 53444。

真实复测脚本 `scripts/check_rocks8000_unset_word.py`：`hcon-unset-before-fix.json` 失败；`hcon-unset-after-fix.json` 和再次原生 OPEN→UNSET 的 `hcon-unset-increment-fixed.json` 均通过。OPEN 阶段 API 与 RocksDB 都为 OPEN，见 `positive-head-open-hcon-fixed/attribute-comparison.json`。两次增量均自动发布，未调用 ensure；各自 RVM/记录快照为 `positive-head-open-hcon-fixed` 和 `positive-head-clear-hcon-fixed`。旧失败记录保留，不追溯覆盖。

Plant UI 自动刷新完成 generation 17，HCON 显示 unset，四件支管模型仍显示；截图 `screenshots/positive-head-hcon-unset-fixed-auto.jpg`。日志 ERROR 0，仅作为本次刷新证据，不代表剩余类型全部通过。

#### 圆柱体积比较口径（诊断）

初始正样本的网格体积比较有 2.0077% 差异。新增 `scripts/audit_rocks8000_cylinder_tessellation.py` 独立分析原始 RVM Cylinder 参数及实际网格：半径 12.5 mm、长度 1040.0054 mm，运行时圆周 12 段，RVM 导出工具三角化 16 段。正多边形体积比例预测差异 2.0077776%，与实测 2.0077007% 吻合；运行时顶点半径最大误差 0.000496 mm、端面位置误差 0.000136 mm。

诊断保存在 `catalogue-positive-clone/cylinder-tessellation-audit.json`。未放宽原有体积容差、未覆盖失败记录；后续需明确解析曲面与离散网格各自的验收标准，不能把本诊断当作拓扑通过证明。

### 头管条件继续消除（后续会话 501 起）

在原始 VALV 支管 `24384/26640` 上逐步修改并独立导出：

- `catalogue-nonzero-head`：只改 HPOS 为 E10，RVM 仍无头管，排除仅由世界原点触发。
- `catalogue-long-head`：只改 HPOS 为 W1000，管段长 1120 mm，RVM 仍无头管，排除该长度范围内的短管解释。
- `catalogue-positive-stub-head`：换正样本 HSTU，RVM 仍无头管。
- `catalogue-native-matching-stub`：先原生查到真实 25 口径 BRAN `/Copy-of-1RCS023MN-YK/809VP` 使用 `/YK-SS/1PK61:YKS-AAAAABEFN325`，再采用该 stub；RVM 仍无头管。不能把原始失败归因于 stub 名义口径不匹配。

上述自动发布均完成。曾尝试未经查询确认的 `/YBS3/CIP00001:P,25`，E3D 异常退出，证据 `catalogue-matching-stub-head-edit` 保留；该尝试不计有效增量，后续改用原生查证引用。

正样本 `24384/26644` 的 `positive-head-bore25` 只将 HBOR 500→25，RVM 删除头管，另外三个实体不变；`positive-head-restored-bore500` 只恢复 HBOR=500 后头管返回。原生 Q ATT、API/模型记录、RVM 均存档。该证据确定 HBOR 参与头管存在判断，尚需区分与到达点口径、stub 口径的关系后再修改生成规则。

#### 已收敛：头部与首构件口径不匹配

进一步对照 `positive-head-stub-nominal25`：保持 HBOR 与首构件为 500，只把 HSTU 换成真实名义口径 25 的 stub，RVM 仍有头管，圆柱半径改为 16.7 mm。这排除了“stub 名义口径与 HBOR 不匹配就没有头管”的解释。

生成器现在将 ContainerHead 槽位两端口径不匹配的 TUBI 标记为无效诊断管段，沿用虚线显示路径；其他槽位未套用尚未验证的规则。模型库 242 项单元测试通过，服务编译并部署为隔离 PID 55268。

真实 `positive-head-bore25-fixed`：自动发布，无 ensure；三件正常实体与 RVM 的世界网格边界/体积对比通过，单独保留一件 `is_invalid_tubi=true` 的诊断管段。HBOR API 与 RocksDB 均为 25。Plant UI generation 25 显示虚线段和 HBOR=25，截图 `screenshots/positive-head-bore25-invalid-fixed-auto.jpg`；更新前截图 `screenshots/positive-head-bore500-before-fix-increment.jpg`。该通过仅覆盖这条已证实的头部口径规则，HSTU 置空和原始 VALV/COUP 额外管段仍未解决。

反向增量 `positive-head-bore500-fixed`：HBOR API/持久层均恢复 500，四个正常实体与 RVM 对比全部通过，无无效管段。头管边界误差 0.228 mm、网格体积误差 0.751%，均在原有阈值内；没有放宽比较标准。Plant UI generation 26 自动恢复实体管，截图 `screenshots/positive-head-bore500-restored-fixed-auto.jpg`。两个方向都未手动重新生成。

#### ORIL 未设置的头管规则

VALV `24384/26641` 和 COUP `24384/26643` 的 ARRI/LEAV 都为 1/2；原始 ORIL=false，而正样本 FTUB 的 ORIL=true。

- `catalogue-oriented`：VALV 只执行 `ORI Y IS N AND Z IS U`，ORI 仍为 [0,0,0]、POS 仍为 [120,0,0]，ORIL=true 后 RVM 出现头管。
- `catalogue-unoriented`：只执行 `ORIFLAG FALSE`，几何朝向保持单位朝向，RVM 头管再次消失。
- `catalogue-coup-oriented`：原始 COUP 只设置单位朝向；HPOS=0、管长 120、原有 HSTU（名义 7、OD 6.35）均保留，RVM 出现半径 3.175、长度 120 的头管。这直接排除之前针对该样本的原点、长度、stub 名义口径假设。

生成器针对 ContainerHead→MemberArrive 槽位检查显式 ORIL=false，标记无效诊断管段；构件本身继续生成，不把未知属性当 false。成员间/尾槽位仍需单独原生证据。242 项模型库测试通过，服务编译通过，隔离服务 PID 50820。

`catalogue-oriented-fixed` 已自动发布并通过两个正常实体的 RVM 世界边界/体积对比，阀门体积误差 0.0000061%，头管体积误差 0.7515%，没有放宽原有阈值。截图 `screenshots/catalogue-orientation-set-fixed.jpg` 同时显示实体头管、阀门、ORI=0,0,0 和 ORIL=true。

`catalogue-unoriented-fixed` 自动发布后仅有一件正常 VALV、一个无效诊断管段，正常实体与 RVM 完整匹配。ORIL/ORI/POS/NAME/BUIL/SHOP 六字段 API/持久层一致；Plant UI generation 31 自动将实体管变为诊断线、ORIL 显示 false，截图 `screenshots/catalogue-orientation-unset-fixed-auto.jpg`。

`catalogue-coup-unoriented-fixed` 对原始短 COUP 同样通过：一个正常 COUP、一个无效诊断管段，RVM 仅 COUP；同六字段一致。自动发布完成后在 UI 加载并聚焦 COUP、隐藏重叠 VALV，截图 `screenshots/catalogue-coup-unoriented-fixed.jpg` 显示通孔和 ORIL=false。

随后用同一 COUP 连续执行专门的自动发布回归，全程未调用 ensure。会话 538 将 ORIL 置 true，RocksDB/API 自动收敛，RVM 的 COUP 与实体头管两项均通过；会话 539 置 false 时，数据窗口已入队，但唯一 worker 正在全局房间积压的一页内逐构件计算，88 秒后看门狗记录 `dispatcher_not_claiming_eligible_row`。当时 `queue_paused=false`、worker 存活，水位停在 `applied=538/file=539`，因此此前会话 491 的同类发布问题得到稳定复现。

修复后，全局空闲房间轮在每个面板或构件任务完成后检查数据队列并让位；批次内部 scoped 房间收敛仍保持原子执行。重启先从同一 RocksDB store 恢复并发布会话 539；会话 540 在同一进程中自动发布 ORIL=true，RVM 两项通过。随后在 195 个房间目标的全局轮运行期间写入会话 541：房间轮处理 96 个目标后立即 partial 收口，数据批次启动并在 3532ms 内完成，水位推进到 541。最终 ORIL=false 状态仍为一个正常 COUP、一个无效诊断管段，RVM 只含 COUP 且比较通过。证据目录为 `coup-publication-regression-{oriented,unoriented-fixed,hot-fixed,preempt-unoriented-fixed}`，调度单元测试 134 项通过。

### 剩余目录覆盖清单

#### HSTU 空引用头管增量回归

补充原生对照 `positive-head-connected` / `positive-head-connected-no-stub`：恢复原 HREF `/LV-CO-1RX-3062D` 后，仅清空 HSTU，RVM 从四个实体变为三个。该 HREF 非空不代表克隆支管已建立双向连接。修复前比较脚本明确失败：多出一个正常 TUBI；失败 JSON 保留。

生成器仅对 BRAN 的 ContainerHead 检查存储的 HSTU/HSRO 引用；明确无 stub 时标记 `head stub unset` 诊断线。非空引用的目录解析失败不被当成空引用，原外径回落路径保留。未扩展至成员间、尾管或其他容器类型。模型库 242 项单元测试与服务构建通过，隔离服务更新为 PID 1916。

真实增量 `positive-head-stub-restored-fixed` 恢复 HSTU 为 `13246/524994`，四个正常实体与 RVM 世界网格边界/体积全部匹配；`positive-head-stub-cleared-fixed` 再清空 HSTU，三个正常实体全部匹配、另有一个无效诊断管段。比较阈值仍为 2 mm / 1%。两阶段 HSTU、HREF、PSPE、HBOR、HCON 的 API 与 RocksDB BRAN 数据均通过显式期望值断言，证据为各目录 `stub-attribute-comparison.json`。

Plant UI 同一支管连续自动刷新至 generation 35 / 36：细实体头管变为诊断线，HSTU 从 `13246/524994` 变为 `0/0`，没有点击重新生成。已存档 `screenshots/positive-head-stub-restored-fixed-auto.jpg` 和 `screenshots/positive-head-stub-cleared-fixed-auto.jpg`。此项通过不关闭历史发布失败、目录备份缺口及剩余类型覆盖问题。

`remaining-catalogue-detail` 已保存剩余 39 个有样本 noun 的原始 E3D 属性与所属库文件。样本均来自私有 AMS 的 5052/5053/6890/7000 库。目录接口的树查询存在 ref0→dbnum 缺失而返回 500 的情况；属性接口可读取这些样本。

`remaining-catalogue-map` 初版仅面向 CATE，尚未覆盖 STCA/GMSS/NGMS 和独立 GMSE，遍历到 0/0 的错误是该发现脚本的边界问题，不应计作这八类源数据损坏。其余样本的 SCOM 和原生 SPCO 查询保存在 `catalogue-spco-probe`、`catalogue-spco-by-gmse`；多数最先找到的样本没有现成 SPCO 引用，仍需构造合法实例或选择已使用样本。未计全部通过。

直接把设计 VALV 的 SPRE 设置为 SCOM `/SBCL150RFDD` 导致 E3D 退出；已用原生 SPCO 链路替代，失败保留于 `catalogue-valve-select`。

### 多图元阀门新增与移动（通过）

原生查询确认 SPCO `13245/643605` `/NAC-B/A0Ph33VNN0-D:25x25` → SCOM `13245/524863` `/A0Ph33VNN0FF`，GTYP=VALV，GMRE=`13245/524827`。目录有 18 SCYL、2 LSNO、1 SLINE、1 SBOX、2 SCON；当前表示配置下 RVM 实际导出 18 Cylinder + 2 Snout，不能把未导出的类型计作实体几何通过。只读证据在 `remaining-spec-native-detail-components`。初次完整属性/引用跳转查询及重试使原生进程退出 3221225477；缩小为显式名称和所需字段后成功，两个失败目录保留。

创建私有 BRAN `24384/26652` `/CODEX_TYPES_MULTI_BRAN`，VALV `24384/26653` `/CODEX_TYPES_MULTI_VALV`。`multishape-valve-created`（源会话 520）自动发布一个正常阀门、一个无效管段；正常阀门世界包围盒误差 0.000008 mm，但修复前聚合网格体积与 RVM 子件求和相差 26.9516%，`route-comparison.json` 为 FAIL。RVM 各子件有向体积同号，没有完全相同参数与矩阵的重复项，排除了符号抵消和完全重复件。

根因是目录生成器仅对 ATTA/特定 CLFL 程序保留独立 `parts`，普通 VALV 先经 `Manifold::compose` 合并了相交子件；RVM 则把可见目录图元作为独立显示组件导出。先把 `visible_catalogue_programs_keep_independent_components` 改成所有可见目录程序的契约并得到红测，再令生成器保留过滤后的 `solids` 为独立 `parts`，同时保留 `solid` 合成结果供需要单一 Manifold 的调用方使用。该测试转绿，`e3d-model` 全部 242 个库测试和根服务构建通过。

`multishape-valve-moved` 只移动 N 700、设置 SHOP/BUIL=true，自动发布成功。创建与移动阶段各七项属性（NAME/POS/ORI/ORIL/SHOP/BUIL/SPRE）均与 API、RocksDB VALV 表一致。`model-delta-comparison.json` 证明实际模型平移 [0,700,0]，RVM 包围盒同样平移；本地顶点最大差 0、拓扑和实例变换完全一致。初版要求 geo_hash 相同而失败，已保留 `model-delta-hash-initial-failure.json`；修正为对实际顶点、拓扑的断言后位置增量通过，哈希不同仍作为观察项。

部署修复后的服务后，再执行 `multishape-valve-moved-fixed`：把位置增量到 `[5000,1400,0]`，并把 SHOP/BUIL 恢复为 false。官方比较器在原阈值下通过，世界包围盒误差 0.000008 mm，显示组件体积误差 0.9753%（阈值 1%）；七项属性再次与 API 和 RocksDB 一致。固定种子三方向 600 条射线的附加诊断仍为 597/600：`cylinder-phase-audit.json` 证明相关 RVM 圆柱圆周为 23 段且有相位偏移，运行时为 16 段、相位 0，因此三条近切线射线受离散化差异影响。这项附加诊断不替代已经通过的包围盒/显示体积验收，也没有修改正式阈值。

Plant UI 自动刷新到 generation 39，ERROR 0。树中展开并单独聚焦 `/CODEX_TYPES_MULTI_VALV` 后，界面显示最终阀门外形以及 `POS=5000,1400,0`、`ORIL=true`、`BUIL=false`、`SHOP=false`、`SPRE=13245/643605`。无桌面遮挡的证据已归档为 `screenshots/multishape-valve-moved-fixed-auto.jpg` 和 `screenshots/multishape-valve-moved-fixed-oril-auto.jpg`。本节只计 RVM 实际可见的 18 SCYL + 2 LSNO；未导出的 SLINE/SBOX/SCON 仍不计覆盖。

### AMS8000 当前设计 SPRE 反查与三组合法实例

`scripts/equi8000_live_spco_inventory.py duct` 从 RocksDB 当前设计的 SPRE 反查原生 SPCO→SCOM→GMSET，在 dbnum 13244 的 79 个 SPCO 中成功解析 79 个 SCOM、46 个 GMSET。可见目录程序覆盖 LINE、SEXT、SREV、SCTO、SBOX；完整映射保存在 `remaining-live-spco-inventory-duct/inventory.json`。宽范围探测遇到三个会使原生 E3D 退出的引用，失败日志按批次保留，正式用例只采用缩小后可重复成功的引用。

#### LINE + SEXT + SREV BEND

复制真实 `/C-OR-1R332-B` 为 BRAN `24384/26654` `/CODEX_TYPES_REMAIN_BEND`，目标 BEND `24384/26656` 使用 SPRE `13244/108845`，其目录程序包含 LINE×22、SEXT×10、SREV×5。创建快照 `remaining-bend-created-fixed` 的五个实体全部通过 RVM 世界边界和体积比较；初次捕获误把首个 FTUB `24384/26655` 当作根而超时，`remaining-bend-created/publication-timeout.json` 是测试夹具选根错误，服务实际已应用会话 523，不能记作发布失败。

目标上移 U350 并设置 SHOP/BUIL=true 后，`remaining-bend-moved` 的五个正常实体继续全部通过，目标包围盒精确平移 `[0,0,350]`，geo_hash 不变；七项 NAME/POS/ORI/ORIL/SHOP/BUIL/SPRE 在 API 与 RocksDB BEND 表一致。移动导致两段连接管失配，RVM 删除它们，运行时以两条无效诊断线保留，未计实体比较。再将 SHOP/BUIL 恢复 false，`remaining-bend-restored-fixed` 仍通过同一五实体比较，位置保持 U3000、geo_hash 不变，API 与持久层同步回退。

Plant UI 自动刷新至 generation 41/42，目标目录模型、`POS=-4291.87,-9373.71,3000`、`SPRE=13244/108845` 及 SHOP/BUIL 的 true→false 更新均可见，ERROR 0。截图：`screenshots/remaining-bend-moved-auto.jpg`、`screenshots/remaining-bend-moved-spre-auto.jpg`、`screenshots/remaining-bend-restored-auto.jpg`。

#### LINE + SEXT + SCTO BEND

复制真实 `/C-OR-1R332-H` 为 BRAN `24384/26660` `/CODEX_TYPES_REMAIN_SCTO`，目标 BEND `24384/26661` 使用 SPRE `13244/109849`。创建和向 E 移动 275 mm 后的 `remaining-scto-created`、`remaining-scto-moved` 均通过全部两个正常实体的 RVM 世界边界与体积比较；移动后目标包围盒精确平移 `[275,0,0]`，geo_hash 不变。NAME/POS/ORI/ORIL/SHOP/BUIL/SPRE 在 API 与 RocksDB BEND 表一致。移动后的两条断开管段仅作为无效诊断线，不计实体。

Plant UI generation 46 中目标 BEND 可单独显示并定位，界面显示 `POS=-8727.57,-11486.66,70`、`ORI=0,0,-145`、`SPRE=13244/109849`、SHOP/BUIL=true，截图 `screenshots/remaining-scto-moved-auto.jpg`。

#### SBOX + SEXT ATTA（数据型附件）

复制真实 `/C-GR-1RX-3000A` 为 BRAN `24384/26663` `/CODEX_TYPES_REMAIN_SBOX`，目标 ATTA `24384/26675` 使用 SPRE `13244/108749`，目录 GMSET 包含 SBOX×1、SEXT×8。向 N 移动 225 mm 并设置 SHOP/BUIL=true 后，NAME/POS/ORI/ORIL/SHOP/BUIL/SPRE 在 API 与 RocksDB ATTA 表全部一致，位置为 `[-6988.92,21915.61,2200]`。

该 ATTA 在创建和移动两阶段都没有独立模型记录，AMS8000 原生 RVM 也不导出该目标；因此它按数据型附件记为数据增量通过，不把其 SBOX/SEXT 目录程序记作实体几何通过。所属支管其余六个正常实体在两阶段均通过 RVM 比较。Plant UI generation 46 选中该 ATTA 后显示上述位置、`SPRE=13244/108749`、SHOP/BUIL=true；截图 `screenshots/remaining-sbox-moved-auto.jpg`。

#### RocksDB 冷重启复核

停止服务与 RocksDB 进程后，使用同一 store 启动新 RocksDB PID 58072 和服务 PID 38408。健康检查返回 ready，`sul_db.medium=external`、`durable=true`、会话 8000/529、模型待办与死信均为 0。冷启动直接读取三根持久模型并重新导出原生 RVM：`cold-restart-bend` 五个实体、`cold-restart-scto` 两个实体、`cold-restart-sbox` 六个实体全部再次通过；重启后的 API 与 RocksDB 属性断言也全部通过。

BEND/SCTO 根的 `published_target` 保持在各自最后受影响的旧会话，晚于它们的 ATTA 更新只推进了整个文件会话。通用增量捕获器因此留下 `publication-timeout.json`。冷启动比较器以 `--cold-restart` 明确区分持久快照复核，并在结果中写入 `root_watermark_behind_unrelated_file_session=true`；它仍对实际持久记录逐件执行 RVM 边界和体积比较，没有删除或覆盖该水位证据。

### 布尔重型 VALV（通过）

dbnum 13245 的合法规格 `13245/851590` `/VMB1/LVGH22XUY0-D1:50x50` 对应 GMSET `/LVGH22XUY-G`，包含 LCYL×11、LINE×1、LSNO×6、SBOX×3、SCON×2、SCTO×6、SCYL×6、SSPH×1。复制其真实所属支管为 `24384/26679` `/CODEX_TYPES_REMAIN_BROAD`，目标 VALV 为 `24384/26685` `/CODEX_TYPES_REMAIN_BROAD_VALV`。自动发布完成，运行时记录 `has_neg=true`，Plant UI generation 48 可显示并定位阀门，属性 `POS=-2685.11,-8478.79,334.34`、`SPRE=13245/851590`；诊断截图为 `screenshots/remaining-broad-created-ui-diagnostic.jpg`。

初版比较器把 RVM 解析图元的三角网格体积直接当作曲面体积，圆柱、锥台、圆环等离散段数不同时会产生假失败。比较器现按 RVM 参数计算 Box、Cylinder、Snout、Circular/Rectangular Torus、Pyramid、Dish 的解析体积，并对每个实体采用三角网格与解析口径中更接近运行时离散方式的一项。验收阈值没有放宽。

`remaining-broad-created/route-comparison.json` 的 14/14 个正常实体全部通过；目标 VALV 外边界最大误差 0.174 mm，解析体积误差 0.253%。向 U 移动 325 mm 后，`remaining-broad-moved-again` 再次通过全部 12 个正常实体，目标 AABB 精确平移 `[0,0,325]`，NAME/POS/ORI/ORIL/SHOP/BUIL/SPRE 在 API 与 RocksDB 中一致。纯平移前后目录局部顶点、拓扑和实例变换完全相同。

进一步按目标 VALV 本身拆分 RVM，移动前后都稳定导出 19 个可见图元：Cylinder×8、Snout×6、Box×3、CircularTorus×2。因此 SBOX 已由三个原生 RVM Box 直接覆盖。原生目录属性证明 SLINE 与 SCON 都是 `CLFL=true、TUFL=false`；在本测试固定的 `TUBE ON / CL OFF / OBST OFF` 表现设置下，两者按 AVEVA 显示规则应被过滤，不应计为缺失实体。目录负体也不能从原始 RVM 正图元列表中单独认领：运行时记录明确 `has_neg=true`，PlantMesh 已烘焙目录切割，而 RVM 文件保留的是正显示图元。把 19 个 RVM 正图元简单求并与 PlantMesh 做 600 条射线比较时，移动前后都固定有 4 条差异；该结果作为 RVM 口径限制保留，不能记成逐射线通过。

多部件闭合审计使用 `closed_component_assembly` 口径：移动前后边界边均为 0，共边虽有四面相接，但所有边重数为偶数且正反方向完全配对，结果通过。自动断言为 `scripts/check_rocks8000_broad_boolean_coverage.py`，Plant UI 冷启动截图为 `screenshots/cold-restart-broad-boolean-plant-ui.jpg`。

为使目录烘焙网格身份只由局部几何决定，`e3d_model_service` 在计算哈希前规范化接近零的局部顶点。移动回原位和再次移动后 geo hash 保持稳定；针对性测试 `pure_translation_keeps_baked_catalogue_mesh_id` 通过。Plant UI generation 49 的归档截图为 `screenshots/remaining-broad-moved-again-auto.jpg`。

### REDU 数据型目录成员（通过）

复制真实分支并创建 REDU `24384/26698` `/CODEX_TYPES_REMAIN_REDU_TARGET`，SPRE=`13244/109009`。目标移动到 `[-8409.86,13651.54,3600]` 并设置 SHOP/BUIL=true 后，七项属性在 API 与 RocksDB REDU 表中完全一致，水位自动推进，没有调用 ensure。

该 REDU 在创建和移动阶段均无独立模型记录，原生 RVM 也不导出该目标，因此按数据型成员验收。所属分支其余五个正常实体的 RVM 对比全部通过，最大边界误差 0.00130 mm。Plant UI 已选中目标并显示 REDU、refno、POS、SHOP/BUIL，截图为 `screenshots/remaining-redu-data-increment-plant-ui.jpg`。

### SCTN 直属 FITT（通过）

真实 FITT `24384/24949` `/-SR-CSV-S5001-V1/F1` 位于 SCTN `24384/24948` 下，SPRE 指向 SPCO，再直接以 SFIT 的 GMRE 结束目录链。FITT 不是独立模型单元；数据与模型增量都上卷到 SCTN。生成器现允许目录链以含 GMRE 的 SFIT 结束，并在 SCTN builder 中收集直属 FITT 几何。

原生 RVM 证明结构安装标架为 `SCTN world × translate(POSS) × Rx(90°) × Rz(FITT.BANG)`。BANG=0 的在线 AABB 与 RVM 最大误差 0.00175 mm；BANG=15° 时最大误差 0.00134 mm，体积均为 1,458,000 mm³。真实三阶段会话 535→536→537 中，BANG/SHOP/BUIL 分别为 `0/false/false`、`15/true/true`、`0/false/false`，API、RocksDB 和源会话完全一致。旋转时 geo hash 改变，恢复时逐字回到基线。自动断言见 `scripts/check_rocks8000_remaining_fitt.py`。

Plant UI 已展开 SCTN 路径并选中 FITT，右侧属性显示 BANG=0、BUIL=false、SHOP=false，视口显示所属结构模型。截图为 `screenshots/remaining-fitt-restored-plant-ui.jpg`。

在会话 537 后再次停止服务和 RocksDB，并以同一 store 启动 RocksDB PID 36532、服务 PID 62668。服务直接恢复会话 537，`sul_db.medium=external`、`durable=true`，模型待办、数据/房间阶段死信及副作用待办均为 0；过程中没有调用 ensure 或模型重新生成接口。`cold-restart-fitt` 从恢复后的模型库、FITT 属性表和原生 RVM 重新取证，BANG/SHOP/BUIL 仍为 `0/false/false`，geo hash 与重启前完全相同，AABB 最大误差仍为 0.00175 mm。`scripts/check_rocks8000_remaining_fitt.py` 已把该阶段纳入自动断言。Plant UI 冷启动复核截图为 `screenshots/cold-restart-fitt-plant-ui.jpg` 和 `screenshots/cold-restart-redu-plant-ui.jpg`；前者显示 FITT `24384/24949` 及恢复后的属性，后者显示 REDU `24384/26698` 的数据增量结果。

### 布尔挖孔增量回归（通过）

`compare_equi8000_boolean.py` 重新核对无孔、开 300 mm 孔、孔平移、扩到 500 mm、填孔五个阶段。PlantMesh、原生 RVM 与解析体积的相对误差均小于 `1e-6`；孔内射线为 0 次命中，孔外射线为 2 次命中，五阶段全部通过。对应证据为 `boolean-baseline`、`boolean-hole-regenerated`、`boolean-move-result`、`boolean-widen-observed`、`boolean-filled-result`。

严格拓扑审计另外覆盖开孔、移动、扩孔三个有孔状态：每个 PlantMesh 都是单一连通闭合实体，边界边 0、非流形边 0、Euler 特征 0、genus=1。该检查保留单体孔洞的严格标准，与上面的多部件目录装配口径分开。

### 最终回归

`e3d-model` 全部 242 个库测试通过；BEND、SCTO、SBOX、VALV、REDU、FITT、HSTU 空引用、unset WORD 和布尔孔比较器均通过。房间轮数据抢占修复后的 `model_update_pending` 单元测试 134 项通过，主服务构建成功。最终服务仍运行在外部 RocksDB 模式，会话 541、durable=true，数据、模型、房间待办与死信均为 0；COUP 的连续 ORIL true→false、服务重启恢复以及带房间积压的热抢占均已取得 API、RocksDB 与 RVM 证据。
