# -*- coding: utf-8 -*-
"""生成《增量模型生成规则 · 原理与示例》PPT（封面 + 5 页正文）。

用法：
    python scripts/make_increment_rules_ppt.py [输出路径] [--blue #RRGGBB]

默认输出 docs/slides/增量模型生成规则-原理与示例.pptx。

只讲原理，不讲实现流程：
    01 属性更新 vs 模型更新的区别（数据的更新规则）
    02 什么时候触发模型重算（五类变化 → 三种处置）
    03 属性分三类（以 E3D 属性字典 DCHC 为准）
    04 对照 E3D core.dll 自己的三条更新通路
    05 改一个元素重算谁（生成根 + 联动）
    06 ams8000 真实库里十个窗口的示例

主题与基础件复用 make_increment_arch_ppt.py（同目录）；品牌色同样由 `--blue` / `DECK_BRAND_BLUE` 控制，默认 #005BAC。
内容出处：CONTEXT.md 术语表；
    docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md §3 / §5；
    docs/2026-09-04_model-cascade-update-inventory.md §1；
    ADR-009（搬迁）/ ADR-066（刚体前移）；teach/cases/case-01、case-02。
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from pptx import Presentation  # noqa: E402
from pptx.dml.color import RGBColor  # noqa: E402
from pptx.enum.text import MSO_ANCHOR, PP_ALIGN  # noqa: E402
from pptx.util import Inches, Pt  # noqa: E402

from make_increment_arch_ppt import (  # noqa: E402
    ACCENT, ACCENT_ON_DARK, BLUE_200, BLUE_300, BODY_TOP, CONTENT_W, INK,
    INK_SOFT, ML, PANEL, SLATE, SLIDE_H, SLIDE_W, TEAL, WHITE,
    arrow, card, codeblock, foot, head, new_slide, note, rect, rich, table,
    textbox,
)

DIM = RGBColor(0xA8, 0xBC, 0xCB)
DIMMER = RGBColor(0x8B, 0xA1, 0xB2)


# ---------------------------------------------------------------- 封面


def slide_cover(prs):
    s = new_slide(prs, dark=True)
    rect(s, Inches(0), Inches(0), Inches(0.26), SLIDE_H, fill=ACCENT_ON_DARK)

    tf = textbox(s, Inches(1.1), Inches(1.95), Inches(10.6), Inches(0.4))
    rich(tf.paragraphs[0], "gen-model  ·  Incremental Model Generation  ·  Principles",
         14, ACCENT_ON_DARK, bold=True)

    tf = textbox(s, Inches(1.1), Inches(2.45), Inches(11.0), Inches(1.0))
    rich(tf.paragraphs[0], "模型增量生成规则", 50, WHITE, bold=True)

    tf = textbox(s, Inches(1.1), Inches(3.45), Inches(11.0), Inches(0.6))
    rich(tf.paragraphs[0], "什么时候要重算模型 · 属性更新与模型更新的区别", 24, DIM)

    rect(s, Inches(1.12), Inches(4.35), Inches(1.5), Pt(3), fill=ACCENT_ON_DARK)

    tf = textbox(s, Inches(1.1), Inches(4.7), Inches(11.0), Inches(1.2))
    p = tf.paragraphs[0]
    p.line_spacing = 1.45
    rich(p, "原理与示例：一份改动进来，属性怎么更新、模型要不要重算、重算谁",
         15, RGBColor(0xC7, 0xD4, 0xDE))
    p = tf.add_paragraph()
    p.line_spacing = 1.45
    rich(p, "六页正文（含一页 core.dll 对照）  ·  不讲实现流程  ·  口径以 E3D 属性字典（DCHC）与 `CONTEXT.md` 术语为准",
         12.5, DIMMER, mono_color=DIMMER)
    return s


# ---------------------------------------------------------------- 01 区别


def slide_two_updates(prs):
    s = new_slide(prs)
    head(s, "一份改动，两条更新链：属性更新无条件，模型更新有条件", kicker="01 · 区别")

    top = Inches(1.66)
    box = rect(s, ML, top, CONTENT_W, Inches(0.78), fill=INK, radius=0.08)
    tf = box.text_frame
    tf.vertical_anchor = MSO_ANCHOR.MIDDLE
    tf.margin_left = Inches(0.24)
    tf.margin_right = Inches(0.2)
    p = tf.paragraphs[0]
    p.alignment = PP_ALIGN.LEFT
    p.space_after = Pt(3)
    rich(p, "同一份输入：设计师 SAVEWORK，库文件从会话 S 走到 T　→　比两端，每个元素得到**恰一条**净变化",
         13.5, WHITE, mono_color=BLUE_200)
    p = tf.add_paragraph()
    p.alignment = PP_ALIGN.LEFT
    rich(p, "新建 · 修改（带「变了哪些属性」）· 删除 · 结构变化（OWNER / 成员 / 顺序）。"
            "窗口内新增后又删除 = 无；记录被重写但内容一字未变 = 无。",
         11, DIM)

    cy, ch = Inches(2.7), Inches(2.02)
    lw = Inches(5.72)
    rx = Inches(6.85)
    card(s, ML, cy, lw, ch, "属性更新（数据面）　凡变必做", [
        "变了的元素，**整条记录的属性重新落库**——不看变的是哪个属性，也不问它影不影响几何。",
        "删除只留**墓碑**（保留类型 / 属主 / 名字）；窗口内改了多次，只落最终态。",
        "元素级、便宜：读记录、写属性行。做完即推进数据水位 `applied_sesno = T`。",
    ], accent=TEAL, size=12)
    card(s, rx, cy, Inches(5.7), ch, "模型更新（模型面）　只在几何输入变了才做", [
        "先判**要不要**：按变化类型与属性类别分三档——重生成 / 只动位姿 / 不动。",
        "再判**算谁**：变化元素上卷到**生成根**（整根 BRAN / EQUI …），一根一窗只算一次。",
        "根级、昂贵：读目录、算网格、替换旧产物。本窗全部根成功才推进 `model_sesno = T`。",
    ], accent=ACCENT, size=12)
    arrow(s, ML + Inches(2.86), top + Inches(0.78), ML + Inches(2.86), cy, color=TEAL, width=1.8)
    arrow(s, rx + Inches(2.85), top + Inches(0.78), rx + Inches(2.85), cy, color=ACCENT, width=1.8)

    rows = [
        ["", "属性更新", "模型更新"],
        ["触发", "记录变了就做，与属性名无关", "几何输入变了才做：看属性类别 + 结构变化"],
        ["粒度", "元素", "生成根：BRAN / HANG / SUPPO / EQUI，或最近的有意义属主"],
        ["结果", "`applied_sesno = T`", "`model_sesno = T`；允许暂时落后于数据"],
    ]
    table(s, rows, ML, Inches(4.86), CONTENT_W,
          [Inches(1.2), Inches(4.5), Inches(6.07)], size=11.5, row_h=Inches(0.36))

    note(s, "两条链**解耦**：「新属性 + 旧模型」是被承认的可见中间态，不是污染；模型重算失败不阻断属性更新。"
            "例：给 BRAN 改名 → 属性 ✓ 模型 ✗；换 ELBO 的规格 `SPRE` → 两者都 ✓。",
         top=Inches(6.34))
    foot(s, "来源：`CONTEXT.md` §数据面 / §模型面 / §净变化 / §墓碑 / §应用水位 / §模型水位；ADR-056 D1 · D2")
    return s


# ---------------------------------------------------------------- 02 触发条件


def slide_when(prs):
    s = new_slide(prs)
    head(s, "什么时候触发模型重算：五类变化，三种处置", kicker="02 · 触发条件")

    codeblock(s, ML, Inches(1.66), Inches(7.75), Inches(4.3), [
        "对窗口 S→T 里每一个净变化元素 e：",
        "",
        "  新建 e                         ──▶ **重生成** 根(e)            # 新几何要生出来",
        "  删除 e                         ──▶ **删除清理** + 重生成原根     # 旧几何不能留",
        "  e 的 OWNER 变了（搬迁）         ──▶ **重生成 旧根 + 新根**       # 双端事件",
        "  e 的成员增删 / 换序             ──▶ **重生成** 根(e)            # 成员序就是管路走向",
        "  e 只改了属性 {a1 … an}          ──▶ 逐个归类，取**最重**的一档：",
        "      任一 ∈ 几何类 · 连通类 · 未知 · UDA        ──▶ **重生成** 根(e)",
        "      全部 ∈ 位姿类 {POS ORI BFORI}",
        "          且 e 不是「几何随摆放位置派生」的件     ──▶ **只动位姿** 根(e)",
        "      全部 ∈ 元数据类（字典 DCHC=0 ∧ 生成器不读）  ──▶ **不动模型**",
        "",
        "  记录被重写、内容一字未变        ──▶ 不算变化",
    ], title="判定规则", size=10.5, accent=ACCENT)

    cx, cw = Inches(8.75), Inches(3.8)
    card(s, cx, Inches(1.66), cw, Inches(1.32), "重生成　Regen", [
        "重新读目录、重新算网格，整根替换旧产物。",
        "唯一真正花钱的一档；**判不准就落这里**。",
    ], accent=ACCENT, size=11.5, title_size=13)
    card(s, cx, Inches(3.12), cw, Inches(1.32), "只动位姿　TransformOnly", [
        "网格原样复用，只给已有几何乘一个刚体变换。",
        "包围盒、空间树、房间归属按新位姿重算。",
    ], accent=BLUE_300, size=11.5, title_size=13)
    card(s, cx, Inches(4.58), cw, Inches(1.38), "不动模型　Skip", [
        "属性照常落库，模型一个字不动：",
        "不上卷、不排工作、不改空间树。",
    ], accent=SLATE, size=11.5, title_size=13)

    note(s, "**宁多勿漏**：重生成永远正确、只是不省。「少算」的两档（只动位姿 / 不动模型）都要**两票**："
            "E3D 属性字典说它不动几何 ∧ 我们的生成器确实不读它（或它确实只是位姿）。",
         top=Inches(6.2))
    foot(s, "来源：`CONTEXT.md` §模型影响 / §搬迁 / §净变化；ADR-009；"
            "`docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md` §5")
    return s


# ---------------------------------------------------------------- 03 属性分类


def slide_attr_classes(prs):
    s = new_slide(prs)
    head(s, "属性分三类：动几何、只动位姿、不动模型",
         kicker="03 · 属性 → 处置　·　口径 = E3D 属性字典的 DCHC（设计变化类别）")

    rows = [
        ["字典类别 DCHC", "条数", "代表属性", "模型处置", "怎么理解"],
        ["**0 · 无设计变化**", "3941",
         "`NAME` `DESC` `PURP` `FUNC` `LCHKDA` `STYP` `HREF` `TREF` …",
         "不动模型（还要第二票：生成器不读）",
         "改名、改描述、改用途、改归档标记——E3D 自己改它时也不重画"],
        ["**3 · 刚体位姿**", "3", "`POS` `ORI` `BFORI`",
         "只动位姿（有例外名单，见「重算范围」页）",
         "整台设备挪 2 m：网格复用、只改矩阵。**位姿类只有这三个**"],
        ["**4 · 几何 / 表示**", "315",
         "`DIAM` `HEIG` `XLEN` `SPRE` `CATR` `LEVE` `POSS` `POSE` `CPOS` `YDIR` `ZDIR` `PARA` `JOIS` …",
         "重生成",
         "尺寸、目录引用、端点、参数、可见性级别——形状或画法变了"],
        ["**1 / 2 · 头 / 尾连通**", "7 / 4",
         "`HBOR` `HDIR` `HPOS` `HSTU` / `TBOR` `TDIR` `TPOS` `LTAI` …",
         "重生成（自身 + 相邻件 / 所属分支）",
         "管件接口变了，两侧隐含管段跟着变"],
        ["**字典没收**", "514", "`UDA:*`、伪属性、长名",
         "重生成（未知按最重处理）",
         "例外：只有缓存戳 `CACHID` / `SPAMAP` 跳动 → 不动，它不是业务变化"],
    ]
    table(s, rows, ML, Inches(1.64), CONTENT_W,
          [Inches(1.75), Inches(0.72), Inches(4.05), Inches(2.4), Inches(2.85)],
          size=10.5, row_h=Inches(0.56))

    cy, ch = Inches(5.12), Inches(1.7)
    card(s, ML, cy, Inches(5.72), ch, "名字里带 POS，不等于位姿", [
        "`POSS` / `POSE` 是型材 `SCTN` 的起点 / 终点——决定型材长度；`CPOS` 是曲线几何参数；"
        "`NPOS` 是节点位置。它们都是几何参数 → **重生成**。",
        "往「少算」名单里加名字要证据，删名字只是变慢。",
    ], accent=ACCENT, size=10.5, title_size=12.5)
    card(s, Inches(6.85), cy, Inches(5.7), ch, "「不动模型」为什么要两票", [
        "第一票：字典 DCHC=0（E3D 交互修改时不重画）。第二票：我们的生成器不读它。",
        "缺第二票的 6 个：`DIR` `FILL` `GTYP` `PKEY` `UKEY` `VXREF`——字典说 0，但它们是我们几何的输入，照旧重生成。",
        "E3D 跨会话收变更时根本不分属性、整件重画；我们比它细，所以每一步省算都得自证。",
    ], accent=TEAL, size=10.5, title_size=12.5)

    foot(s, "来源：`docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md` §3 / §5；"
            "`teach/cases/case-02`；字典 4270 条")
    return s


# ---------------------------------------------------------------- 04 重算范围


def slide_root_cascade(prs):
    s = new_slide(prs)
    head(s, "改一个元素，重算谁：先上卷到生成根，再看联动", kicker="05 · 重算范围")

    lw = Inches(5.75)
    codeblock(s, ML, Inches(1.64), lw, Inches(3.72), [
        "SITE",
        "└ ZONE                       容器：自己不生成",
        "  └ **BRAN**  ◀── 生成根         交付单元 BRAN / HANG / SUPPO / EQUI",
        "    ├ ELBO ── 改 SPRE ──┐",
        "    ├ TUBI              │  上卷到最近的交付单元",
        "    └ VALV ── 改 POS ───┘  ──▶ **整根 BRAN 重算**",
        "                          （TUBI 是派生几何，随根一起生成）",
        "",
        "STRU",
        "└ FRMW",
        "  └ **SUPPO** ◀── 生成根",
        "    └ GENSEC",
        "      └ SPINE ── 改 POS ──▶ **SUPPO 重算**（SPINE 只是父件的输入）",
    ], title="几何按「根」生成，不按元素", size=10.5, accent=ACCENT)

    card(s, ML, Inches(5.48), lw, Inches(1.34), "上卷的三条纪律", [
        "一个变化元素归到**恰好一个**根；同一根一窗只算一次。",
        "没有交付单元祖先 → 最近的非 SITE/ZONE 属主作根；`WORL / SITE / ZONE` 与管件 `FTUB` 永不作根。",
        "loop 容器（`LOOP PLOO PAVE VERT`）自己不成根，变化归属主。",
    ], accent=TEAL, size=10.5, title_size=12.5)

    rx, rw = Inches(6.75), Inches(5.8)
    rows = [
        ["A 变了", "A 以外谁跟着算"],
        ["`ZONE` / `SITE` 改 `POS` `ORI`", "整棵子树的根：只动位姿，不重网格"],
        ["删除 `ZONE`", "子树所有根：删除清理"],
        ["`ELBO` 从 BRAN-A 搬到 BRAN-B", "A、B **两根都重算**"],
        ["目录件（共享 `SPCO`）改参数", "引用它的**全部**设计根（例：1 个 SPCO → 72 根 BRAN）"],
        ["`PANE` / 楼板 / 墙 变了", "该库全部根的**房间归属**重算，几何不重算"],
        ["`BRAN` 里的 `VALV` 挪位", "整根 BRAN 重算——两侧隐含管段长度变了"],
    ]
    table(s, rows, rx, Inches(1.64), rw, [Inches(2.4), Inches(3.4)], size=10.5,
          row_h=Inches(0.5))

    card(s, rx, Inches(5.3), rw, Inches(1.52), "「只动位姿」的例外——照旧重生成", [
        "管件 / 支吊架元件（管段跟着变）· 面板顶点 `PAVE`（面板形状变）· 孔洞 / 负体（属主几何变）",
        "几何随摆放位置派生的 23 类件：门窗 `DOOR` `WINDOW`、栏杆 `HRPOST` `HRGATE` `KICKPL`、"
        "楼梯 `STRFLT` `STRLNG`、桥架支架 `CTSUPP`、标注 `LINDIM` `MLABEL` …——形状由「放在哪」决定，挪了就得重算。",
    ], accent=BLUE_300, size=10.5, title_size=12.5)

    foot(s, "来源：`CONTEXT.md` §生成根 / §最小交付单元 / §联动更新；"
            "`docs/2026-09-04_model-cascade-update-inventory.md` §1；ADR-009 / ADR-066")
    return s


# ---------------------------------------------------------------- 05 示例


def slide_examples(prs):
    """十个窗口全部来自真实库 ams8000_0001（AvevaMarineSample 8000 库）：

    九个钉死窗口（`window_root_plan.rs::live_ams8000_pinned_windows_land_in_the_expected_buckets`）、
    2026-09-05 rocksdb-8000 真机增量报告（`docs/rocksdb-8000-*-increment-20260905.md`）、
    2026-08-19 TTY 净窗口续测（`docs/2026-08-12_live-test-ledger.md`）、
    2026-09-04 联动清单 §2.8（277→278）。会话号就是 E3D SAVEWORK 的次序。
    """
    s = new_slide(prs)
    head(s, "举例：ams8000 真实库里的十个窗口，各自触发了什么",
         kicker="06 · 示例　·　库文件 ams8000_0001，会话号 = SAVEWORK 次序，refno 为 dbnum/序号")

    rows = [
        ["#", "会话窗口", "设计师在 E3D 里做了什么", "属性更新", "模型更新", "为什么"],
        ["1", "195→196",
         "EQUI `24384/26186` 只改 `POS`，平移 10 mm",
         "✓",
         "**只动位姿**：`mesh_id` 复用、矩阵差 9e-13，transform=1",
         "纯刚体位姿；子件随根一起前移"],
        ["2", "267→268",
         "整条 BRAN `24384/26261` `BY U 2000`：9 个成员 `POS` 连带，BRAN 自己的 `HPOS` `TPOS` 也变",
         "✓",
         "**整根 BRAN 重算**（regen 1），不走只动位姿",
         "头尾连通属性变了、管段是派生几何；单挪一个 FTUB（268→269）同样整根"],
        ["3", "277→278",
         "接管：BRAN `24384/26281` 只有 `HPOS` / `TPOS` 变（E3D 回写头尾位置）",
         "✓",
         "**整根 BRAN 重算**",
         "连通类属性（字典类别 1 / 2）→ 重生成"],
        ["4", "288→291",
         "EQUI `24384/24776` 下 BOX `24384/26291` 的 `XLEN` 500→900→700",
         "✓",
         "**EQUI 根重算**；UI 显示两件不同尺寸的 BOX",
         "`XLEN` 是尺寸属性（字典类别 4）"],
        ["5", "294→296",
         "同一 EQUI 下 NBOX `24384/26293` 孔宽 300→500、孔心沿 E 移 150 mm",
         "✓",
         "**EQUI 根重算**：体积 364M→340M mm³；移孔后体积不变、孔位变",
         "负体被属主消费：挪它就是改属主几何，不算只动位姿"],
        ["6", "目录实例窗",
         "VALV `24384/26641`（BRAN `26640`）换规格：SPCO 10→25，即 `SPRE` 变",
         "✓",
         "**整根 BRAN 重算**；网格哈希与体积都变，未手动 ensure",
         "目录引用是几何输入（字典类别 4）"],
        ["7", "45→46",
         "BOX `24384/25802` 从 EQUI `25801` 改挂到 EQUI `25803`",
         "✓ `OWNER`",
         "**新旧两根都重算**（regen 2）",
         "搬迁是双端事件；旧根名下还留着它的旧几何"],
        ["8", "266→267",
         "`NEW BRAN` `24384/26261` + `COPY` 9 个路由成员",
         "✓",
         "**新根重算**（regen 1）；PIPE `24384/23225` 成员变了但不成根",
         "新建 → 所属根整根生成；PIPE 不是交付单元"],
        ["9", "269→271",
         "先 `DELETE` FTUB 4，再 `DELETE` 整条 BRAN `26261`",
         "✓ 留墓碑",
         "删成员：**整根重算**；删整条：**删除清理**(26261)，regen 0",
         "成员删了根还在 → 重算；根没了 → 清掉名下全部几何（24→26 删 EQUI 子树同理）"],
        ["10", "230→232",
         "FTUB `POS` U 2900→3400→2900，三会话合成一窗",
         "✓ 只剩戳",
         "**不动模型**",
         "净变化为零，只剩 BRAN 的 `CACHID`；缓存戳不是业务变化"],
    ]
    tbl = table(s, rows, ML, Inches(1.58), CONTENT_W,
                [Inches(0.4), Inches(0.95), Inches(3.45), Inches(0.95), Inches(2.7), Inches(3.32)],
                size=10, row_h=Inches(0.45))
    tbl.rows[0].height = Inches(0.32)

    note(s, "三句话：**属性更新凡变必做、不看属性名**；**模型更新只在几何输入变了才做、按生成根整根算**；"
            "**判不准就多算，少算必须两票**。",
         top=Inches(6.45))
    foot(s, "来源：ams8000_0001 九个钉死窗口（`window_root_plan.rs`）· `docs/rocksdb-8000-*-increment-20260905.md` · "
            "08-12 live 台账 · 09-04 联动清单 §2.8")
    return s


# ---------------------------------------------------------------- 06 对照 core.dll


def slide_core_paths(prs):
    """E3D core.dll / Core3D 自己的三条更新通路（ida-bridge 取证，
    `docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md` §0 / §2 / §4.4）。"""
    s = new_slide(prs)
    head(s, "对照：E3D core.dll 自己怎么决定「重画谁」——三条通路",
         kicker="04 · 对照 core.dll　·　我们站在第 ② 条路的位置，借第 ① 条路的属性语义做细分")

    cy, ch, cw = Inches(1.64), Inches(2.42), Inches(3.8)
    gap = (CONTENT_W - cw * 3) // 2
    card(s, ML, cy, cw, ch, "① 会话内交互路　最细", [
        "**触发**：用户在 E3D 里改属性（`putAtt` → `VDESPT` → `EVALAT` → `UPGRPH`）。",
        "**判别**：按属性字典 DCHC **逐属性**。类别 0 → 不动；类别 3 → 只改图形段矩阵（`STRTRA` / `SEGTRA`）；"
        "类别 4 → 重建：顶层件整件 `REPELE`，其下单个图元只重建自己那段 `SEGREP`；类别 1 / 2 → 转成 4 落到相邻件 / 所属分支。",
        "规则表 A–T 负责上卷与联动（owner、结构件、点类、连接）。",
    ], accent=ACCENT, size=10.5, title_size=13)
    card(s, ML + cw + gap, cy, cw, ch, "② GETWORK / UNDO 路　最粗", [
        "**触发**：别人 SAVEWORK 了、我把变更收进来——与 gen-model 的场景**同构**。",
        "**判别**：不看任何属性，记录被重写就算变。每个变更元素折到顶层可画件 → **整件重画**；改个 `NAME` 也整件重画。"
        "唯一的属性门：`TUBI` 只问 `POS` `ORI` `ITLE` `SPRE`。",
        "只收 DESI / ISOD 两种库；目录库变了**不进这条路**（靠缓存整体失效 + 用户重新导航）。",
    ], accent=TEAL, size=10.5, title_size=13)
    card(s, ML + (cw + gap) * 2, cy, cw, ch, "③ 图形缓存路　只看一个戳", [
        "**触发**：`cacheGetworkChanges` 开着时。",
        "**判别**：修改类变更只认 `CACHID` 一个属性——「几何变没变」交给写 `CACHID` 的那一方（Design 应用侧）。",
        "语料普查：13724 次跳动全是 +1 计数；「几何属性变 ⇒ `CACHID` 变」98.9% 成立，反向只 2.7%（92.8% 是整库扫写）"
        "→ 我们只拿它作离线校验，不作判据。",
    ], accent=SLATE, size=10.5, title_size=13)

    rows = [
        ["", "core ① 交互路", "core ② GETWORK 路", "gen-model"],
        ["判别粒度", "逐属性（字典 DCHC）", "无：记录重写即变", "逐属性 + 结构变化；预览与执行同一把尺子"],
        ["只动位姿", "类别 3 只改矩阵；P23 / 管件 / 点类 / 负体例外", "整件重画", "刚体前移；例外名单按 `UPGRPH` 逐条对齐"],
        ["不动模型", "类别 0：3941 个属性", "没有", "DCHC=0 ∧ 生成器不读：3941 − 6 个"],
        ["重建粒度", "整件 `REPELE` / 单图元 `SEGREP` / 全部 `REPALL`", "顶层件整件", "整根重算；单元级只重建变了的件是第二例外（默认关）"],
        ["目录变了", "`SPCO` / `SVER` / `DATA` 变 → 全部重画", "不进这条路", "依赖闭包 + 引用表反查 → 只算受影响的根（比 core 强）"],
    ]
    tbl = table(s, rows, ML, Inches(4.18), CONTENT_W,
                [Inches(1.3), Inches(3.5), Inches(2.2), Inches(4.77)], size=10.5,
                row_h=Inches(0.35))
    tbl.rows[0].height = Inches(0.32)

    note(s, "结论：core 在 ② 上**不分属性、整件重画**；我们在同一位置借 ① 的 DCHC 语义做细分——所以每一步「少算」都要两票自证，"
            "「多算」随时可做；③ 的 `CACHID` 只作离线校验。",
         top=Inches(6.34))
    foot(s, "来源：`docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md` §0 / §2 / §4.4（ida-bridge 实读 core.dll · Core3D.dll，E3D 3.1）")
    return s


# ---------------------------------------------------------------- 组装


def build(out_path: Path) -> Path:
    prs = Presentation()
    prs.slide_width = SLIDE_W
    prs.slide_height = SLIDE_H

    slide_cover(prs)
    slide_two_updates(prs)
    slide_when(prs)
    slide_attr_classes(prs)
    slide_core_paths(prs)
    slide_root_cascade(prs)
    slide_examples(prs)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    prs.save(str(out_path))
    return out_path


def main() -> None:
    args = []
    skip = False
    for a in sys.argv[1:]:
        if skip:
            skip = False
            continue
        if a == "--blue":
            skip = True
        elif not a.startswith("--blue="):
            args.append(a)

    repo = Path(__file__).resolve().parents[1]
    default = repo / "docs" / "slides" / "增量模型生成规则-原理与示例.pptx"
    out = Path(args[0]).resolve() if args else default
    saved = build(out)
    print(f"slides: {len(Presentation(str(saved)).slides._sldIdLst)}")
    print(f"saved : {saved}")


if __name__ == "__main__":
    main()
