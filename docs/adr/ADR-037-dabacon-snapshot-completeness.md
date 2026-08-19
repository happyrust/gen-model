# ADR-037：Dabacon 单快照读取与完整性提交门

状态：Accepted（2026-08-19）

关联：ADR-001（水位承诺）、ADR-017（暂存窗口）、ADR-021（文件回退）、
ADR-028（抽取树身份）、ADR-031（净窗口单一口径）、ADR-034（CATA 依赖闭包）、
ADR-036（成员删除对账）；`specs/013-dabacon-snapshot-completeness/`

## 背景

生产增删改已经统一到 `pdms_io::net_window`，但一次数据批次仍会由多个 reader
分别重开同一路径。更严重的是，终稿解析失败、选中子索引页读取失败和基线 chunk
失败都可能降为 warning/局部成功，随后仍建立 `applied_sesno`。水位承诺的是窗口
完整落库，不是已生成的那部分 SQL 执行成功。

## 决策

1. `pdms_io` 是顶层 dabacon reader authority；`parse_pdms_db` 保留为内部 decoder。
   一次读取由 `DabaconSnapshot` 绑定稳定文件身份、头、会话映射与目标锚点。
2. 净窗口只有完整结果才能返回。Added/Modified 终稿完整解析失败时，先用不依赖
   属性字典的最小身份解析 noun；仅代码内白名单 `MNUM` 可作为非持久系统记录跳过，
   其余失败阻断窗口。白名单不是配置项。
3. 只有结构上已经证明不可达的指针可跳过。一个已经被路由选中的合法 child 页读取
   或解码失败、层级不下降，均视为不完整观察并阻断；不得把通用 `Err` 合成业务删除。
4. 每条按 RecordLoc 解码的记录必须满足 `decoded.refno == expected_refno`。
5. 模型计划最终存在性查询使用同一文件身份与目标会话锚点；同 sesno 换文件也拒绝。
6. 基线任一 chunk 失败时停止调度、等待已派发写入结束、清理该 dbnum，且不登记成功、
   不建立水位。失败后的空库优于部分基线。
7. CATA 扫描失败不缓存空集；任一闭包 `missing > 0` 不缓存依赖清单。required 路径继续
   阻断，best-effort 路径可返回错误账本但不能把不完整结果固化为成功缓存。

## 对 ADR-031 的修订

ADR-031 / spec-003 中“任意非根子页不可读可跳过”与“任意终稿不可解析可跳过”的宽松
规则由本 ADR 取代：容忍必须先有结构证明或明确 noun 白名单，计数和 warning 不能代替
提交完整性。

## 后果

- 某些以前会带 warning 推进的窗口现在会 Failed 并保留水位，直到 reader/字典修复。
- 同一路径的 append 仍可继续；路径原子替换为另一文件会被稳定身份识别。
- `parse_pdms_db` 不需要消失，但主仓不再让多个 decoder 各自拍板同一个水位。
