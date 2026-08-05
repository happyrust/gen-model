# MISSION — 读懂 AVEVA E3D `core.dll` 的几何/图形实现

## 为什么学这个
你在维护 `gen-model`（Rust 引擎：解析 PDMS/E3D 数据库 → 解析目录(catalogue) → 生成 3D 网格 → 落库）。
要让 `gen-model` 忠实复刻并**增量更新**几何，就必须弄懂 AVEVA 原生 `core.dll` 到底怎么做的：

1. **怎么判断哪些元素是"几何体类型"**（类型分类）。
2. **怎么把"关联模型"生成出来**（设计元素 → 目录几何 → 图元 → 图形）。

## 成功的样子（Definition of Done）
- 能画出 `core.dll` 的几何/图形三层架构，并说清每层职责边界。
- 能凭记忆复述"类型判定"依赖 `DB_Noun` 的哪些标志位、数据从哪来。
- 能用一个**具体元件（如弯头 ELBO）**从头走一遍"关联模型"的生成链路。
- 能区分：哪些是反编译实测、哪些是结合 AVEVA 文档的合理推断。

## 当前聚焦
增量更新：把链路上真实发生过的缺陷立成案例卡，汇总为一份可查的参考文档。

- 参考文档：[`reference/increment-update.html`](reference/increment-update.html)（九阶段全景 + 五张示意图 + 六条不变量）
- 案例集：[`cases/README.md`](cases/README.md)（20 张卡，按变化语义 / 反向级联 / 删除清理 / 水位重放 / 解析按需 / 性能 六族分组）

## 已完成
- 第 1 课：[类型分类 + 关联模型生成](lessons/0001-core-dll-geometry-graphics.html)，配弯头端到端例子。
- 第 2 课：[`ref_rev` 反向引用索引](lessons/0002-ref-rev-reverse-reference-index.html)。
- 第 3 课：[批量模型生成流水线](lessons/0003-batch-model-generation.html) —— 批量循环**不在 core.dll**，在 `Core3D.dll` 的 `add/build/css/cachegml/maplib`；core.dll 供应 GINO 141 opcode、boxlib 包围盒、cpslib 图片存储、nounlib 分类谓词。同时修正了课 01/课 02/速查表各一条旧结论。
- 第 4 课：[`DB_Noun` 类型描述符](lessons/0004-db-noun-type-descriptor.html) —— 收掉 DoD 第 2 条"类型判定依赖哪些标志位、数据从哪来"。三条取值路径的一眼判断法、两条互不触发的懒加载链、`isValid()` 语义反转陷阱。**基线是 E3D 2.1 / PDMS 12.1.1**（本机唯一有的 core.dll）。
- 速查表：[`reference/glossary.html`](reference/glossary.html)（符号 + 地址 + 代码落点 + 复现命令）。
- `DB_Noun` 速查：[`reference/db-noun.html`](reference/db-noun.html)（字段偏移表 + dabacon 字段号表 + 2.1/3.1 版本差 + 复现命令）。

## 版本基线（重要）
本工作区现在跨**两个** `core.dll`，偏移量互不通用：

| 基线 | 何处 | 覆盖 |
|---|---|---|
| **E3D 3.1** | `D:\AVEVA\Everything3D3.1\`（`ida-pro-mcp`，当前不可达） | 课 01–03、记录 0001–0009 |
| **E3D 2.1 / PDMS 12.1.1** | `/Volumes/DPC/reverse/core.dll`（`ida-bridge`，可复现） | 课 04、记录 0010 |

跨版本引用**只认 dabacon 字段号**，不要搬偏移量（差值 24→56 B 递增，不是线性平移）。详见[记录 0010](learning-records/0010-db-noun-e3d21-offsets-and-version-drift.md)。

## 约束 / 偏好
- 目标：AVEVA Everything3D 3.1，`D:\AVEVA\Everything3D3.1\` 下的 `core.dll`、`Core3D.dll`（均 32 位）。
- 工具：本机 `ida-pro-mcp`（headless idalib，端口 13338；会话 `core31-retrace` = core.dll，`core3d-retrace` = Core3D.dll）。
- 讲解用中文；重证据（具体地址 + 反编译代码）；多举例。

## 下一步候选
课 03 遗留（需要 3.1 基线，当前不可达）：
1. 逐块拆 `Core3D!build/ELMODL`(0x1025277e, 7675 B) 的子树遍历与 level 分支。
2. 解码 `GMDRAW` 跳过的 5 个 noun 码：621502 / 621505 / 312510241 / 312510247 / 312510290。
3. 把 `libgm` 的 `gm_Create*` 与 PDMS 目录图元(SCYL/SBOX/SSNO…)做成精确对照表。

课 04 遗留（2.1 基线，本机随时可跑）：
4. 展开 `DB_Udtg::findUdtg`，把 UDET / UDA 那条分支走完。
5. 给 `ReadData` 里 5 个未命名字段号定语义：`261556351`(0xE8) / `281413407`(0xF8) / `861007`(0x120) / `602413`(0x130) / `13953605`(0x134)。
6. 拿同目录的 `XBaseDll.dll`（已有 .i64）交叉验证字段号是否在别处复用。
