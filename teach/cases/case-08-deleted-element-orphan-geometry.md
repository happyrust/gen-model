# 案例 08 · 删除元素后旧几何残留：软删墓碑被生成期过滤掉了

<sub>族 C 删除清理 · Critical · 已修 · 证据层 B（单测）+ C（实库）</sub>

## 一句话

被删元素带 `deleted = true` 软删墓碑，而生成期用 `!deleted` 过滤——于是它**永远进不了删除集**，
几何原样留在库里。

## 现象

元素在 E3D 里被删除，增量更新跑完之后：

- 它的 `inst_relate` / `geo_relate` / `geo` 以及 mesh 仍然留在库中；
- 删除的若是整个交付单元，**它整棵子树的几何全部成为孤儿**；
- 三维里实体还在。

## 证据

缺陷登记：[`../../docs/specs/incr-gen-fixes/spec.md`](../../docs/specs/incr-gen-fixes/spec.md) **F1（Critical）**。

根因位置：`replace_exist` 的级联删除只作用于**本次重生成的实例键**（`inst_info_map.keys()`，
`src/fast_model/pdms_inst.rs`）。这套「删旧写新」的逻辑对**修改**是对的——重生成谁就先删谁的旧几何；
对**删除**是失效的，因为被删元素根本不会被重生成，它的键不在 `inst_info_map` 里。

两条过滤叠加形成闭环：

```text
E3D 删除元素 X
   └─ 增量落库：pe:X 打软删墓碑 deleted = true（记录仍在，子树仍可经 pe_owner 遍历）
        └─ 生成期收集实例：WHERE !deleted  ⇒ X 被过滤掉
             └─ replace_exist 的删除集 = inst_info_map.keys() ⇒ 不含 X
                  └─ X 的 inst_relate / geo_relate / geo 永久残留
```

## 根因

清理动作被**挂在了重生成的副作用上**，而不是挂在「本窗口的净变化 = Deleted」这个事实上。
只要一个元素不再被生成，顺带清理就够不着它——而「不再被生成」恰恰就是删除的定义。

## 修法

F1 的需求写得很硬（MUST）：

1. 增量刷新 MUST 依据本窗口的**净变化 = Deleted** 集合，**直接按被删 refno 级联删除**其
   `inst_relate / geo_relate / geo`，不依赖 owner 重生成顺带清理；
2. 删除一个容器时 MUST 同时清理其**整棵原子树**的几何实例（软删后 `pe` 子树仍可经 `pe_owner` 遍历）；
3. 清理 MUST **幂等**：对不存在的键重复删除是 no-op，不报错、不阻断其它删除；
4. 清理失败 MUST 走与几何生成一致的错误传播 / 补偿通道，不静默吞错。

落地（[`tasks.md`](../../docs/specs/incr-gen-fixes/tasks.md) T101–T106）：

- `model_refresh.rs::collect_deleted_geometry_refnos` 收集净变化 Deleted（跳过 SYS meta）；
- `helper.rs::delete_inst_relate_subtree` 遍历被删 refno 的 pe 子树（**含 deleted**），
  收集自身 + 后代，调用幂等的 `delete_inst_relate_cascade`；
- `conservative_regen` **先清理、再 owner 重生成**；清理失败 `?` 上抛（走案例 13 打通的错误通道）；
- 子树遍历改为分批（20）的**无深度上限 BFS**，靠去重天然防环——不再是「仅删根却报成功」。
  （P3 backlog 里「递归深度硬编码 10 层」的描述出自旧实现；现在 `delete_inst_relate_subtree(&[root], 10)`
  里的 `10` 是 chunk_size 不是深度。）
- 补偿路径同样补上：`cleanup_deleted_by_pe_state` 按 `pe.deleted` 反推，drain 时先清理再 regen。

## 验证

- 单测：净变化 = Deleted / **Cancelled**（新增后删除）→ 删除集分类正确。
- 实库（2026-07-26）`live_cleanup_by_pe_state_clears_subtree_and_spares_live_sibling`：
  用 `4000000001/…` 造「软删父 + 软删子 + 未删兄弟」三棵几何，断言被删子树（含后代）清空
  且**兄弟原样保留**——1 passed。
- 仍缺（C 层）：在真实 E3D 里删除一个可恢复的测试元素，确认 Surreal 元素、`inst_relate`、
  模型树节点和三维实体都被清理的**前后截图**。

## 规律

**「删除」不能靠「重新生成时顺手覆盖」来实现。** 增量更新里存在两类动作：
一类是「重算这些东西」（幂等覆盖即可），一类是「让这些东西消失」（必须显式执行）。
前者可以靠重生成兜底，后者没有任何兜底——不主动删就永远在。

第二条更隐蔽：**软删墓碑与生成期过滤是一对天然矛盾。** 墓碑的目的是「记录还在、可追溯」，
过滤的目的是「不生成已删的东西」，两者都对；但只要清理逻辑复用了带过滤的那条查询，
它就一定看不见需要清理的目标。凡是遍历「要删什么」的查询，都必须显式声明**包含 deleted**。

## 关联

- [`spec.md F1`](../../docs/specs/incr-gen-fixes/spec.md) · [`tasks.md T101–T106`](../../docs/specs/incr-gen-fixes/tasks.md)
- 案例 [09 级联清理的事务边界](case-09-cascade-delete-transaction.md)（同一个孤儿问题换了个入口回来）
- 案例 [13 mesh panic](case-13-mesh-panic-kills-the-watchdog.md)（F1 依赖 F2 先打通错误通道）
