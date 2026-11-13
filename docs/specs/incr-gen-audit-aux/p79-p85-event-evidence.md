# P79–P85 的事件级证据：离线净窗口 + 运行时采集器

七个案例（`P79`–`P85`）的 `blocked_reason` 一律含 `requires-db-userchanges-event-observation`。
本文记录 2026-10-29 这一轮为拆掉这个阻塞做的两件事，以及**它们各自能证明什么、不能证明什么**。

## 1. 离线净窗口：七个案例的窗口操作流

用生产收集器（`IncrementPipeline::collect_window`，e3d-io 底座净窗口）与
`net_window_probe` 对每个案例的 TTY 窗口重放一遍，操作流如下。窗口取自各案例
`summary.json` 的 `baseline.captured_sesno` → `apply_sesno`，因此这**就是案例自己那次
SAVEWORK 提交的内容**，不是另一份语料。

| 案例 | 窗口 | Add | Modified | Deleted |
|---|---|---|---|---|
| `P79` | 740→743 | `EQUI 24384/26555`、`BOX 24384/26556` | `ZONE 24384/26552`(MEMBERS)、`DBMDAT 24384/26333`(DBMVAL) | — |
| `P80` | 751→754 | `EQUI 24384/26561` | `ZONE 24384/26558`(MEMBERS)、`DBMDAT`(DBMVAL) | — |
| `P81` | 767→770 | `EQUI 24384/26572`、`BOX 24384/26573` | `ZONE 24384/26569`(MEMBERS)、`DBMDAT`(DBMVAL) | — |
| `P82` | 812→815 | `FTUB 24384/26666` | `BRAN 24384/26656`(CACHID,MEMBERS)、`DBMDAT`(DBMVAL) | — |
| `P83` | 823→826 | — | `BRAN 24384/26670`(CACHID,MEMBERS,SPAMAP)、`DBMDAT`(DBMVAL) | `FTUB 24384/26671` |
| `P84` | 834→837 | — | `BEND 24384/26695`(SPAMAP,SPRE)、`BRAN 24384/26693`(CACHID)、`DBMDAT`(DBMVAL) | — |
| `P85` | 845→848 | — | `BEND 24384/26708`(SPAMAP,SPRE)、`BRAN 24384/26706`(CACHID)、`DBMDAT`(DBMVAL) | — |

原始回执：`output/dc-events/<case>-netwindow-<base>-<target>.json`（`net_window_probe`）与
`output/dc-events/<case>-parity-<base>-<target>.json`（`increment_planner_parity`，
含 e3d-model 侧的 L3 计数行）。

### 这些数字坐实了什么

- **`P79` / `P81` 的 created 祖先确实在窗口里**：Add 有两条（容器 + 子件），且 owner 的
  `MEMBERS` 变化被单独记进 Modified。`P81` 的改名没有在净窗口里单独出现属性行——
  因为子件是窗口内新建的，两端 diff 只看到终态 `-CHILD-RENAMED`，**改名被 created 折叠**。
  这正是 `elementCreated` 走 `isElementCreated` 去重时该有的外部表现。
- **`P80` 的 child 建后又删在净窗口里完全消失**：窗口只有 owner 的 `Add`，没有 child 的
  `Add`，也没有任何 `Deleted`。`fold_net_op` 的 `(Added, Delete) → Cancelled` 规则在这
  个案例上真实发生了。这与案例预期「删除新建容器的子件不产生生产删除行」一致，但
  **净窗口的折叠是实现层行为，不是 core 的 `DB_UserChanges` 记录**——core 那边是
  `elementDeleted` 里 `isElementCreated` 命中后只清 include root，两件事在同一个方向上
  但不是同一份证据。
- **`P82` 是成员集 +1，不是纯换序**：`BRAN` 的 Modified 带 `MEMBERS`，同时有一条
  `FTUB` 的 `Add`。净窗口的 `classify_children_delta` 判 `MemberChanged`（集合大小变了）。
  这**排除了「只有成员个体换序」**这一读法，但**没有**给出 core 内部 `v28 == 3` 那个
  比较码——core 的 `elementsChangedBetween` 在 `primaryList` 为真时会逐成员比较，
  码值 3 才升级成 `elementReordered`；净窗口的集合差只能说明"集合变了"，说明不了
  "core 是否也对某个成员发了 reorder"。
- **`P83` 是成员删除 + 一条普通 `Deleted`**：`FTUB 24384/26671` 在 base 存在、target 不存在，
  且它是窗口前已存在的元素（不在本窗口的 Add 里），因此按 `elementDeleted` 的语义应
  产生一条普通 deleted 行。`BRAN` 同时带 `MEMBERS`。RVM 侧那次「`M1` 槽位变成无名
  `TUBE 1`」已确认为导出器物化行为：三份快照 `members`/`geometries` 恒为 13/9，
  `M2`–`M9` 的 AABB 逐位相同，只有被删槽位的 AABB 从 Box(8536.56…9393.56) 变成
  Cylinder(8544.01…9387.11)。**按名对拍会误报，必须按几何与 AABB 判。**
- **`P84` / `P85` 的 `SPRE` 写入在净窗口里只留下一条属性行**：两案例的 `BEND` 都是
  一条 Modified 带 `SPAMAP,SPRE`。`P85` 在 TTY 里写了两次（`TBR90:50X150` 再
  `TBL90:50X150`），净窗口只看到终值——这与「同窗口多次写同一 qualifier，最终只保留
  一项」一致，但 `combine` 到底合并了几条、`DB_UserChanges` 留了几条记录，
  净窗口结构上就看不见（它是两端 diff，天生只有终态）。

### 结论：净窗口不能替代事件观测

净窗口是 `IndexDiff(base@S, target@T)` 的终态差分。`DB_UserChanges` 是 core 在一次
`SAVEWORK` 内**逐次调用**累积的桶。两者的差别恰好就是七个案例要测的那部分：

| 案例 | 净窗口能给的 | 只有事件能给的 |
|---|---|---|
| `P79` | 容器+子件都新建 | child 是否**重复**记了 owner（去重） |
| `P80` | child 的建+删被折叠掉 | deleted 是否进了 created 分支（不产生生产删除行） |
| `P81` | 终态是改名后的名字 | 改名是否只更新 created ref、不建普通 qualifier map |
| `P82` | 成员集 +1 | 是否**没有**任何成员被判 `v28 == 3` |
| `P83` | 成员集 −1 + 一条 deleted | 同上，且 deleted 是不是普通行 |
| `P84` | `SPRE` 终值 | comb key 是否从"属性+qualifier"切到"属性+空" |
| `P85` | `SPRE` 终值 | `combine` 合并成几项、留几条记录 |

## 2. 运行时采集器：`scripts/e3d/increment_suite/tools/dc_events_probe.py`

既然净窗口不够，就必须挂到 core.dll 上看真实调用。采集器已落地，纪律与
`E:\reverse\e3d\e3d-license-hook\m66_hook_pmlcommand.py` 一致：**只 `Interceptor.attach`，
不调用目标函数、不替换实现、不改返回值、不写进程内存。**

挂钩点（全部是 core.dll 导出，mangled 名交给 loader 自己解析）：

| 事件 | 导出 | 记什么 |
|---|---|---|
| `elementCreated` | `?elementCreated@DB_UserChanges@@QAEXABVDB_Element@@@Z` | `elem` refno |
| `elementDeleted` | `?elementDeleted@DB_UserChanges@@QAEXABVDB_Element@@@Z` | `elem` refno |
| `elementIncluded` | `?elementIncluded@DB_UserChanges@@QAEXABVDB_Element@@0@Z` | `elem`（被搬元素）、`elem2`（新 owner） |
| `elementReordered` | `?elementReordered@DB_UserChanges@@QAEXABVDB_Element@@@Z` | `elem` refno |
| `attributeModified` | `?attributeModified@DB_UserChanges@@QAEXABVDB_Element@@PBVDB_Attribute@@@Z` | `elem`、`attr` 名 |
| `attributeModified`（带 qualifier 的重载） | `?attributeModified@DB_UserChanges@@QAEXABVDB_Element@@PBVDB_Attribute@@ABVDB_Qualifier@@@Z` | 同上 + `qualified=true` |
| `combine` | `?combine@DB_UserChanges@@QAEXABV1@@Z` | 调用次数（P85 的 batch 边界） |

`DB_Element` 的 refno 偏移取 `+0x0C` / `+0x10`，已由 ida-bridge 在
`elementCreated` 的汇编里坐实（`mov eax,[edi+0Ch]` / `mov [esi+3Ch],eax`），
详见 [`db-userchanges-layout-calibration.md`](./db-userchanges-layout-calibration.md)。
注意 `__thiscall` 下 `this` 在 `ecx`，`onEnter(args)` 的 `args[0]` 才是第一个栈参数
（即 `DB_Element const&`）。

用法：

```powershell
# 另开一个窗口，在 runner 起 TTY 之前先挂上（探针会等进程出现）
python scripts/e3d/increment_suite/tools/dc_events_probe.py `
  --out output/dc-events/p84-live.jsonl --process des.exe --timeout 900
```

退出码：`0` = 至少抓到一条事件；`2` = 连上但零事件；`1` = 等不到进程。

### 现状：采集器已就绪，但这一轮没跑成

本轮所有 TTY 窗口跑完（`20261029-p79-rvm4` … `20261029-p85-rvm`，`teardown_sesno` 到 853）
之后，AMS 库文件被 `aios-database.exe`（pid 70192，2026-10-28 23:55 起、CPU 5756s）持续独占，
`ams8000_0001` 与伴生 `ams7998_0001` 都打不开写句柄。抢 TTY 需要库空闲，因此**这一轮
没有产出 `dc_events` 实跑证据**。七个案例的状态因此保持不变：

- `P79`、`P81`–`P85`：`calibrate`，`requires-db-userchanges-event-observation`
- `P80`：`calibrate`，`requires-db-userchanges-event-observation;rvm-omits-empty-containers`

要收口，下一次库空闲时按这个顺序做一遍即可（每个案例一条）：

1. 起 `dc_events_probe.py`（后台），确认打出 `_installed count=7 total=7`；
2. 跑 `Export-AmsIncrementCaseRvm.ps1 -Mode run -E3dInstallDir 'E:\reverse\e3d\shadow_e3d31_gen_model_test'`；
3. 拿 jsonl 里的 `elementCreated/elementDeleted/...` 序列去对 `case.json` 的 `dc_events`
   与 `forbidden`，两条都对上才把 `status` 从 `calibrate` 改 `passed`。

`P84`/`P85` 还要多看一眼：`attributeModified` 的两个重载谁被调用了。`SPRE =0/0` 若走
无 qualifier 的重载，说明 comb key 是"属性+空"；若走带 qualifier 的重载而 qualifier 为空
对象，说明是"属性+空 qualifier"，两者在 `combine` 的 map 键上不同。分支判据已在
`attributeModified(qual)` 里定位到：`0x059872c8 cmp eax,1` 就是
`DB_Qualifier::stringQualifiers().size() == 1` 的判定点，`size==1` 与 `size!=1` 两条路
往 `this+0x28` 的 qualifier map 插不同形状的键。

`P82`/`P83` 的 `ATT_MEMB` 分流也已在汇编里确认：`attributeModified(qual)` 的
`0x05987176 cmp eax, ATT_MEMB` 命中后跳到 `loc_59873CF`，直接调
`DB_UserChangesDependency::getDependencies` —— 走依赖表，与普通属性的 qualifier map
是两条独立路径。`elementReordered` 只在 `elementsChangedBetween` 的成员逐条比较、
码值 `== 3` 时被调用，是另一个入口。
