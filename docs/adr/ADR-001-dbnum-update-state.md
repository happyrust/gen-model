# ADR-001：以 DBNUM 状态记录作为唯一增量水位来源

状态：已接受  
日期：2026-07-23

> **2026-08-05 修订（ADR-017）**：稳态增量窗口的 `applied_sesno` 推进条件增强为「窗口数据 + 全部模型生成作为一个提交单元写回成功」；失败不推进、幂等重放、文件身份 / 扫描 / 阻断规则均不变。基线路径维持原语义。

> **2026-08-13 修订（ADR-021，取代 2026-08-12 修订）**：两处语义收窄。其一，本 ADR 的「只有对应数据批次成功持久化后才能推进 `applied_sesno`」管的是**写**的一侧，ADR-021 补上对偶的**读**侧约束：`applied_sesno > 0` 必须有数据支撑，该 dbnum 在 `pe` 里一行都没有时按首次导入重建基线，不得从水位往后接增量（判定落在「基线还是增量」的路由上，不落在入队门上；不进 `FileAnomaly`）。其二，「水位永不回拨」收窄为**增量路径内永不回拨**：判定为回退（`file_latest_sesno < applied_sesno`，文件被还原/替换）的 dbnum，默认处置是由数据批次 worker 在冻结点复核后**整库清空该 dbnum 的数据并按首次导入重建**（`wipe_dbnum_for_reinit`：水位行清值不删行、统计与队列残留清空、spatial epoch 同阶段递增）——水位随重建归零再重新建立，属于正常处置而非 opt-in 例外。`watermark_realign` 档位、`AIOS_WATERMARK_REALIGN` 与单库对齐端点随之移除；`TypeChanged` / `Duplicate` / `Missing` / `ForeignProject` 等身份歧义异常仍阻断等人。本节「初始状态」描述的 `dbnum_info_table` 回填播种不变，但由它造出的水位从此受数据支撑校验约束。

> **2026-08-12 修订（watermark_realign，已被 2026-08-13 修订取代）**：为「数据批次失败、文件异常或模型生成失败都不能回退或虚增 `applied_sesno`」增加**唯一的 opt-in 例外**——配置 `watermark_realign = "rebaseline"`（或对单库调用 `POST /api/v1/dbnums/{dbnum}/realign`）时，判定为**回退**（`file_latest_sesno < applied_sesno`，文件被还原/替换）的 dbnum 允许被显式对齐：先经 `prune_above_watermark` 在 `STAGED_COMMIT_SERIAL` 与 `DBNUM_STATE_WRITE_GATE` 两把写闸内物理清除会话号高于文件水位的行与队列残留，再把 `applied_sesno` **写 0（写值不删行，登记身份保留）**，交由基线路径按首次导入重建。例外仅此一条：人工开启（配置档位或单库端点）、只对回退（`FileAnomaly::auto_realignable`）；其余异常种类、默认档位（`off`）以及自动扫描/执行路径自身，均维持本 ADR 原语义——水位永不因失败或异常自行回拨。

## 背景

现有后端同时使用两类记录判断一个 `dbnum` 的最新应用会话：

- `dbnum_watermark:{dbnum}`：数据成功持久化后推进的专用水位。
- `dbnum_info_table`：由 PE 写入和事件维护、按 `ref_0` 分行的元素统计。

当前查询会从两者取最大 `sesno`。这存在三个问题：

1. `dbnum_info_table` 的业务粒度不是 `dbnum`，不能代表完整数据批次是否成功。
2. 元素统计可能在批次未完整完成时先发生变化，取最大值可能错误跳过后续增量。
3. 文件身份、文件最新会话和已应用会话没有集中管理，无法可靠识别文件回退、重复、缺失和迁移。

## 决策

扩展现有 `dbnum_watermark:{dbnum}`，将其定义为当前项目库内“一行一个 `dbnum`”的权威 DBNUM 状态记录，不新增第二张同粒度表。

记录至少包含：

```text
dbnum
db_type
file_name
file_path
file_size
file_modified_at
file_latest_sesno
applied_sesno
scanned_at
applied_at
```

状态变化规则：

- 首次发现一个未登记 `dbnum` 时，扫描可以建立登记文件身份，并写入文件观察字段。
- 已登记文件身份一致时，扫描可以更新文件属性、`file_latest_sesno` 和 `scanned_at`。
- 只有唯一文件在项目与 `db_type` 一致、且水位不回退时，`PathMigrated` 才能更新登记路径。
- `TypeChanged`、回退、重复、缺失等阻断异常不得覆盖登记文件身份，也不新增第二套持久 observed identity；异常由当次扫描结果、任务回执和日志报告。
- 文件观察字段必须通过独立写入更新；该写入失败不能修改或间接影响 `applied_sesno`。
- 只有对应数据批次成功持久化后才能推进 `applied_sesno` 和 `applied_at`。
- 数据批次失败、文件异常或模型生成失败都不能回退或虚增 `applied_sesno`。
- 逻辑增量窗口为 `(applied_sesno, file_latest_sesno]`；实际读取从不小于 `applied_sesno + 1` 的首个可用会话开始，不假设会话号连续。

`dbnum_info_table` 继续承担按 `ref_0` 的元素统计职责，不再参与日常水位计算。

## 初始状态

启动时先读取数据库表定义、现有水位行数与播种完成标记（`queue_control:watermark_seed`），
再幂等创建缺失的增量状态表。在 pe 兼容播种于此库**完整完成一次**之前——即 `dbnum_watermark`
整张表不存在、表存在但没有任何水位行（建表后播种前中断）、或有行但完成标记缺失（分块播种
半途中断，或首次升级到带标记的版本）——都视作在已有业务数据上兼容启用增量：按 `pe` 中每个
dbnum 已持久化数据的最大 `sesno` 建立 `applied_sesno`，以当前数据库实际内容为基线，而不是
拿现场文件最新会话冒充已应用水位。播种是 `??` 填空、绝不覆盖已建立水位，因此补跑无损；
全部批次成功后落下完成标记，此后启动回到 `dbnum_info_table` 快路径。播种前会做一次按 dbnum
的完整性比对（`count(pe)` 对 `sum(dbnum_info_table.count)`，与基线路径同口径），对不上的库
打告警但不阻断——那多半是历史解析中断留下的洞，需要人工决定是否重建基线。若水位表已有行，
则已有 `applied_sesno` 优先；旧行只有 `sesno` 时固化该值；没有专用水位行但 `dbnum_info_table`
有历史统计时，才以该 dbnum 的最大统计 `sesno` 回填。播种走 `pe` 源时对全表聚合，大库上耗时
较长，启动日志会打出开始（含原因）与完成（含耗时）两行。仍无法得到历史水位的 DESI 不得猜测
范围，必须先由全量项目生成建立明确基线，再允许增量应用。

## 结果

### 正面

- 水位语义与数据批次成功条件一致。
- 预览可以同时展示“文件最新”和“已经应用”，不再混淆。
- 文件回退、重复、缺失和迁移可以统一检测。
- 手动模式和自动模式可复用同一状态记录。
- 不引入新的持久化依赖或重复表。

### 代价

- 现有 `SesnoRangeResolver` 和水位推进逻辑需要迁移到新字段。
- 旧版本程序若继续依赖 `sesno` 字段，不能与新版本并发更新同一项目。
- 扫描阶段会产生仅限文件观察元数据的写入。

### 约束

- 同一项目不能同时运行自动 watcher 和手动更新。
- 不通过文件内容哈希识别文件；本期使用文件头身份、路径、大小、修改时间和会话号。
- 文件内容哈希只有在实际出现无法识别的同路径替换问题时再增加。

## 未采用方案

### 继续从两个表取最大值

拒绝。它无法证明完整 `dbnum` 批次已经成功，会使水位提前。

### 将 `dbnum_info_table` 改成一行一个 DBNUM

拒绝。该表已经承担按 `ref_0` 的元素计数和最大参考号统计，改变粒度会破坏既有语义和事件。

### 新建独立 `dbnum_file_state` 表

拒绝。现有 `dbnum_watermark` 已经是一行一个 `dbnum`，扩展它是更小且更直接的改动。

### 每次都从全部 PE 记录推导水位

拒绝。代价更高，且仍不能表示整个数据批次是否完整成功。
