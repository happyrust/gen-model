# Spec 020：模型树右键「重新生成模型」

**Created**: 2026-08-20  
**Status**: Proposed  
**Input**: 「给 plant-ui 模型树添加右键重新生成的功能，方便我可以删除已经生产的模型，然后重新生成。」
经 2026-08-20 逐条问答定稿八项决定（容器走 deep query / anc 归根 / 名词表从 health 读 /
复用 ModelLoadVm / 跑完全部显示 / 给确认框 / 整片删一次 / 菜单项形状）。

## 背景事实

三条既有事实决定了这个特性的形状，写在前面免得实现时重新发现：

1. **服务端两条腿都是现成的**：`DELETE /api/v1/model/subtree?refno=&confirm=`
   删精确 PE 子树下已生成的模型数据；`POST /api/v1/model/ensure {refno, force}`
   解生成根、跑一次 `generate_unit_model`。`force` 的注释原文就写着「人明确要求
   重生成时置 true」。
2. **`force` 不删旧数据**——它只跳过 `settled_status` 那道「已经有了就直接返回」的
   判断。所以「先删后生成」必须是两次调用，不能靠 `force` 一步顶掉。
3. **plant-ui 至今一次生成 API 都没调过**。`crates/plant-ui-app/src/main.rs` 里钉着
   `eye_dispatch_does_not_call_the_model_generation_api`，显示路径被明确禁止碰生成。
   本特性是界面上第一个主动发起生成的入口。

## User Scenarios & Testing

### User Story 1 - 一个交付单元重做一遍（Priority: P1）

作为模型查看者，当我发现某根 BRAN / 某台 EQUI 的三维模型不对（生成期的 bug 已经修了、
或者几何本来就算错了），我希望在模型树上右键它、点一下「重新生成模型」，旧产物被删掉、
重新算一遍，然后**当场在三维里看到新的那份**，而不必整场取回工作等十几秒。

**Why this priority**: 这是提出需求时的原话场景，也是唯一一个不依赖 anc 归根就能跑通的
最小切片——右键的那一行自己就是生成根。

**Independent Test**: 在 live 库上右键一根已生成的 BRAN，确认 `inst_relate` 行先归零、
`ensure` 回 `Generated`、三维里该 BRAN 重新出现且 eye 变「已显示」。

**Acceptance Scenarios**:

1. **Given** 一根已生成模型的 BRAN，**When** 右键点「重新生成模型」并确认，
   **Then** 先发 `Unload` 清掉屏上那份、再 `DELETE /model/subtree`、再
   `ensure(force=false)`，成功后对该 refno 发一条 `SetVisible{visible:true}`。
2. **Given** 同一根 BRAN 在重生成之前是**隐藏**状态，**When** 重生成成功，
   **Then** 它被显示出来（本特性把重生成视作一次显示指令，见 FR-012）。
3. **Given** ensure 回 `NoRenderableGeometry`（这根本来就没有可画几何），
   **When** 收尾，**Then** 计入「成功」但日志点名说明该单元无可画几何，
   eye 停在「未加载」——不假装它显示出来了。

---

### User Story 2 - 一个容器整片重做（Priority: P1）

作为模型查看者，我希望右键一个 ZONE / SITE / PIPE 说「重新生成模型」，
它自己找出这个范围里**所有已经生产过的**东西，逐个交付单元重做，
而不是甩我一个「容器不能做生成根」的报错。

**Why this priority**: 右键一个区说「这片重新生成一遍」是这功能最常见的用法；
禁掉容器等于只剩叶子能用。

**Independent Test**: 在 live 库上右键一个 ZONE，断言候选根集合与
`resolve_generation_roots_on` 对同一批元素解出的根集合一致（允许客户端多出
「无交付单元祖先」的那些元素本身，它们由服务端解根）。

**Acceptance Scenarios**:

1. **Given** 一个 ZONE 下有 340 个已生成交付单元，**When** 点重新生成，
   **Then** 客户端用 `inst_relate.anc` 一次解出候选根集合，逐个 ensure，
   进度条按 N/M 前进。
2. **Given** 候选根里有两个嵌套的交付单元（EQUI ⊃ SUPPO），**When** 外层先生成，
   **Then** 内层 ensure 读到 `renderable > 0` 回 `AlreadyAvailable`，计入「跳过」，
   不重复生成——去重由服务端免费提供，客户端不做嵌套裁剪。
3. **Given** 右键的是 SITE，**When** 解候选根，**Then** SITE 自身不进候选集
   （`WORL / WORLD / SITE / ZONE` 恒不做生成根），但它子树里的单元照常进。
4. **Given** 容器下某个元素的 anc 链上没有任何交付单元名词（STRU / SCTN、
   FLOOR / WALL 那类），**When** 归根，**Then** 把**元素自己**原样发 ensure，
   由服务端的 normal-root 策略解根——客户端不抄那套 owner 链兜底。

---

### User Story 3 - 动手之前看得见代价（Priority: P1）

作为操作者，我希望在真正删任何东西之前，界面先把这一趟的规模摆给我看：
要删多少个已生成元素、要重做多少个单元；并且明确告诉我**中途断掉会丢东西**。

**Why this priority**: 右键一根 BRAN 和右键一个 SITE 在菜单里是同一行字，
但一个是几秒、一个是半小时且会把整个 SITE 装进三维。这一步是唯一的刹车。

**Independent Test**: 纯函数测——给定 deep query 结果，断言确认文案里的两个数字
与元素集合 / 候选根集合的大小一致，且文案含中断告警。

**Acceptance Scenarios**:

1. **Given** 点了菜单项，**When** deep query 返回，**Then** 弹出确认，文案含
   「将删除 N 个已生成元素，归成最多 M 个生成单元重做」与「中途中断的话，
   没重做完的那些找不回来」。
2. **Given** deep query 返回空集（这个范围里一个已生成元素都没有），
   **When** 弹确认，**Then** 说「这个范围里没有已生成的模型」并只给关闭，
   不发任何删除。
3. **Given** 人在确认框上取消，**When** 关闭，**Then** 没有发出任何 DELETE / ensure，
   三维与 eye 一动不动。

---

### User Story 4 - 跑起来之后看得见、拦得住、追得回（Priority: P2）

作为操作者，我希望这一趟跑起来之后有进度、能叫停，失败的那些不要凭空消失。

**Why this priority**: 规模上去之后没有进度条的操作等同于卡死。

**Independent Test**: 纯函数测状态机——注入若干个根的成功 / 409 / 404 / 412 /
超时 / 生成失败，断言收尾计数与「整趟中止」判据。

**Acceptance Scenarios**:

1. **Given** 一趟正在跑，**When** 看界面，**Then** `ModelLoadVm::Loading` 显示
   「重新生成 <行名> · N/M」，`fraction()` 随完成数前进。
2. **Given** 一趟正在跑，**When** 打开模型树右键菜单或取回工作菜单，
   **Then** 这两项置灰（`regen_busy`，与既有 `get_work_busy` 同纪律）；eye 不受影响。
3. **Given** 点了「停在这里」，**When** 当前那个 ensure 还在飞，**Then** 不再派发
   下一个，已经发出去的那一个照跑（服务端 `await_background_without_cancelling`
   不受客户端影响），按钮文案不说「取消」。
4. **Given** 某个根 ensure 失败，**When** 收尾，**Then** 服务端留下的 `RegenRoot`
   pending 行经 `/api/v1/update/pending-units` 出现在任务队列的「待重试单元」里，
   前台只报一句「M 个单元：成功 X、跳过 Y、失败 Z，失败的去待重试单元重试」。
5. **Given** 服务回 503 初始化未就绪或网络不可达，**When** 收到，**Then** 整趟
   立即中止并红字说明——继续往下删只会扩大损失。
6. **Given** 某个根 ensure 回 400「容器不能做生成根」，**When** 收到，**Then**
   响亮报错而不是静默跳过：它意味着客户端的候选根与服务端策略对不上
   （多半是 health 里的名词表读错了）。

---

### Edge Cases

- **右键一个从没生成过的元素**：deep query 空集 → US3 场景 2 的空态，不删不生成。
- **`inst_relate.anc` 未回填**（`inst_relate_anc_ready()` 为 false）：菜单项照常显示，
  点下去响亮失败并给出与模型查询同样的那句提示（升级 gen-model 对该库启动一次），
  不做深遍历降级——旧深遍历路径已随层级查询优化 P3 退役。
- **多选里既有 ZONE 又有它下面的 BRAN**：候选根并集去重，那根 BRAN 不做两遍；
  DELETE 对每个 target 各发一次，ZONE 那次已经覆盖 BRAN 的子树，BRAN 那次是空操作。
- **重生成期间设计库来了新数据**：本特性不与增量更新互斥。ensure 拿的是库里此刻的
  样子，这是正确行为；重生成结束后若队列又推进了水位，走既有的「自动刷新只换树 +
  日志提示再点取回工作」。
- **一个根 ensure 超过 120 秒**：服务端回超时但后台继续跑。计入「仍在后台」，
  不算失败、不重试，收尾提示稍后取回工作查看。
- **跑到一半关窗口 / 切项目 / 服务重启**：按定稿决定，没重做完的那批**永久丢失**
  ——anc 里已经没有它们，重新右键同一个容器也找不回来，服务端亦无记录。
  确认框必须把这句话说出来（FR-009）。

## Requirements

### Functional Requirements

- **FR-001**: 模型树行菜单与三维视口右键菜单（`element_menu`）MUST 增加一项
  「重新生成模型」，成批时按现有规矩带计数后缀（「重新生成模型 3 项」）。
  它 MUST 自成一组，位于「查看所属房间」之下、「复制 REFNO」之上，用 `separator` 隔开。
- **FR-002**: 该项 MUST 吃 `live` 门禁，与显示 / 隐藏 / 定位同一道条件；
  MUST NOT 出现灰色占位项（现有约定：不接线就不显示）。
- **FR-003**: 候选元素集 E MUST 由 `inst_relate.anc` 索引查询得到
  （`select value in from inst_relate where anc contains <target>`），
  对每个 target 各查一次后取并集去重。**MUST 在任何删除之前查完**——
  删掉之后 `inst_relate` 行不复存在，那时再查是空集。
- **FR-004**: 候选生成根集 R MUST 由以下三部分并集去重构成，且 **MUST NOT 依赖
  `anc` 数组的顺序**：
  1. E 中各元素 anc 链上、noun 属于交付单元名词表的那些祖先；
  2. E 中自身 noun 属于交付单元名词表的元素；
  3. anc 链上不含任何交付单元名词的元素**自身**（交给服务端解 normal 根）。
  外加：右键的 target 自身，当且仅当它的 noun 不属于 `WORL / WORLD / SITE / ZONE`。
- **FR-005**: 交付单元名词表 MUST 从服务端读，不得在客户端硬编码。
  gen-model 的 `GET /api/v1/health` MUST 新增 `delivery_unit_types` 字段，
  值取自既有的 `configured_delivery_unit_types()`。客户端读不到时 MUST 响亮告警
  并拒绝对容器行执行（单个非容器 target 仍可执行，因为它不需要归根）。
- **FR-006**: 确认之后的执行顺序 MUST 是：对每个 target 发一条
  `ModelAction::Unload` → 对每个 target 发一次
  `DELETE /api/v1/model/subtree?refno=<target>&confirm=<target>` → 按 R 逐个
  `POST /api/v1/model/ensure {refno, force:false}`。
- **FR-007**: ensure MUST 用 `force:false`。删除已经把这一片清空，
  `force:false` 让同根的后续请求读到 `renderable > 0` 直接回 `AlreadyAvailable`，
  嵌套单元与共根元素的去重由服务端免费提供。
- **FR-008**: ensure MUST 串行派发（并发 1）。生成根锁是 `try_lock` 不排队，
  并发派发会把同根的第二个请求变成 409 而不是等待。
- **FR-009**: 点击菜单项 MUST 先跑 FR-003/FR-004 的查询、再弹确认框；确认框
  MUST 报出 `|E|` 与 `|R|` 两个真实数字（`|R|` 标明为上限，实际生成数可能更少），
  MUST 明说「中途中断的话，没重做完的那些找不回来」。`|E| == 0` 时 MUST 只给关闭。
- **FR-010**: 进度 MUST 复用 `vm::ModelLoadVm`（`Resolving` → `Loading{done,total}`
  → `Success` / `Failed`），标签形如「重新生成 <行名> · N/M」。
  MUST NOT 新增任务窗、MUST NOT 接进 `task_queue.rs`。
- **FR-011**: 每个根 ensure 成功之后 MUST 立即清掉相关行的 `scopes` 缓存
  并对该根发一条 `ModelAction::SetVisible{visible:true}`；不等整趟跑完。
  相机 MUST NOT 移动。
- **FR-012**: 重生成 MUST 把涉及的模型**全部显示出来**，包括重生成之前处于
  「已隐藏」的那些。（定稿决定；它与 ADR-0021 取回工作的「按快照回放方向」
  刻意不同，因为重生成是人对着具体对象发起的一次「我要看这批新的」。）
- **FR-013**: 界面 MUST 有一个 `regen_busy` 标志；为真时「重新生成模型」与
  「取回工作」两项置灰，理由与既有 `get_work_busy` 相同（两者都会大动三维）。
  eye 点击不受限制。
- **FR-014**: 「停在这里」按钮 MUST 只停止后续派发，MUST NOT 声称能取消已发出的
  那一次生成；文案不得使用「取消」。
- **FR-015**: 错误 MUST 分两档处置：
  - **整趟立即中止**：503 初始化未就绪、网络不可达 / 连接失败。
  - **记一笔、继续下一个**：409（这个根别人在做，计「跳过」）、404（元素没了，
    计「跳过」）、412（解不出根，计「跳过」并告警）、120 秒超时（计「仍在后台」）、
    真实生成失败（计「失败」）。
  400「容器不能做生成根」MUST 响亮报错、MUST NOT 静默跳过。
- **FR-016**: 收尾 MUST 报一句含全部计数的结论，并指明失败项去任务队列的
  「待重试单元」重试。MUST NOT 在前台另建一份失败清单。

### Key Entities

- **候选元素集 E**：这一趟范围里**已经生产过**模型的元素（有 `inst_relate` 行）。
  它天然排除从未生成过的元素——与需求原话「删除已经生产的模型」一致。
- **候选生成根集 R**：E 归并出来的生成单元，`ensure` 的实际目标。`|R|` 是上限，
  嵌套与共根会在服务端自动跳过。
- **交付单元名词表**：服务端 `configured_delivery_unit_types()` 的结果。
  默认 `[BRAN, HANG, SUPPO, EQUI]`，`DbOption.toml` 的 `delivery_unit_types`
  能整体替换、`append_delivery_unit_types` 能扩充。
- **`RegenRoot` pending 行**：ensure 跑之前落的 durable 记录，失败即成为
  「待重试单元」，是这个特性唯一的持久失败账本。

## Success Criteria

- **SC-001**: 右键一根已生成 BRAN 重新生成，`inst_relate` 行先归零后重建，
  三维里该 BRAN 重新出现，全程无需取回工作。
- **SC-002**: 右键一个 ZONE，客户端解出的候选根集合与服务端
  `resolve_generation_roots_on` 对同一批元素解出的根集合一致（客户端可多出
  「无交付单元祖先」的元素本身，不可漏）。
- **SC-003**: 一趟含 340 个候选根的重生成中，实际触发真生成的次数
  ≤ 候选根数，且同一根不被生成两次（靠 `AlreadyAvailable` 计数佐证）。
- **SC-004**: 确认框里的两个数字与 deep query 结果逐一对得上；
  空集时不发出任何 DELETE / ensure（抓包或日志佐证）。
- **SC-005**: 重生成期间「取回工作」与「重新生成模型」均置灰；eye 仍可点。
- **SC-006**: 注入 409 / 404 / 412 / 超时 / 生成失败五种回包，收尾计数逐条对得上；
  注入 503 时整趟在第一个错误处中止，后续根一个都没发出去。
- **SC-007**: 把 ensure 的 `force` 改回 `true`、或把删除挪到 deep query 之前、
  或去掉 `regen_busy`，对应回归测试变红。

## Assumptions

- `plant-ui` 直连 SurrealDB（`plant-ui-data` 已在用 `query_inst_refnos_by_root_anc`），
  所以 FR-003 / FR-004 的两条查询不经过 gen-model 的 HTTP 面。
- `inst_relate.anc` 存的是元素祖先 refno 的 u64 数组；本 spec 不依赖其顺序，
  也不依赖它是否包含元素自身（FR-004 的三部分并集对两种情形都成立）。
- 归根需要 noun，客户端按候选祖先 refno 回 `pe` 批量读一次 noun；
  这一步的批量大小沿用 `model_instances_anc` 已验证过的分批口径。
- gen-model 侧改动仅一处：`/health` 增 `delivery_unit_types` 字段。
  `configured_delivery_unit_types()` 是进程内 `OnceLock` 缓存、不读库，
  因此不需要套 `within_health_budget`。
- 「中途中断永久丢失」是**已知且被接受**的代价（2026-08-20 定稿）。
  逐单元「删一个生一个」能消除它，但会把 DELETE 从 1 次变成 M 次，被否决。
  界面对此的义务只有一条：在确认框里说出来。
- 本特性不实现通用的「局部重画」。它只在自己这条路径上做 Unload + SetVisible，
  `auto_refresh` 那条路仍然停在旧几何上，两条路径的行为不同，这是已知的。
