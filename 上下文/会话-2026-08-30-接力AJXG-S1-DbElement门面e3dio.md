# 会话上下文 — 2026-08-30 · 接力 AJXG:S1 DbElement 门面落 e3d-io(CE1Y)

> 本会话:BajieAsk-agent-1-05196a3e(CE1Y)。20:33 经 handoff 接入 AJXG
> (BajieAsk-agent-1-30bf5315)存档(11 条,17K 字符,已通读到底)。
> AJXG 档案《会话-2026-08-30-接力MD6I-AJXG.md》工作日志停在 19:26;
> 用户 19:38 拍板后 AJXG 未再动作(两仓 git 无新提交、无门面代码)→ 拍板零落地,本会话承接。

## 接手时盘面(20:3x 核实)

- gen-model HEAD = `b2b954ae`、e3d-io HEAD = `c8aca10`,与 AJXG 记录一致,无新提交。
- e3d-io 工作区他人 M 文件:`examples/record_continuation_probe.rs`、
  `examples/record_header_address_probe.rs`、`src/index/mod.rs`、`src/meta/constants.rs`、
  `tests/index_page_decode.rs` + 若干 ?? examples/tests。**一律不碰。**
- `src/lib.rs` 干净 → 加一行模块导出属本会话自己的改动,可入库。

## 用户拍板链(权威)

1. 19:07「我需要的是对标 core.dll 里的 api 函数」→ AJXG 产出对标矩阵
   `docs/plans/2026-08-30-core-dll-api-alignment.md`(A–J 十组,S1–S4 顺序)。
2. 19:38 决策卡答复:「**开写,但放 e3d-io crate**」——S1 DbElement 门面落 e3d-io,
   否掉了 AJXG 推荐的 gen-model direct 层。本会话执行该拍板。

## 本会话关键设计发现

- **跨库路由不需要注入定位器**:`RefNo::dbno()`(refno.rs,位布局来自 core.dll
  `sub_5AEB6B0`:`dbno = (word0 & 0x1FFF) | ((word0 >> 13) & 0x3E000)`)本身携带库号。
  验证:BRAN 24384/23257 → dbno()=8000 ✓。AJXG 当初「schema/locator 倒灌」的顾虑
  大半消解:门面只需 DbSet 按 dbnum 注册已开引擎,跨库 ref 用 `refno.dbno()` 路由,
  未注册库 fail loud。文件路径发现(CataDbLocator)留在消费方,不进 e3d-io。
- typed getter 架在 **descriptor 权威提取管线**上
  (`extract_parsed_element_with_descriptors` → `ElementExtraction`;
  `resolve_attribute`/`summarize_element_with_template` 均已 `#[deprecated]`,不用)。
- 导航架在 `ParsedElement`(owner + members 原序,G3 约束)上;NXTITM 游标语义
  (首/next/-1 结束)以 first_member/next_sibling 落地。
- 模板接线抄 gen-model `DirectStore::read_attrs`:每库一个 `TemplateProvider`
  (`{库类型前三字母小写}vir.dat`,期望 noun = db1_hash(库类型));attlib 全局一份。

## 实施(S1 第一轮闭环)

新文件 `vendor/e3d-io/src/dbelement.rs`:
- `DbSet`:attlib + 按 dbnum 注册 (engine, provider);`element(refno)` 按 dbno 路由;
  `find_named(dbnum, name)`(Named 层全扫,重名不折叠;加速索引属消费方)。
- `DbElement`:惰性句柄,OnceLock 缓存 ParsedElement/ElementExtraction。
  - A 身份:refno/db_no/is_null/exists/noun_hash/element_type/name
  - B 导航:owner/members(原序)/member(i)/first/last/next_sibling/previous_sibling/
    members_of_type/is_descendant_of(owner 链,防环上限)
  - C typed getter:get_string/integer/double/bool/word/refno/position/direction/
    orientation/real_array/integer_array/refno_array(严格变体投影,Unset→None,
    类型不符→TypeMismatch,属性不在 noun→AttributeUnknown)
  - D 跨库:get_element/get_element_array(nulref→None/保位;dbno 路由,未注册 fail loud)
- 测试 `tests/dbelement_facade.rs`:真语料 ams8000(+5052 跨库),真值锚
  `descriptor_extraction_real.rs`(BRAN 24384/23257、FTUB 24384/23262)。

范围外(记账):E 加速名字索引(gen-model direct_index)、世界元素定位法(矩阵 B 组
未定)、S2 差分门面、S3 表达式、qualifier/UDA 转换器(矩阵 C 组真缺口)。

## 工作日志

- 20:33 收 handoff;通读 AJXG 存档 11 条 + AJXG 档案 + 对标矩阵全文 + task_plan。
- 20:3x 核实两仓 git:拍板零落地坐实;摸清 e3d-io 提取管线/模板接线/RefNo 位布局。
- 20:4x 开写 src/dbelement.rs。
