# 案例 03 · 五张分类名单合并为唯一 attr→effect 映射

<sub>族 A 变化语义 · Medium · 已修 · 证据层 B（单测 + 探针）</sub>

## 一句话

属性归类原本由 if-else 链的**顺序**决定，73 条重名被短路后永远走不到——错写、漏写、两边不一致都不报错。

## 现象

没有可见的线上故障，这是一处**结构性隐患**：

- 一个属性同时写进两张表时，谁在链的前面谁赢。语义被链的形状绑架，读代码看不出来。
- `attribute_affects_model` 的 `matches!` 大列表与前四张表有 **73 条重名**，短路之后那 73 条永远走不到。
- 名单里还混进了 **37 条 noun 名**（`SCTN` `STWALL` `SPCO` `PANE`…），它们是元素类型不是属性名，
  `normalize_attribute_name` 只做 trim / 去 `att.` 前缀 / 取首段 / 转大写，**没有缩写展开也没有截断**，
  所以 `matches!` 是严格相等——这 37 条是永不命中的死分支。

死分支不构成正确性缺口（未命中的属性会落 `Unknown`，采集层保守触发重生成），但它说明名单把
noun 和属性混在了一张表里，而真正该收的「这些几何 noun 自己的形状属性」很可能压根没进清单——
只是被 `Unknown` 兜住了，所以一直没暴露。

## 证据

- 旧 `classify_attribute_effect` 的链：DATA_ONLY → STRUCTURAL → TRANSFORM_ONLY → CASCADE → `attribute_affects_model`。
  案例 [02](case-02-transform-only-was-too-wide.md) 里「TRANSFORM_ONLY 移出 7 条即自动落 DirectGeometry」
  之所以成立，正是踩着这个顺序——改动本身安全，但**这条知识只存在于链的形状里**。
- 重名与死分支的量化来自探针 `output/audit_ref_gap_probe.py`、`output/dchc_coverage_probe.py`
  （只读，从源码现场解析名单再与字典 / 运行库 schema 做集合比对）。
- 227 条清单的真实构成（出处 [`../../docs/2026-07-26_p3-t903-t904-assessment.md`](../../docs/2026-07-26_p3-t903-t904-assessment.md) 第二节）：

| 类别 | 条数 |
|---|---:|
| 字典里有、可比 DCHC | 130（其中 111 条 DCHC ≠ 0，吻合 85%） |
| 是运行库真实属性但字典导出没有 | 9 |
| **是 dabacon noun 名，不是属性名** | **40**（后按数据重新推导为 37） |
| 两者皆非 | 48 |

- 别名表的教训单独记一笔：对 97 个「清单有、字典无」的名字做双向前缀匹配得到 17 条，逐条抽查后
  **大部分是误匹配**——`POSI`→`POS` 错（`POSI` 是独立 BOOL 属性 hash 722860）、`PXDI`→`PX` 错、
  `EXTR`→`EXTREF` 错（左边是 noun 名）。站得住的只有 6 条：
  `LEVEL`→`LEVE`、`SPREF`→`SPRE`、`GMREF`→`GMRE`、`THIC`→`THICKNESS`、`PARAM`→`PARA`、`DEPT`→`DEPTH`。

## 根因

「用 if-else 链逐张表试」这个形状本身就是问题：它把**归类**（这个属性是什么效果）和
**仲裁**（两张表都收了怎么办）混成了一件事，而仲裁规则是隐式的、不可测试的。

## 修法

分两个提交落地：`fa99ea9c`（守护测试先行）、`dfae60b8`（结构重构）。

- 五张 `pub const` 数组保留（按效果分组声明、各带 doc 注释，**分组本身就是文档**）；
  原 `matches!` 大列表显式化为第五张 `DIRECT_GEOMETRY_ATTR_NAMES`，那 73 条重名在显式化时剔除、
  只保留在原先胜出的表里，**归类不变**。
- 新增唯一事实源
  [`ATTRIBUTE_EFFECT_TABLES`](../../src/data_interface/model_impact.rs)（`model_impact.rs:169`）
  把每张表与其效果配对，`attribute_effects()` 用 `OnceLock<HashMap>` 惰性合并成一张查找表。
- `classify_attribute_effect` = 查映射 → `PARA` 前缀回退（`PARA1`/`PARAM7` 这类序号变体不逐个登记）
  → `Unknown`。`attribute_affects_model` 改为**纯派生**：`!(DataOnly | Unknown)`，不再自持第二份名单。
- 组间顺序不再携带语义；跨表重名从「静默按顺序仲裁」变成**守护测试直接报错**。
- 死分支按数据重新推导为 **37 条**（6 CASCADE + 31 DIRECT_GEOMETRY；文档原记 40，其中
  `ADIR`/`DEPT`/`PPOS` 字典里有名，保留）。删除条件闭合：**与 noun 表同名 ∧ 字典 4270 名无 ∧ 运行库 701 名无**。

同一轮还把 DCHC 字典快照编译期嵌入（`gen_dchc_fixture.py` 提炼 → `dchc_change_classes.json` 随源码入库），
`raw_dchc_code` 覆盖从 2/702（仅强制码）扩到全字典 4270 条。这不只是覆盖率数字——
此前两道字典对账守护会因 `output/` 导出缺失而**静默跳过**，在新环境 / CI 里实际是空转的。

## 验证

五道守护测试：

| 测试 | 钉住什么 |
|---|---|
| `attribute_effect_tables_have_no_duplicate_names` | 跨表重名即失败 |
| `direct_geometry_table_maps_to_its_declared_effect_and_action` | 第五张表映射到 DirectGeometry / 重生成动作 |
| `numbered_parameter_variants_fall_back_to_the_prefix_rule` | 序号参数变体走前缀回退 |
| `curated_tables_are_reconciled_against_the_runtime_schema` | 名单中的名字必须存在于运行库 schema（702 名） |
| `exemption_tables_match_the_dictionary_change_class` | DataOnly / TransformOnly 减免名单对账字典设计变化类 |

等效性：对全量 702 条运行库属性名、全部 curated 名与序号变体**逐一对比新旧分类，逐条一致**——
这是等效改造，不改任何一条属性的归类。全量 `cargo test --lib`：**181 passed / 0 failed / 38 ignored**。

一个让人安心的旁证：`DATA_ONLY_ATTR_NAMES` 是唯一一份「直接跳过、什么都不做」的清单，
能对上字典的 3 条（`NAME` / `DESC` / `PURP`）**全部 DCHC = 0**。唯一一处「跳过」判定被一个
完全独立的数据源确认，不存在假中性。

## 规律

**顺序即语义 = 语义无处可查。** 一旦「A 表优先于 B 表」这种规则只存在于代码的书写顺序里，
它就既不能被测试、也不能被阅读，还会在下一次重排 / 格式化时静默改变。把仲裁规则显式成数据
（一张 `&[(&[&str], Effect)]`），重名就从「按顺序悄悄胜出」变成「编译后第一条测试就报错」。

配套的一条：**清单类代码必须有外部对账。** 名单是人手攒的，唯一能防它漂移的是拿一份独立数据源
（这里是运行库 schema + E3D 字典）逐条比对，并把差集钉成快照。

## 关联

- 案例 [02 TRANSFORM_ONLY 收窄](case-02-transform-only-was-too-wide.md)（这次重构消除了它所依赖的隐式顺序）
- 案例 [06 建边资格解耦](case-06-ref-edge-eligibility-decoupled.md)（同一轮审核发现的另一处「两件事被绑在一起」）
- [`../../docs/2026-07-26_p3-t903-t904-assessment.md`](../../docs/2026-07-26_p3-t903-t904-assessment.md)
