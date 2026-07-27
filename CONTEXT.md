# 增量模型生成（Incremental Model Generation）

本上下文描述：当 E3D/PDMS 设计数据发生增量变化（新增 / 修改 / 删除 / 搬迁）后，如何以**尽量小而正确**的范围重新生成受影响的三维模型，而不是全量重算。术语在此统一，实现见 `src/data_interface/` 与 `src/fast_model/`。

> 本文件仅为词汇表（glossary），不含实现决策与流程；决策见 `docs/adr/`，规格见 `docs/specs/`，计划见 `docs/plans/`。

## Language

**增量模型生成 (Incremental Model Generation)**：
在已有全量模型基础上，仅对本次数据变化影响到的部分重新生成模型，与「全量生成」相对。
_Avoid_: 局部更新、局部刷新

**生成根 (Generation Root)**：
一次增量重算所选定的**根参考号**——从它开始（含其子树）重新生成模型。一个变化元素归一到恰好一个生成根；同一生成根在一个批次内只生成一次。生成根有两种口径：交付单元根、正常颗粒根。
_Avoid_: 重生成根、regen root、目标根

**最小交付单元 (Minimum Delivery Unit, MDU)**：
一类**具备独立成败与独立交付语义**的模型单元类型。类型集合由项目配置决定：默认 `BRAN / HANG / SUPPO / EQUI`，`delivery_unit_types` 可完全取代默认集合、`append_delivery_unit_types` 可在默认集合上追加；层级容器 `WORL/WORLD/SITE/ZONE` 与管件 `FTUB` 恒被拒绝。当变化元素自身或其最近祖先命中该类型时，以该单元为生成根；`FTUB` 及其子件在正常颗粒路径中也必须继续上溯，不能成为生成根。
_Avoid_: 交付单元类型、delivery type、内置交付单元

**交付单元根 (Delivery-Unit Root)**：
生成根的一种：变化元素**最近的** MDU 类型的自身或祖先；嵌套时取最近者。
_Avoid_: 单元根、unit root

**正常颗粒 (Normal Granularity)**：
生成根的另一种：当变化元素**没有** MDU 祖先时采用的默认口径，等同于自动 watch 路径长期使用的 significant owner 口径（而非「整个 ZONE 兜底」，也非「跳过」）。
_Avoid_: 常规颗粒、默认颗粒、ZONE 兜底

**Significant Owner（重要属主）**：
正常颗粒根的解析规则：从元素属主起、跨越 loop 容器上溯到最近的**非 SITE/ZONE** 属主作为根；若该属主是 SITE/ZONE（过粗），改用元素自身作根；元素自身即 SITE/ZONE/WORL 时不生成（不整区重算）。
_Avoid_: 显著属主、主属主

**Loop 容器 (Loop Container)**：
`LOOP / PLOO / VERT / PAVE` 等**自身不是几何生成根**的层级容器；解析生成根时需跨过它继续上溯到 PANE/EXTR 等真正的生成体。
_Avoid_: 环容器

**父模型输入 (Parent-Model Input)**：
自身不作为独立交付模型、但其数据参与父元素几何或坐标计算的子元素，例如 GENSEC 的 `SPINE / POINSP / JLDATU / PLDATU / ENDATU`；其变化归并到父级生成根。
_Avoid_: 辅助几何、漏生成类型

**模型影响 (Model Impact)**：
一次元素操作对模型的处理动作三态：`Regen`（重生成几何）/ `TransformOnly`（仅更新 world transform，网格不变）/ `Skip`（纯业务元数据，不处理）。由单一权威 `classify_operation_impact` 判定，取「宁多勿漏」。
_Avoid_: 几何影响、is_geometry_update

**净变化 (Net Change)**：
一个 `refno` 在整个待更新会话窗口内所有操作合并后的最终结果：`Added / Modified / Deleted / Cancelled`（新增后删除=无净变化）。
_Avoid_: 合并变化、final op

**搬迁 (Move / Relocation)**：
元素 `OWNER` 变化导致其归属的生成根在更新前后不同；原生成根与新生成根**两端都需重生成**。
_Avoid_: 移动、迁移

**应用水位 (applied_sesno)**：
某 `dbnum` 已成功落库、且对应模型工作已持久化为 durable pending 的会话号上界；模型执行可在水位后失败并独立重试。它只在对应数据批次与模型计划原子收口后推进，与扫描观察值 `file_latest_sesno` 严格区分、互不替代。
_Avoid_: 水位、sesno 水位（泛指时）

**待重试单元 (Pending Model Unit)**：
数据已成功后仍需执行或重试的独立模型任务，包含位姿更新、删除清理、反向级联展开和生成根重算；生成根任务键为 `(dbnum, 生成根)`，同一根只保留一条最新任务。消费时先完成非重生成动作，再重新读取并批量生成其展开出的全部根。
_Avoid_: 重试任务、pending task

**关联展开链 (Association Expansion Chain)**：
设计元素 →(`SPRE`/`CATR`/SPREF)→ 目录构件(`SCOM`/`SPCO`) →(几何集 `GMSET`)→ 用设计参数(`DESP`/`PARA[]`)展开出图元 的正向链；决定"一个元素画成什么形状"。core.dll 在段重建时按此链现场展开。
_Avoid_: 目录链、catalogue chain

**反向引用 (Back-reference)**：
E3D 为可被引用元素维护的"谁引用了我"的存储型逆指针（`BREF`/`SPBREF`/`SCBREF`/`TABREF`/`DBREF` 等属性，由 `DB_ElementChangesPlugger::PostSetRefListAttribute` 维护）。用于从被改动的目录/规格元件**反查**需重生成的设计实例，是「关联模型也要更新」判定的权威来源。
_Avoid_: 逆引用、backref、被引用列表

**变更集 (Change Set)**：
按会话区间算出的元素变更集合（对应 core.dll 的 `DB_UserChanges`，由 `DB_DB::elementsChangedBetween` 产出）。core.dll 分五类：`elementCreated`/`elementDeleted`/`attributeModified`/`elementReordered`/`elementIncluded`；gen-model 现映射为 `Add`/`Modified`/`Deleted`（reorder 并入 `children_changed`，无 include）。
_Avoid_: 增量集、delta、diff

## 按需解析元件库（On-demand CATA）

**按需解析（元件库 / CATA）(On-demand CATA Parsing)**：
只解析「本次生成真正引用到」的元件库元素，而非整库解析 CATA；与「整库解析」相对。
_Avoid_: 懒解析、增量解析 CATA、局部解析

**引用闭包 (Reference Closure)**：
从一组种子参考号出发，沿引用关系（横向出向引用 + 纵向 owner 链与容器子树）传递可达、并收口到元件库类型的参考号集合；是按需解析要解析的最小 refno 集。
_Avoid_: 依赖闭包、引用图、传递闭包（泛指时）

**部分解析 (Partial Parse)**：
给定 dbnum + refno 子集，仅解析这些元素（靠 refno→文件偏移索引定点），不整库解析。
_Avoid_: by-refno 解析、按号解析、随机解析

**生成根闭包 (Generation-Root Closure)**：
以一个生成根（见「生成根」）子树的出向引用为种子求得的引用闭包；在该根重生成前主动一次性解析落库。
_Avoid_: 根闭包、主动闭包

**惰性兜底 (Lazy Fallback)**：
生成期命中尚未解析的元件库参考号时，即时对其小闭包做部分解析并落库、随后重试原查询，保证不静默缺模型；用于覆盖引用闭包跟不到的非存储引用边。
_Avoid_: 惰性解析、按需补齐、lazy load

**闭包漏边 (Closure Miss)**：
引用关系不体现为存储型 `RefU64` 引用（如几何表达式里按名引用 `DTAB`/`CATREF`）时，引用闭包无法跟到的边；由惰性兜底覆盖。
_Avoid_: 漏引用、R2 残余（泛指时）

**dabacon 字典 (dabacon Dictionary)**：
E3D 的数据字典（schema 源），定义每种元素类型(noun)的属性布局与分类 flag。core.dll 运行期从 dict DB 建成内存表、按 `(nounHash, fieldId)` 读；gen-model 的 `all_attr_info.json` 只是其中「noun→属性」部分的 bincode 快照，不含分类 flag。
_Avoid_: 字典、schema 库

**noun 分类 flag (Noun Classification Flag)**：
dabacon 字典里描述某 noun 图形语义的布尔/枚举字段：`primitive`(#659518) / `geomset`(#859903) / `extrusion`(#663225) / `isPointsetPoint`(#290555737) / `graphicsBehaviour`(←5099119)。决定「是否几何、按哪种画法生成」；语义在字典、不在 core.dll 二进制。
_Avoid_: 类型标志、noun flag

**会话索引 (Session B-tree Index)**：
E3D db 文件「最新会话」内置的一棵 B-tree，把 `refno` 映射到其元素记录的页 / 偏移；据此可 O(log n) 单点定位（`find_refno_entry`）或遍历叶子直接建 `refno→偏移`表（`gen_ref_type_pos_table_from_index`），无需全文件扫描。
_Avoid_: refno 索引树、btree、索引区

**索引优先建表 (Index-first Table Build)**：
构建 `refno→文件偏移`表时优先走会话索引、解码失败才回退全缓冲扫描（`gen_ref_type_pos_table_scan`）；干净库两者相等，编辑库索引得最新会话存活集、更小且更权威（scan 会多收已删、漏活元素、指旧副本），是「部分解析 / 按需解析」定位的底座。
_Avoid_: 索引建表、index-first
