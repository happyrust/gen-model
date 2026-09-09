# 计划：历史模型的跨库时点（让「某一时刻的模型」名副其实）

状态：待评审（Plannotator）
日期：2026-09-08
影响仓库：`old/gen-model`（主要改动）；`old/vendor/e3d-io`、`old/vendor/e3d-model` **只读复用，本计划不改它们**

---

## 0. 目标

今天 `POST /api/v1/model/history/generate` 已经能按 `sesno` 或 `time` 生成历史模型，但它产出的实际是：

> **这一个设计库的会话 N × 今天的目录库 × 今天的 MDB 成员**

本计划把「时点」从「目标设计库的一个数」升级成「**一个可跨库对齐的口径**」，并且让回执把**每个库真正钉在哪一会话**说清楚——现在回执里只有一个 `resolved_session`，读的人无从知道目录库其实是最新的。

**不做**：历史 MDB 成员重建（§5 Q3）；单元级历史增量；把历史快照落库（它现在只进 `ProjectionScope::Historical`，这一点保持）。

---

## 1. 地基

### 1.1 底层能力已经齐了（**读码所得，非本轮实测**）

| 层 | 入口 | 状态 |
|---|---|---|
| 会话定位 | `e3d_io::session::SessionSelector::{Latest, Exact, AtOrBefore}`、`DbView::open_at`、`ReadOnlyEngine::open_at` | 有。索引写时复制，会话页自带那一刻的索引根，`open` 就是 `open_at(Latest)`，**只有一条实现** |
| 每库时点 | `DbFilePin.sesno: Option<u32>` → `OpenDb::open` → `open_at`（`e3d-io/src/db_element.rs:44,152`） | 有，而且**粒度本来就是每个库文件一个** |
| 生成 | `e3d_model::pipeline::generate_subtree(root: DbElement)`（`e3d-model/src/pipeline.rs:306`） | 只吃一个句柄，会话是继承来的。**历史与当前走同一条管线**，几何层没有一行历史专用代码 |
| 时刻解码 | `data_interface::db_file_header::SessionStamp::local_time`（`db_file_header.rs:57`） | 有。沿用 old-pdms-io `get_dt`：字0=年、字1=月、字2=日×24+时、字3=分×60+秒，按**本机时区**解释 |
| 服务面 | `fast_model/historical_model.rs` + `/api/v1/model/history/{generate,query,{key}}` | 有。`SessionSelector::{Latest, Sesno, At}` 三臂齐全 |

**结论：缺的不是能力，是把每库时点填满的那一层。**

### 1.2 缺口集中在一个函数

`fast_model/e3d_model_service.rs:1394` `build_set(target_dbnum, target_sesno)`：

- CATA 库 `continue` 跳过预装，交给 `E3dDbResolver` 懒解析；
- 而 `E3dDbResolver::resolve`（同文件 `:435`）查不到 `sessions` 表就 `DbView::open(&file)` 取**文件最新会话**并缓存；
- 其余 DESI 库用各自的 `pin.sesno`，与请求的时点无关。

所以历史请求只影响 `target_dbnum` 一个库。

### 1.3 会话链有洞是常态（**转引 e3d-io 实测**，`vendor/e3d-io/docs/plans/2026-09-03-sesno-range-increment.md` §1.4）

```
ams7000_0001   1, 237, 238, 262, 263        （5 条）
ams5052_0001   1..7, 163..189               （34 条）
ams7351_0001   1, 2, 116                    （3 条，1.18 GB）
ams8000_0001   1..281                       （无洞）
```

`MERGE CHANGES` 会回收旧会话。历史模型今天走的是 `ReadOnlyEngine::open_at(path, *sesno)` 即 `Exact` 语义，**被 merge 掉的号直接报错**，不就近。e3d-io 有 `AtOrBefore` 这条臂专门干这个，只是这条路没接。

### 1.4 还没测、P0 必须先测出来的四件事

这四件事会直接改写 §2 的设计，**不测就动手是赌**：

1. **各 CATA / DESI 库的会话页时间戳解不解得出来。** `SessionStamp::local_time` 对字段全零的页返回 `None`。跨库按时刻对齐这件事成不成立，全看解得出的比例。
2. **同一时刻分别 at-or-before 落到的会话，会不会出现「目录比设计新」的倒挂。** 各库保存节奏不同，时刻单调不代表落点合理。
3. **目录库钉在旧会话之后，属性还抽不抽得出来。** `TemplateProvider` 读的是外部 `*vir.dat`，不随会话变，**预期不受影响**——但目录求值链（`SPRE→SPCO→CATR→SCOM→GMSE/PTSE`）跨的是元素，旧会话缺件会怎样，要坐实。
4. **产物差异基线。** 同一 refno，「设计库钉旧 + 目录最新」 vs 「两边都钉旧」，差多少件、差在哪。这是本计划唯一的价值证明；差为 0 就说明这活不值得做。

---

## 2. 设计

### 2.1 请求面：`time` 的语义从「选一个 sesno」升级成「一个跨库口径」

`SessionSelector` 三臂不变，改的是它们**怎么落到每个库**：

| 选择器 | 目标设计库 | 其它 DESI | 依赖 CATA |
|---|---|---|---|
| `Latest` | 文件最新 | 各自 pin | 各自最新（今天的行为） |
| `Sesno(n)` | `Exact(n)`（§2.3 可回退） | 按 policy | 按 policy |
| `At(t)` | ≤ t 的最新会话 | **各自** ≤ t 的最新 | **各自** ≤ t 的最新 |

**为什么 `At` 才是真正的跨库口径**：sesno 是每个库自己的计数，`ams8000` 的 264 和 `ams7000` 的 264 之间没有任何关系。时刻是**唯一**能跨库对齐的量。所以 `Sesno(n)` 天然只能钉住一个库——这不是实现偷懒，是数据模型如此。

HTTP 不加必填字段。`sesno` / `time` 保持原样，新增两个可选项：

```jsonc
{
  "dbnum": 8000, "refno": "=24384/26202",
  "time": "2026-08-03T14:25:07+08:00",
  "dependency_policy": "same_time",   // same_time | latest（默认 latest，保持今天行为）
  "at_or_before": false               // 见 §2.3
}
```

### 2.2 每库时点解析：一个纯函数

新增 `fast_model/source_pins.rs`：

```rust
pub struct ResolvedSource {
    pub dbnum: u32,
    pub db_type: String,
    pub file: PathBuf,
    pub sesno: u32,
    pub session_time: Option<String>,
    pub how: PinReason,          // Requested | AtOrBefore | SameTime | Latest
}

pub struct DegradedSource {
    pub dbnum: u32,
    pub reason: DegradeReason,   // NoDecodableTime | TimeBeforeOldest | Unreadable
}

pub fn resolve_source_pins(
    pins: &[DbPin],
    locator: &InMemoryCataLocator,
    target_dbnum: u32,
    selector: &SessionSelector,
    policy: DependencyPolicy,
) -> Result<(Vec<ResolvedSource>, Vec<DegradedSource>), HistoricalModelError>;
```

纯函数、只读文件、不持句柄——与 `e3d-model` 那套「输入只有 `(库文件, sesno)`」的分层态度一致，好测。

**降级不许静默。** 某个库解不出时刻，就退成最新**并记一条 `DegradedSource`**；`degraded` 非空时回执里显式带出来。这条是硬要求：静默混一个今天的目录库进历史模型，比报错糟得多。

### 2.3 落洞回退：默认仍报错

与 e3d-io 那份计划的 Q1 同口径——**多报比漏报安全，但静默就近会让人以为拿到的是他要的那一刻**：

- `Sesno(n)` 命中不到 → 保持今天的 `SESSION_NOT_FOUND`，但把 e3d-io 已有的 `nearest_older` / `nearest_newer` **透进 message**（现在这两个字段被 `to_string()` 吃掉了，用的人不知道洞的两端在哪）；
- 显式 `at_or_before: true` 才走 `SessionSelector::AtOrBefore`，并在回执里 `how = AtOrBefore` + 实际落点。
- `At(t)` 本来就是 at-or-before 语义，不受这个开关影响。

### 2.4 注入点：填 `dependency_sessions`，不动 resolver

`E3dDbResolver::resolve` 已经是「先查 `sessions` 表，查不到才读文件最新」（`e3d_model_service.rs:439-452`）。所以：

> **只要在 `build_set` 之前把解析好的每库时点写进 `dependency_sessions`，resolver 一行都不用改。**

`build_set` 只改一处：其余 DESI 库不再无脑用 `pin.sesno`，而是查解析结果；查不到才退回 `pin.sesno`。CATA 仍然 `continue`（**保留原注释那条理由**：预装会用同 dbnum 的别项目文件盖掉 locator，APS7600 vs ZDJ7600 那个坑），走 resolver 拿到的就是我们填进去的时点。

### 2.5 回执说实话

```rust
pub struct HistoricalModelReport {
    // ... 原字段全部保留，plant-ui 现有消费不破
    pub resolved_session: ResolvedSession,   // 保留：目标设计库
    pub sources: Vec<ResolvedSource>,        // 新增：每个库钉在哪、怎么钉的
    pub degraded: Vec<DegradedSource>,       // 新增：谁没钉住、为什么
    pub mdb_membership: &'static str,        // 新增：恒为 "current"，把 §5 Q3 的限制写在回执上
}
```

`snapshot_key` 现在是 `{word0}_{word1}@{sesno}`，跨库时点下不再唯一（同一设计库会话配不同目录时点是两个不同产物）。追加 pins 摘要的 8 位短哈希：`{word0}_{word1}@{sesno}~{pins8}`。`validate_snapshot_key` 的字符集要一起放行 `~`。

---

## 3. 分步执行

| 步 | 内容 | 落点 | 门 |
|---|---|---|---|
| **P0** | §1.4 四件事的探针：扫 MDB 全部库，出「dbnum / 库类型 / 会话数 / 可解时刻数 / 最早最晚时刻」一张表；再对 1 个真实 root 做「目录最新 vs 目录钉旧」的产物对比 | 仓库外探针 + `docs/evidence/` 落一份 | 表出来了；**若可解时刻覆盖率过低或产物差异为 0，本计划就地重估，不往下走** |
| **P1** | `source_pins.rs`：`ResolvedSource` / `DegradedSource` / `resolve_source_pins`，纯函数 + 单测 | 新文件 | 合成夹具：链有洞、时刻全零、时刻倒挂三种都有确定行为；`At(t)` 每库各自 at-or-before |
| **P2** | `SESSION_NOT_FOUND` 透出 `nearest_older` / `nearest_newer`；`at_or_before` 开关 | `historical_model.rs` | 真库 `ams7000` 请求 `sesno=200`：报错文本里出现 1 与 237；带 `at_or_before` 则落到 1 且 `how=AtOrBefore` |
| **P3** | 接线：`generate_historical` 调 `resolve_source_pins` → 填 `dependency_sessions` → `build_set` 用解析结果 | `historical_model.rs`、`e3d_model_service.rs:1394` | 目录库确实被钉住：`DbSet::pinned_sesno(cata_dbnum)` == 解析值 |
| **P4** | 回执 `sources` / `degraded` / `mdb_membership`；`snapshot_key` 加 pins 短哈希 + `validate_snapshot_key` 放行 | `historical_model.rs`、`handlers.rs` | 同一设计库会话、两个不同目录时点 → 两个不同 `snapshot_key`，互不覆盖 |
| **P5** | 真库门：`At(t)` 端到端，断言每个库的落点与 P0 的表一致 | `tests/`（真库 ignored 门，与 `increment_real` 同款） | 全绿 |
| **P6** | ADR 一条（跨库时点口径）+ 本文件 §3.1 落地记录 | `docs/adr/` | — |

P0 独立且**是闸门**。P1→P3→P4 串行，P2 可并行。

---

## 4. 验收

1. `At(t)` 请求返回的 `sources` 里，每个库的 `session_time` 都 ≤ t，且都是该库满足此条件的最新会话。
2. 任何一个库没按时点钉住，`degraded` 里必有一条指名道姓的记录；`degraded` 为空 ⟺ 全部库按时点钉住。
3. `Sesno(n)` 落在 `ams7000` 的洞里：报错文本包含 1 与 237；加 `at_or_before` 后成功且 `how=AtOrBefore`、`sesno=1`。
4. 同一 `(dbnum, refno, sesno)` 配两个不同 `dependency_policy` → 两个不同 `snapshot_key`，`query` 各取各的。
5. `dependency_policy` 缺省时行为**与今天逐字相同**（回归门：老请求体的产物 mesh_id 集合不变）。
6. `mdb_membership: "current"` 出现在每一份回执里。
7. `cargo test` 全绿，含新增的 `source_pins` 单测与真库门。

---

## 5. 待评审拍板

- **Q1 依赖库默认策略**：`dependency_policy` 缺省取 `latest`（今天的行为，回归安全）还是 `same_time`（更符合「历史模型」这个词的字面意思）？**计划取 `latest`**——默认改语义会让现有调用方在不知情的情况下换掉产物。反方意见是：那默认产出的东西仍然名不副实。
- **Q2 `Sesno(n)` 时依赖库怎么办**：sesno 跨库不可比，`same_time` 只能先把 n 解成时刻再对齐其它库。**若目标库这一会话页解不出时刻，是整单失败还是退成 `latest` 并 degrade？** 计划取「退成 latest + degrade」。
- **Q3 历史 MDB 成员**：本期不做，只在回执标 `mdb_membership: "current"`。要做的话是另一条线（SYS 库 `CURD` 也得按时点读）。**确认不做？**
- **Q4 回执结构变更**：`sources` / `degraded` / `mdb_membership` 是纯新增，`resolved_session` 保留。plant-ui 侧需不需要同步改？还是先只透出、界面下一轮再接？
- **Q5 时刻解码的信任度**：`local_time` 是 old-pdms-io 传下来的约定，e3d-io **明确拒绝**替这个编码背书（`stamp_words` 注释：encoding is not closed），而且按**本机时区**解释、文件里不存时区。跨库对齐把这条约定的权重从「给人看的一格」提到了「决定读哪棵树」。**要不要在 P0 里加一项：拿 E3D 里 `Q SESSION` 的显示值和解码值对若干条，把这条约定坐实或证伪？** 计划：加，且它是 P0 的一部分。

---

## 6. 复现 / 探针

```powershell
# P0 探针：MDB 全部库的会话链与可解时刻覆盖率
D:\work\plant-code\old\vendor\e3d-io\target\release\e3d-descriptor.exe sessions `
  --db D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001

# 目录库同款（换 dbnum）
D:\work\plant-code\old\vendor\e3d-io\target\release\e3d-descriptor.exe sessions `
  --db D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7000_0001

# 现有历史接口（对照基线）
curl -X POST http://localhost:PORT/api/v1/model/history/generate `
  -H "Content-Type: application/json" `
  -d '{"project":"...","mdb":"...","dbnum":8000,"refno":"=24384/26202","sesno":250}'
```

> **本文件的 §1.1–1.3 是读码与转引，§1.4 列的四件事本轮一件都没测。** P0 存在的理由就是把它们变成实测。
