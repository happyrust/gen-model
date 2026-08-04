# 0007 — core.dll 的 back-ref 属性与 PostSetRefListAttribute 广播器（实测）

- **日期**：2026-07-26
- **背景**：讲 `ref_rev` 时要说清「E3D 原生怎么做反向引用」，把 `BREF`/`SPBREF`/`SCBREF`/`TABREF` 与
  `DB_ElementChangesPlugger::PostSetRefListAttribute` 从文档转述升级为实测证据。
- **工具**：`ida-pro-mcp` 会话 `core31-retrace`（`D:\AVEVA\Everything3D3.1\core.dll`，imagebase `0x5170000`）。
- **课件**：`teach/lessons/0002-ref-rev-reverse-reference-index.html` §9

## 关键洞见（均有地址）

1. **back-ref 名字确实是真属性**。每个名字在属性名字符串区有字符串，各被**一个 0xDB 字节的静态初始化器**引用，
   模板一致：`operator new(0x114)` → `std::string("名字")` → `DB_Attribute::DB_Attribute` → 存进 `ATT_*` 全局。

   | 属性 | 字符串 | 初始化器 | 全局 |
   |---|---|---|---|
   | `SPBREF` | `0x5D7FA54` | `sub_57675F0` | `ATT_SPBREF` @ `0x64203E4` |
   | `SCBREF` | `0x5D7D604` | `sub_575D4F0` | 同款 |
   | `TABREF` | `0x5D84578` | `sub_577BC50` | 同款 |
   | `GOBREF` | `0x5D82980` | `sub_56CB910` | 同款 |
   | `HDBREF` | `0x5D83A50` | `sub_56D07D0` | 同款 |
   | `DBREF`  | `0x5D7EBA4` | 同区 | 同款 |

2. **反证：back-ref 属性和正向属性构造上没有区别**。`ATT_SPRE` 的初始化器 `sub_576D8D0` 同样是 0xDB 字节的
   同一模板。它们就是普通属性，差别只在语义与「谁来写」。

3. **`PostSetRefListAttribute` 是广播器，不是维护者**（此前记录的措辞偏简，这里更正）。
   `DB_ElementChangesPlugger::PostSetRefListAttribute` @ `0x591E780`：
   ```c
   if (!DB_Attribute::wnoevt(a3)) {            // 闸门
     v6 = *(this+76); v5 = *(this+77);          // 订阅者指针数组 [begin,end)
     do { (**v6)(*v6, a2, a3, a4); ++v6; } while (--v8);   // 虚调用转发
   }
   ```
   参数三件套 = (改动的元素 `DB_Element const&`, 被写的引用属性 `DB_Attribute const*`,
   新引用目标 `std::vector<DB_Element> const&`)，正好是维护反向索引所需的全部信息。
   - 订阅入口：`DB_ElementChangesPlugger::SubscribePostSetRefListAttribute` @ `0x581F7E0`
   - handler 基类：`DB_PostSetRefListAttributeHandler` ctor @ `0x581B400` / `0x581B410`，dtor @ `0x581BE50`
   - ~~真正写 back-ref 的是订阅它的 handler，尚未逐个定位~~ → **2026-07-26 已定位，见 `0008`**：
     订阅者只有 `Core3D.dll` 的 `DESDRA_SCPlugs` 与 `AfiModeling.dll` 的 `ElementEvents`；
     前者经 `descases/VDESFA` → `structures/BAKREF` / `BREAKF` 维护的是**连接型** back-ref
     （`CREF`/`HREF`/`TREF`/`CRFA`/`JOIS`/`JOIE`…），**不是** `SPBREF` 家族；后者只转发事件。

4. **`DB_Attribute::wnoevt`（want-no-event）是字典驱动的闸门** @ `0x58D5290`：
   `this+8` 未加载则走 vtable 槽 `+20` 懒加载 schema，返回 `this+0xB8` 的布尔。
   与 `DB_Noun::graphicsBehaviour`（`this+0xB4`，懒加载）**同一套路** —— 「哪些属性静默」由字典决定，不在二进制里。

## 对 gen-model 的启示

- `maintain_reverse_index` 的挂点选择（落库写引用属性处）与 core.dll 的 `PostSetRefListAttribute` 一致，
  拿到的信息也同构（`extract_reverse_ref_edges` 的 referrer / 引用属性 / 后态目标集）。
- **离线仍不可得**：这些 back-ref 名字不在 `vendor/aios-parse-pdms/all_attr_info.json`（grep 零命中），
  PDMS 里它们是独立引用表 / 系统维护结构，不是元素隐含块的固定偏移属性 → 只能自建 `ref_rev`（ADR-003）。

## 未决

- **back-ref 属性的 `wnoevt` 是否为 true**（防止写 back-ref 递归触发事件）：值在字典里，静态分析读不到，
  需活 E3D 会话查 schema 才能证实。目前是推断。
- ~~**哪些 handler 订阅了 `PostSetRefListAttribute`**~~ → **已解决，见 `0008`**。顺带发现：
  该导出在 core.dll 内部零调用方，订阅全部来自外部模块；**写 `SPBREF` 家族的那一位仍未找到**
  （不在这两个订阅者里，很可能由 dabacon 内核自己维护），该问题转记到 `0008` 未决。
- **名字拆解**（`SP`/`SC`/`GO`/`HD` + `BREF`，以及少了 B 的 `TABREF`）字面不唯一，语义以字典定义为准。
