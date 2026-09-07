# ADR-061：不复刻 core 的 `XRPNTR` 正向依赖扩散

日期：2026-09-04

关联：ADR-003（前向引用反转 / `ref_rev` 反向索引）、ADR-008（CATA 反向级联）、
ADR-009（改挂两端入队）、ADR-056 / ADR-057（模型面纯函数与状态分家）。

取证：`docs/2026-09-04_model-cascade-update-inventory.md`（联动更新十一类分类表，
本 ADR 收口其中第 10 类）、`docs/plans/2026-09-02-increment-update-audit-and-next-plan.md` §1.4。

## 背景

core 的 `DB_UserChanges::attributeModified(el, att, qual)`（`core.dll 0x5987090`）在记完一条属性
变更之后会**沿依赖递归**：

```
deps = DB_UserChangesDependency::getDependencies(el, att)     # 0x59a11a0
for (dep_el, dep_att) in deps: attributeModified(dep_el, dep_att)   # 递归
```

`getDependencies` 逐指令读出来只有三个来源：

| 来源 | 方向 | 数据从哪来 |
|---|---|---|
| **`ATT_XRPNTR`** | **正向**：`el` → `el` 自己 `XRPNTR` 指向的那个元素，带**同一个属性名** | 元素自身属性 |
| `DB_Attribute::backDependencies(noun)` → `DB_MDB::findElements` | 反向：谁经某个引用属性指向 `el` | 表由 `DB_AttributeValues::InsertBackDependency` 填，**唯一填充方是 `DB_Uda::internalReadData`**——即只来自 UDA 定义 |
| `invokeSubscibers` | 程序注册 | core.dll 内唯一注册者是自测 `DB_Element_test_Get_Pseudo`；Core3D.dll 不导入 |

UDA 那条已经在 09-02 文档里记为「今天 UDA 不参与几何，记录不做」。**`XRPNTR` 这条此前既没实现、
也没有任何 ADR / 计划写明为什么不需要**——它是联动更新分类表上唯一连状态都没有的边。本 ADR 补上。

## 决策

**不复刻 `XRPNTR` 正向依赖扩散。** 增量选根不因为「变更元素的 `XRPNTR` 指向某个元素」而把那个
元素（或它的根）排进重生成。

## 理由

### 1. 层次不对：那条递归属于会话内交互编辑，不属于跨会话刷新

`attributeModified` 是 `DB_UserChanges` 的入口——**会话内本地编辑账本**，作用是让正在操作的用户
视图跟着自己刚敲的那一下即时更新。

我们的增量吃的是**已 SAVEWORK 的会话区间**，对应 core 里的是 GETWORK 刷新路径。本轮实读了这条
路径的全程（`docs/2026-09-04_model-cascade-update-inventory.md` §2.1–§2.3）：

```
DDES_GraphicsMngr::postGetWork    Core3D 0x1052e9f0 (op==4)
  → 库过滤 DB_DB::type ∈ {1, 6}（DESI + ISOD）
  → DB_RawChanges::trim              core.dll 0x5984650
  → 新建件建图形（isElementTopLevelCreate 过滤）   Core3D 0x1052efd0
  → 变更件重画扩散                                  Core3D 0x1052f2e0
       = graphicsBehaviour 过滤 + owner 抢占，**没有任何依赖递归**
```

**core 自己就不把 `XRPNTR` 扩散应用到已保存会话的刷新上。** 照抄它是与 core 分叉，不是向 core 收敛。

### 2. 字典口径：`XRPNTR` 不携带设计变化

E3D 属性字典的设计变化类（DCHC）里 `XRPNTR = 0`（`src/data_interface/dchc_change_classes.json`；
`0` = 无设计变化，`3` = 纯位姿，`4` = 通用几何）。它是簿记链接，不是几何输入。

### 3. 语料里没有属主

`all_attr_info.json` 的 `noun_attr_info_map`（1878-noun descriptor 快照）里**没有任何 noun 拥有
`XRPNTR`**，`named_attr_info_map` 里也没有它——它在本安装上是伪属性。core 侧那条边本身也带守卫
（`if (DB_Element::getAtt(el, ATT_XRPNTR, tmp))`），取不到就不扩散。

### 4. 反向那一侧我们本来就有，而且更宽

`ref_rev`（ADR-003）按 `extract_reverse_ref_edges` / `value_ref_targets` 收变更元素的**全部**出向
`RefU64` 类属性，**不走白名单**；`direct_root_allowed` 让「有引用者的变更元素」级联到引用者的
交付单元。所以「改了 X → 谁引用 X」这一侧是通用覆盖的。缺的只有 core 那条**正向**边
（`el` → `el.XRPNTR`），而按理由 1–3 它在我们的形态下没有承载物。

## 后果

- 联动更新分类表第 10 类从「⚠️ 未记录为决策」变成「✅ 已记录为决策 · 不做」。
- 与第 9 类（UDA 依赖递归）同档：**两条都是 core 有、我们明确不做**，理由同源——它们都只服务
  会话内交互，且都不是几何输入。
- 不新增任何代码、测试或运行期开销。

## 复议触发条件（任一成立就重开本 ADR）

1. 出现真实用例：某元素的几何**输入**里含另一个元素的属性，而两者之间只有 `XRPNTR` 这一条边；
2. `all_attr_info.json` 的 noun 快照里开始出现拥有 `XRPNTR` 的 noun（例如接进 Schematic /
   Engineering 库之后）；
3. UDA 被接进几何——那时 `backDependencies` 那条边要同时评估，两条一起重议；
4. 我们把增量从「已保存会话区间」扩到「会话内实时编辑」——那时 `DB_UserChanges` 整套语义
   （含本 ADR 拒掉的这条）都要重新对齐。

## 不许当已证

- 理由 3 只证到「本安装的 noun 快照里没有属主」。仓内既有测试
  （`curated_tables_are_reconciled_against_the_runtime_schema`）已经把「字典有名、schema 未含属主」
  记为一个已知类别，所以这**不等于**「E3D 里 `XRPNTR` 永远为空」，只等于「本语料上取不到」。
- `DB_UserChangesDependency` 在 core.dll / Core3D.dll 之外的 DLL 里是否有注册者未查
  （`addSubsciber` 在这两个模块里只有自测调用）。对本 ADR 无影响：那是第三条来源，不是 `XRPNTR`。
