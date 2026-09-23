# -*- coding: utf-8 -*-
"""生成《增量模型生成 · 技术架构》PPT。

用法：
    python scripts/make_increment_arch_ppt.py [输出路径]

默认输出 docs/slides/增量模型生成-技术架构.pptx。
内容出处：CONTEXT.md（术语真值）与 docs/adr/（决策真值）。
"""

from __future__ import annotations

import colorsys
import os
import re
import sys
from pathlib import Path

from pptx import Presentation
from pptx.dml.color import RGBColor
from pptx.enum.shapes import MSO_CONNECTOR, MSO_SHAPE
from pptx.enum.text import MSO_ANCHOR, PP_ALIGN
from pptx.oxml.ns import qn
from pptx.util import Emu, Inches, Pt

# ---------------------------------------------------------------- 主题

# 公司蓝白：整条色阶从一个品牌蓝推出来，不用橙 / 绿 / 红。
# 换品牌色只改 BRAND_BLUE（或命令行 --blue #RRGGBB），其余各阶按明度重推。
def _brand_from_cli(default="#005BAC"):
    """在任何常量定义之前解析，好让下面这些颜色（含函数默认参数）一次绑定到位。"""
    for i, a in enumerate(sys.argv):
        if a == "--blue" and i + 1 < len(sys.argv):
            return sys.argv[i + 1]
        if a.startswith("--blue="):
            return a.split("=", 1)[1]
    return os.environ.get("DECK_BRAND_BLUE", default)


BRAND_BLUE = _brand_from_cli()


def _ramp(brand_hex):
    h, l, sat = colorsys.rgb_to_hls(
        *(int(brand_hex.lstrip("#")[i:i + 2], 16) / 255 for i in (0, 2, 4)))

    def at(lightness, saturation=None):
        r, g, b = colorsys.hls_to_rgb(h, lightness,
                                      sat if saturation is None else saturation)
        return RGBColor(round(r * 255), round(g * 255), round(b * 255))

    # 浅色两阶跟着品牌色自己的明度走：品牌蓝本来就亮时，固定值会和它撞成同一个色。
    light = min(max(l + 0.18, 0.55), 0.78)
    lighter = min(max(l + 0.32, 0.70), 0.88)
    # 浅色两阶同时压一点饱和度：高饱和的品牌蓝直接提亮会刺眼。
    return {
        "BLUE_900": at(0.17), "BLUE_800": at(0.24), "BLUE_700": at(0.30),
        "BLUE_500": at(l),
        "BLUE_300": at(light, sat * 0.82), "BLUE_200": at(lighter, sat * 0.60),
        "SLATE": at(0.51, 0.16),
    }


_R = _ramp(BRAND_BLUE)
BLUE_900, BLUE_800, BLUE_700 = _R["BLUE_900"], _R["BLUE_800"], _R["BLUE_700"]
BLUE_500, BLUE_300, BLUE_200 = _R["BLUE_500"], _R["BLUE_300"], _R["BLUE_200"]
SLATE = _R["SLATE"]

INK = BLUE_900
INK_SOFT = RGBColor(0x35, 0x52, 0x70)
MUTED = SLATE
ACCENT = BLUE_500          # 主强调
ACCENT_ON_DARK = RGBColor(0x7C, 0xBA, 0xEB)
TEAL = BLUE_700            # 数据面
GREEN = BLUE_300           # 收口 / 完成态
RED = BLUE_900             # 关键 / 危险，用最深的蓝
LINE = RGBColor(0xD2, 0xDD, 0xE7)
PAPER = RGBColor(0xFB, 0xFC, 0xFD)
PANEL = RGBColor(0xEC, 0xF2, 0xF8)
WHITE = RGBColor(0xFF, 0xFF, 0xFF)

CJK = "微软雅黑"
MONO = "Consolas"

SLIDE_W = Inches(13.333)
SLIDE_H = Inches(7.5)
ML = Inches(0.78)
CONTENT_W = Inches(11.77)
BODY_TOP = Inches(1.72)
FOOTER_Y = Inches(6.92)

_page = {"n": 0}


# ---------------------------------------------------------------- 基础件


def half(length):
    """EMU 必须是整数：`Length / 2` 会变成 float，写进 XML 后 PowerPoint 直接拒收。"""
    return Emu(int(length) // 2)


def _tf(shape):
    tf = shape.text_frame
    tf.word_wrap = True
    tf.margin_left = 0
    tf.margin_right = 0
    tf.margin_top = 0
    tf.margin_bottom = 0
    return tf


def textbox(slide, left, top, width, height):
    return _tf(slide.shapes.add_textbox(left, top, width, height))


_TOKEN = re.compile(r"(`[^`]*`|\*\*[^*]+\*\*)")


def rich(par, text, size, color=INK, bold=False, mono_color=None, italic=False):
    """把 `等宽` 与 **加粗** 标记切成独立 run。"""
    mono_color = mono_color or TEAL
    for chunk in _TOKEN.split(text):
        if not chunk:
            continue
        if chunk.startswith("**") and chunk.endswith("**"):
            # **`code`** 这类嵌套：脱掉加粗标记后按内层标记再切一次
            rich(par, chunk[2:-2], size, color, bold=True,
                 mono_color=mono_color, italic=italic)
            continue
        run = par.add_run()
        f = run.font
        f.size = Pt(size)
        f.bold = bold
        f.italic = italic
        if chunk.startswith("`") and chunk.endswith("`") and len(chunk) > 1:
            run.text = chunk[1:-1]
            f.name = MONO
            f.size = Pt(size - 1.0)
            f.color.rgb = mono_color
        else:
            run.text = chunk
            f.name = CJK
            f.color.rgb = color
    return par


def rect(slide, left, top, width, height, fill=None, line=None, radius=None):
    shp = slide.shapes.add_shape(
        MSO_SHAPE.ROUNDED_RECTANGLE if radius else MSO_SHAPE.RECTANGLE,
        left, top, width, height,
    )
    if radius:
        shp.adjustments[0] = radius
    if fill is None:
        shp.fill.background()
    else:
        shp.fill.solid()
        shp.fill.fore_color.rgb = fill
    if line is None:
        shp.line.fill.background()
    else:
        shp.line.color.rgb = line
        shp.line.width = Pt(1.0)
    shp.shadow.inherit = False
    _tf(shp)
    shp.text_frame.word_wrap = True
    return shp


def arrow(slide, x1, y1, x2, y2, color=MUTED, width=1.5):
    conn = slide.shapes.add_connector(MSO_CONNECTOR.STRAIGHT, x1, y1, x2, y2)
    conn.line.color.rgb = color
    conn.line.width = Pt(width)
    ln = conn.line._get_or_add_ln()
    ln.append(ln.makeelement(
        qn("a:tailEnd"), {"type": "triangle", "w": "med", "len": "med"}))
    return conn


def dot(slide, left, top, color, size=Inches(0.17)):
    shp = slide.shapes.add_shape(MSO_SHAPE.OVAL, left, top, size, size)
    shp.fill.solid()
    shp.fill.fore_color.rgb = color
    shp.line.color.rgb = PAPER
    shp.line.width = Pt(2)
    shp.shadow.inherit = False
    return shp


def new_slide(prs, dark=False):
    slide = prs.slides.add_slide(prs.slide_layouts[6])
    bg = slide.background.fill
    bg.solid()
    bg.fore_color.rgb = INK if dark else PAPER
    return slide


def head(slide, title, kicker=None, dark=False):
    y = Inches(0.46)
    if kicker:
        tf = textbox(slide, ML, y, CONTENT_W, Inches(0.26))
        p = tf.paragraphs[0]
        rich(p, kicker, 11.5, ACCENT if not dark else ACCENT, bold=True)
        y = Inches(0.76)
    tf = textbox(slide, ML, y, CONTENT_W, Inches(0.58))
    p = tf.paragraphs[0]
    rich(p, title, 27, WHITE if dark else INK, bold=True, mono_color=ACCENT)
    rule = rect(slide, ML, Inches(1.45), Inches(0.9), Pt(3.2), fill=ACCENT)
    return rule


def foot(slide, source=None, dark=False):
    _page["n"] += 1
    tf = textbox(slide, ML, FOOTER_Y, Inches(9.6), Inches(0.3))
    p = tf.paragraphs[0]
    if source:
        rich(p, source, 9.5, MUTED, mono_color=MUTED if dark else TEAL)
    tf2 = textbox(slide, Inches(11.9), FOOTER_Y, Inches(0.65), Inches(0.3))
    p2 = tf2.paragraphs[0]
    p2.alignment = PP_ALIGN.RIGHT
    rich(p2, f"{_page['n']:02d}", 10, MUTED)


def hanging(par, left, hang):
    """悬挂缩进：首行从 left-hang 起，回行对齐到 left。"""
    pPr = par._p.get_or_add_pPr()
    pPr.set("marL", str(int(left)))
    pPr.set("indent", str(int(-hang)))


def bullets(slide, items, left=ML, top=BODY_TOP, width=CONTENT_W, size=15.5,
            gap=9, dark=False):
    """items: (level, text) 或纯字符串（等价 level 0）。"""
    tf = textbox(slide, left, top, width, Inches(5.0))
    first = True
    for item in items:
        level, text = item if isinstance(item, tuple) else (0, item)
        p = tf.paragraphs[0] if first else tf.add_paragraph()
        first = False
        p.alignment = PP_ALIGN.LEFT
        p.space_after = Pt(gap if level == 0 else gap - 3)
        p.space_before = Pt(0 if level else 3)
        p.line_spacing = 1.16
        if level == 0:
            hanging(p, Inches(0.22), Inches(0.22))
            m = p.add_run()
            m.text = "▍ "
            m.font.size = Pt(size)
            m.font.color.rgb = ACCENT
            m.font.name = CJK
            rich(p, text, size, WHITE if dark else INK)
        else:
            hanging(p, Inches(0.82), Inches(0.26))
            m = p.add_run()
            m.text = "－  "
            m.font.size = Pt(size - 2)
            m.font.color.rgb = MUTED
            m.font.name = CJK
            rich(p, text, size - 2, INK_SOFT if not dark else RGBColor(0xC8, 0xD2, 0xDA))
    return tf


def card(slide, left, top, width, height, title, lines, accent=TEAL, size=12.5,
         title_size=14):
    box = rect(slide, left, top, width, height, fill=WHITE, line=LINE, radius=0.04)
    rect(slide, left, top, Pt(3.6), height, fill=accent)
    tf = box.text_frame
    tf.margin_left = Inches(0.24)
    tf.margin_right = Inches(0.16)
    tf.margin_top = Inches(0.16)
    tf.vertical_anchor = MSO_ANCHOR.TOP
    p = tf.paragraphs[0]
    p.alignment = PP_ALIGN.LEFT
    p.space_after = Pt(6)
    rich(p, title, title_size, INK, bold=True, mono_color=accent)
    for ln in lines:
        p = tf.add_paragraph()
        p.alignment = PP_ALIGN.LEFT
        p.space_after = Pt(4)
        p.line_spacing = 1.12
        rich(p, ln, size, INK_SOFT)
    return box


def table(slide, rows, left, top, width, col_w, header=True, size=12,
          row_h=Inches(0.42)):
    n_rows, n_cols = len(rows), len(rows[0])
    shp = slide.shapes.add_table(n_rows, n_cols, left, top, width, row_h * n_rows)
    tbl = shp.table
    tbl.first_row = header
    for i, w in enumerate(col_w):
        tbl.columns[i].width = w
    for r, row in enumerate(rows):
        tbl.rows[r].height = row_h
        for c, val in enumerate(row):
            cell = tbl.cell(r, c)
            cell.margin_left = Inches(0.12)
            cell.margin_right = Inches(0.08)
            cell.margin_top = Inches(0.05)
            cell.margin_bottom = Inches(0.05)
            cell.vertical_anchor = MSO_ANCHOR.MIDDLE
            cell.fill.solid()
            if r == 0 and header:
                cell.fill.fore_color.rgb = INK
            else:
                cell.fill.fore_color.rgb = WHITE if r % 2 else PANEL
            tf = cell.text_frame
            tf.word_wrap = True
            p = tf.paragraphs[0]
            p.line_spacing = 1.08
            rich(p, val, size, WHITE if (r == 0 and header) else INK_SOFT,
                 bold=(r == 0 and header),
                 mono_color=RGBColor(0x9E, 0xC4, 0xD8) if (r == 0 and header) else TEAL)
    return tbl


def codeblock(slide, left, top, width, height, lines, title=None, size=11.5,
              accent=ACCENT, title_size=14):
    """show-me 的「调用树 / 伪代码」形态：等宽、结构靠缩进，不靠项目符号。

    行首写 `**` 表示这一行是结论行（加粗 + 深色）。
    """
    box = rect(slide, left, top, width, height, fill=PANEL, line=LINE, radius=0.03)
    rect(slide, left, top, Pt(3.2), height, fill=accent)
    tf = box.text_frame
    tf.margin_left = Inches(0.26)
    tf.margin_right = Inches(0.14)
    tf.margin_top = Inches(0.16)
    first = True
    if title:
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.LEFT
        p.space_after = Pt(9)
        rich(p, title, title_size, INK, bold=True, mono_color=accent)
        first = False
    for ln in lines:
        p = tf.paragraphs[0] if first else tf.add_paragraph()
        first = False
        p.alignment = PP_ALIGN.LEFT
        p.space_after = Pt(2)
        p.line_spacing = 1.2
        for j, part in enumerate(ln.split("**")):
            if not part:
                continue
            strong = bool(j % 2)
            run = p.add_run()
            run.text = part
            run.font.name = MONO
            run.font.size = Pt(size)
            run.font.bold = strong
            run.font.color.rgb = INK if strong else INK_SOFT
    return box


def statebox(slide, left, top, width, height, text, fill=WHITE, line=LINE,
             color=INK, size=12, radius=0.16):
    box = rect(slide, left, top, width, height, fill=fill, line=line, radius=radius)
    tf = box.text_frame
    tf.vertical_anchor = MSO_ANCHOR.MIDDLE
    tf.margin_left = Inches(0.08)
    tf.margin_right = Inches(0.08)
    p = tf.paragraphs[0]
    p.alignment = PP_ALIGN.CENTER
    rich(p, text, size, color, bold=True,
         mono_color=BLUE_200 if color == WHITE else BLUE_700)
    return box


def caption(slide, left, top, width, text, size=10.5, color=MUTED,
            align=PP_ALIGN.CENTER, mono_color=None):
    tf = textbox(slide, left, top, width, Inches(0.3))
    p = tf.paragraphs[0]
    p.alignment = align
    p.line_spacing = 1.12
    rich(p, text, size, color, mono_color=mono_color or BLUE_500)
    return tf


def note(slide, text, top=Inches(6.28), accent=ACCENT):
    box = rect(slide, ML, top, CONTENT_W, Inches(0.5), fill=PANEL, radius=0.08)
    rect(slide, ML, top, Pt(3.6), Inches(0.5), fill=accent)
    tf = box.text_frame
    tf.margin_left = Inches(0.24)
    tf.margin_right = Inches(0.18)
    tf.vertical_anchor = MSO_ANCHOR.MIDDLE
    p = tf.paragraphs[0]
    p.alignment = PP_ALIGN.LEFT
    rich(p, text, 12.5, INK, mono_color=accent)
    return box


# ---------------------------------------------------------------- 幻灯片


def slide_cover(prs):
    s = new_slide(prs, dark=True)
    rect(s, Inches(0), Inches(0), Inches(0.26), SLIDE_H, fill=ACCENT_ON_DARK)
    tf = textbox(s, Inches(1.1), Inches(2.02), Inches(10.6), Inches(0.4))
    rich(tf.paragraphs[0], "gen-model  ·  Incremental Model Generation", 14,
         ACCENT_ON_DARK, bold=True)

    tf = textbox(s, Inches(1.1), Inches(2.52), Inches(11.0), Inches(1.0))
    rich(tf.paragraphs[0], "增量模型生成", 50, WHITE, bold=True)

    tf = textbox(s, Inches(1.1), Inches(3.5), Inches(11.0), Inches(0.6))
    rich(tf.paragraphs[0], "技 术 架 构 介 绍", 26, RGBColor(0xA8, 0xBC, 0xCB))

    rect(s, Inches(1.12), Inches(4.42), Inches(1.5), Pt(3), fill=ACCENT_ON_DARK)

    tf = textbox(s, Inches(1.1), Inches(4.78), Inches(11.0), Inches(1.0))
    p = tf.paragraphs[0]
    p.line_spacing = 1.45
    rich(p, "E3D / PDMS 设计数据变更  →  最小范围、正确收敛的三维模型重算", 15,
         RGBColor(0xC7, 0xD4, 0xDE))
    p = tf.add_paragraph()
    p.line_spacing = 1.45
    rich(p, "版本 0.1.28    ·    真值：`CONTEXT.md` 术语表 + `docs/adr/` 决策记录",
         12.5, RGBColor(0x8B, 0xA1, 0xB2), mono_color=RGBColor(0x8B, 0xA1, 0xB2))
    _page["n"] += 0
    return s


def slide_agenda(prs):
    s = new_slide(prs)
    head(s, "本次介绍的五个问题", kicker="AGENDA")
    items = [
        ("01", "为什么必须增量", "全量重算的代价，与「小而正确」这条目标"),
        ("02", "一次增量走过哪些阶段", "从文件落盘到模型可见的完整链路"),
        ("03", "算什么 · 不算什么", "变更发现、选根、影响分类、联动扩散"),
        ("04", "怎么落库、怎么收口", "直写 RocksDB、两枚水位、任务内序列化"),
        ("05", "坏掉了怎么办", "写入标记、整库重初始化、可观测口径"),
    ]
    y = Inches(1.92)
    for idx, title, sub in items:
        box = rect(s, ML, y, CONTENT_W, Inches(0.84), fill=WHITE, line=LINE, radius=0.06)
        rect(s, ML, y, Pt(3.6), Inches(0.84), fill=ACCENT)
        tf = textbox(s, ML + Inches(0.26), y + Inches(0.17), Inches(0.9), Inches(0.5))
        rich(tf.paragraphs[0], idx, 22, ACCENT, bold=True)
        tf = textbox(s, ML + Inches(1.15), y + Inches(0.13), Inches(4.2), Inches(0.35))
        rich(tf.paragraphs[0], title, 16, INK, bold=True)
        tf = textbox(s, ML + Inches(1.15), y + Inches(0.47), Inches(9.8), Inches(0.3))
        rich(tf.paragraphs[0], sub, 12.5, MUTED)
        y += Inches(0.96)
    foot(s)
    return s


def slide_why(prs):
    s = new_slide(prs)
    head(s, "为什么必须增量：全量重算不是一个选项", kicker="01 · 问题")
    card(s, ML, BODY_TOP, Inches(3.75), Inches(2.5), "数据规模",
         ["一个 MDB 下几十个设计库（`dbnum`），单库百万级元素；",
          "AMS 7997 一库 6772 个生成根。",
          "全量解析 + 全量生成以小时计，而设计侧 `SAVEWORK` 以分钟计。"],
         accent=RED)
    card(s, ML + Inches(4.01), BODY_TOP, Inches(3.75), Inches(2.5), "变更规模",
         ["一次 `SAVEWORK` 通常只动几个到几百个元素；",
          "绝大多数元素的几何与上一版逐字节相同。",
          "重算比例失衡是这条链路存在的全部理由。"],
         accent=ACCENT)
    card(s, ML + Inches(8.02), BODY_TOP, Inches(3.75), Inches(2.5), "正确性门槛",
         ["「少算」的错误不报警：模型看着在、其实是旧的。",
          "所以每一处省算都要有明确判据，",
          "并以「增量收敛 == 从零初始化，逐边相等」作硬验收。"],
         accent=TEAL)

    tf = textbox(s, ML, Inches(4.55), CONTENT_W, Inches(0.4))
    rich(tf.paragraphs[0], "设计目标", 16, INK, bold=True)
    bullets(s, [
        "**在已有全量模型之上，只对本次数据变化影响到的部分重新生成**——范围尽量小，结论必须正确。",
        "取「宁多勿漏」：判不准就多算一点；任何一处少算都要有两票独立判据才允许。",
        "模型失败不许阻断数据推进；数据落库与模型追平是两条可以暂时不同步的链。",
    ], top=Inches(4.95), size=14.5)
    foot(s, "来源：`CONTEXT.md` §增量模型生成 / §模型影响；ADR-056 D1")
    return s


def slide_two_faces(prs):
    s = new_slide(prs)
    head(s, "一句话架构：数据面与模型面并行，只共用同一份窗口差分", kicker="02 · 总览")

    # 中间共享件
    shared = rect(s, Inches(4.55), Inches(3.42), Inches(4.2), Inches(0.82),
                  fill=INK, radius=0.1)
    tf = shared.text_frame
    tf.vertical_anchor = MSO_ANCHOR.MIDDLE
    p = tf.paragraphs[0]
    p.alignment = PP_ALIGN.CENTER
    rich(p, "净窗口收集器  IndexDiff(base@S, target@T)", 14, WHITE, bold=True,
         mono_color=RGBColor(0x9E, 0xC4, 0xD8))
    p = tf.add_paragraph()
    p.alignment = PP_ALIGN.CENTER
    rich(p, "每个 refno 恰一条  Add / Modified / Deleted", 11.5,
         RGBColor(0xA8, 0xBC, 0xCB), mono_color=RGBColor(0xA8, 0xBC, 0xCB))

    card(s, ML, Inches(1.85), Inches(5.6), Inches(1.4), "数据面 (Data Face)",
         ["把属性数据摄入持久层、推进 `applied_sesno`。",
          "产物：`pe` / `<NOUN>` / `ATT_UDA` / `pe_owner` 边。"], accent=TEAL)
    card(s, Inches(7.0), Inches(1.85), Inches(5.55), Inches(1.4), "模型面 (Model Face)",
         ["从 (库文件, 会话) 算三维几何，对输入是纯函数。",
          "产物：`inst_relate` / `inst_info` / `tubi_relate` + 房间边。"], accent=ACCENT)

    card(s, ML, Inches(4.5), Inches(5.6), Inches(1.72), "落点：SurrealDB (RocksDB 后端)",
         ["稳态窗口**直写**，不经暂存层（ADR-056）。",
          "元素批：批量删边 + `INSERT IGNORE`；`pe` 行原地替换、从不删。",
          "尾事务最后一笔推进 `applied_sesno`。"], accent=TEAL)
    card(s, Inches(7.0), Inches(4.5), Inches(5.55), Inches(1.72), "落点：模型行 + 两枚水位",
         ["几何 / 房间边与模型行同事务；空间树随提交就地收敛。",
          "任务收尾一笔盖 `model_sesno`——模型面唯一的持久事实。"], accent=ACCENT)

    arrow(s, Inches(3.5), Inches(3.25), Inches(5.6), Inches(3.42))
    arrow(s, Inches(9.8), Inches(3.25), Inches(7.7), Inches(3.42))
    arrow(s, Inches(5.6), Inches(4.24), Inches(3.5), Inches(4.5))
    arrow(s, Inches(7.7), Inches(4.24), Inches(9.8), Inches(4.5))

    note(s, "两面**并行而不是先后**：模型面不读 `pe`、不等 `data_ready`，只要文件身份解析完就能开工。",
         top=Inches(6.38))
    foot(s, "来源：`CONTEXT.md` §模型面与数据面 / §严格初始化阶段；ADR-053 / ADR-056 / ADR-057")
    return s


def slide_glossary(prs):
    s = new_slide(prs)
    head(s, "五个必须先对齐的名词", kicker="02 · 总览")
    rows = [
        ["名词", "是什么", "不是什么"],
        ["`dbnum` / 库",
         "一个 E3D 设计库文件。`refno` 的 `word0` 按位解出它属于哪个库，全仓只有 `refno_dbnum()` 一处解法。",
         "不是靠扫全库建的 `Ref0 → dbnum` 表（ADR 退役）。"],
        ["`sesno` / 会话号",
         "库文件内部单调递增的写入代号。一次 `SAVEWORK` 推一格，增量窗口就是一段会话区间 `S → T`。",
         "不是时间戳，不是版本号。"],
        ["生成根 (Generation Root)",
         "一次重算选定的根 `refno`——从它开始（含子树）重新生成。一个变化元素归一到恰好一个根。",
         "不是「变化的那个元素」本身。"],
        ["净窗口 (Net Window)",
         "窗口 `S → T` 内每个 `refno` 合并后的最终结果，四态：`Added / Modified / Deleted / Cancelled`。",
         "不是操作流水，不需要逐会话回放。"],
        ["两枚水位",
         "`applied_sesno` = 数据已落到哪；`model_sesno` = 模型已追平到哪。同一行并排存放。",
         "不是进度条，不是任何一条链的放行闸门。"],
    ]
    table(s, rows, ML, BODY_TOP, CONTENT_W,
          [Inches(2.25), Inches(6.4), Inches(3.12)], size=11.5, row_h=Inches(0.68))
    note(s, "「一个库一个 `dbnum` 任务」是全链路的隔离单元：同 `dbnum` 任一时刻只有一个写者，所以写库不需要 CAS。",
         top=Inches(6.04))
    foot(s, "来源：`CONTEXT.md` §RefNo 库归属 / §生成根 / §净变化 / §应用水位 / §模型水位；ADR-060 / ADR-071 / ADR-079")
    return s


def slide_pipeline(prs):
    s = new_slide(prs)
    head(s, "端到端链路：从 SAVEWORK 到模型可见", kicker="03 · 链路")

    rows = [
        ("触发", "E3D `SAVEWORK` 落盘，库文件的会话号推到 T", "文件本身", None),
        ("① 发现", "文件观察 + 冻结源 → 判定 `(dbnum, S→T)` 入队", "数据批次队列的一行", "data"),
        ("② 收集", "`IndexDiff(base@S, target@T)` 比两根元素索引", "每 refno 一条净变化（内存）", "data"),
        ("③ 依赖", "以本批 DESI 为 seed 算 CATA 依赖闭包", "`pe.dependency = true` 的行", "data"),
        ("④ 数据", "元素批分块事务 → 尾事务", "RocksDB；`applied_sesno = T`", "data"),
        ("⑤ 选根", "regen / delete / advance / lazy 四去向", "本任务的单元清单（内存）", "model"),
        ("⑥ 生成", "按根并行生成几何，每根算完紧接着算房间归属", "模型行 + 两张房间边（投影）", "model"),
        ("⑦ 刷盘", "write-behind 按根刷，等本批根的 durable ACK", "RocksDB 模型行", "model"),
        ("⑧ 收尾", "一笔事务盖水位 + 写入标记", "`model_sesno = T`、标记 `finished`", "model"),
    ]

    y = Inches(1.70)
    step = Inches(0.51)
    band_w = Inches(0.13)
    for i, (stage, what, out, half) in enumerate(rows):
        color = {"data": TEAL, "model": ACCENT, None: SLATE}[half]
        rect(s, ML, y, band_w, Inches(0.44), fill=color)
        if i < len(rows) - 1:
            rect(s, ML + Inches(0.055), y + Inches(0.44), Pt(1.4),
                 step - Inches(0.44), fill=BLUE_200)
        box = rect(s, ML + band_w, y, Inches(11.64), Inches(0.44),
                   fill=WHITE if i % 2 else PANEL, line=None)
        tf = box.text_frame
        tf.vertical_anchor = MSO_ANCHOR.MIDDLE
        tf.margin_left = Inches(0.2)
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.LEFT
        rich(p, stage, 12.5, INK, bold=True)

        caption(s, ML + Inches(1.6), y + Inches(0.11), Inches(5.6), what, size=12,
                color=INK_SOFT, align=PP_ALIGN.LEFT, mono_color=BLUE_700)
        caption(s, ML + Inches(7.4), y + Inches(0.11), Inches(4.3), "→  " + out,
                size=11.5, color=color, align=PP_ALIGN.LEFT, mono_color=color)
        y += step

    for label, top_y, color in (("数据半边", Inches(2.72), TEAL),
                                ("模型半边", Inches(4.76), ACCENT)):
        tf = textbox(s, Inches(0.02), top_y, Inches(0.7), Inches(0.3))
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.RIGHT
        rich(p, label, 10.5, color, bold=True)

    caption(s, ML, Inches(6.34), CONTENT_W,
            "①–⑧ 在**同一个 `dbnum` 任务**里一气做完：中间没有队列、没有回读库。"
            "④ 之前失败 → 水位不动、整窗幂等重放；⑥ 之后某根失败 → 只留痕不排队，"
            "数据水位照推、`model_sesno` 本轮不盖。",
            size=11.5, color=INK_SOFT, align=PP_ALIGN.LEFT)
    foot(s, "来源：ADR-056 / ADR-064 / ADR-065 / ADR-071 / ADR-076；`CONTEXT.md` §dbnum 任务")
    return s


def slide_sequence(prs):
    s = new_slide(prs)
    head(s, "时序：谁在什么时候跟谁说话", kicker="03 · 链路")

    actors = ["设计端 E3D", "文件观察 watcher", "数据批次队列", "dbnum 任务",
              "SurrealDB (RocksDB)"]
    w = Inches(2.27)
    gapx = Inches(0.095)
    xs = [ML + i * (w + gapx) for i in range(len(actors))]
    cx = [Emu(int(x) + int(half(w))) for x in xs]
    top = Inches(1.68)
    hh = Inches(0.4)
    life_bottom = Inches(6.4)

    for i, a in enumerate(actors):
        is_task = (i == 3)
        statebox(s, xs[i], top, w, hh, a,
                 fill=INK if is_task else WHITE,
                 line=None if is_task else LINE,
                 color=WHITE if is_task else INK, size=11.5, radius=0.14)
        rect(s, cx[i] - Pt(0.7), top + hh, Pt(1.4), life_bottom - top - hh,
             fill=BLUE_300 if is_task else BLUE_200)

    msgs = [
        (0, 1, "`SAVEWORK` 落盘，会话号推到 T"),
        (1, 2, "观察 + 冻结源 → 入队 `S→T`"),
        (2, 3, "出队，冻结点复核区间"),
        (3, 3, "`IndexDiff(base@S, target@T)` → 净变化"),
        (3, 4, "CATA 依赖块 + 元素批事务 ×N"),
        (3, 4, "尾事务：`applied_sesno = T`"),
        (3, 3, "选根 → 按根生成几何 + 逐根房间归属"),
        (3, 4, "模型行 + 房间边（按根 write-behind）"),
        (4, 3, "durable ACK（只等本批的根）"),
        (3, 4, "收尾一笔：`model_sesno = T` + `finished`"),
    ]
    y0 = Inches(2.44)
    ystep = Inches(0.42)

    rect(s, cx[3] - Inches(0.055), y0 + ystep * 2 - Inches(0.12),
         Inches(0.11), ystep * 7 + Inches(0.24), fill=ACCENT)

    for k, (a, b, label) in enumerate(msgs):
        y = y0 + ystep * k
        tf = textbox(s, Inches(0.14), y - Inches(0.1), Inches(0.5), Inches(0.24))
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.RIGHT
        rich(p, str(k + 1), 9.5, MUTED)
        if a == b:
            bw = Inches(4.3)
            box = rect(s, cx[a] - half(bw), y - Inches(0.14), bw, Inches(0.28),
                       fill=WHITE, line=ACCENT, radius=0.22)
            tf = box.text_frame
            tf.vertical_anchor = MSO_ANCHOR.MIDDLE
            tf.margin_left = Inches(0.06)
            tf.margin_right = Inches(0.06)
            p = tf.paragraphs[0]
            p.alignment = PP_ALIGN.CENTER
            rich(p, "⟲  " + label, 9.5, INK, mono_color=BLUE_700)
        else:
            back = b < a
            arrow(s, cx[a], y, cx[b], y,
                  color=SLATE if back else BLUE_700, width=1.4)
            span = abs(int(cx[b]) - int(cx[a]))
            pad = Inches(0.9)
            if back:  # 标签贴着来源（右侧）写，不压住活动条
                tf = textbox(s, cx[b] - pad, y - Inches(0.27),
                             Emu(span + int(pad) - int(Inches(0.08))), Inches(0.22))
                align = PP_ALIGN.RIGHT
            else:
                tf = textbox(s, cx[a] + Inches(0.08), y - Inches(0.27),
                             Emu(span + int(pad)), Inches(0.22))
                align = PP_ALIGN.LEFT
            p = tf.paragraphs[0]
            p.alignment = align
            rich(p, label, 9.5, MUTED if back else INK_SOFT,
                 mono_color=SLATE if back else BLUE_700)

    caption(s, ML, Inches(6.55), CONTENT_W,
            "第 9 步是全链唯一的「等」：任务只等**自己单元清单里的根**落到 RocksDB，"
            "同库别的根不是它的账。此外没有一处等水位、等队列、等另一条链。",
            size=11.5, color=INK_SOFT, align=PP_ALIGN.LEFT)
    foot(s, "来源：ADR-076 D2 / D5；`CONTEXT.md` §dbnum 任务 / §任务内序列化 / §冻结源")
    return s


def slide_discovery(prs):
    s = new_slide(prs)
    head(s, "阶段①：变更怎么被发现", kicker="03 · 链路")
    bullets(s, [
        "**只看文件，不接 E3D 的通知。** 设计端 `SAVEWORK` 落盘，服务端靠文件系统发现——没有插件、没有消息总线的耦合。",
        (1, "候选库文件：只按 AVEVA 命名规则（三位项目前缀 + 库号[+ 四位序号]）认，**不读文件头**。人手复制的 `… copy` / `…_old` 副本正因命名规则才被挡在外面。"),
        (1, "文件观察：路径 + 修改时刻 + 大小 + 观察时刻 + 来源。它只陈述文件层事实，库号 / 库类型 / 最新会话号都要读头，属于后一步的分析裁决。"),
        "**两条发现路，同一套判据。** watcher 的文件事件给子集，周期重扫（300 s）给全集；两者产出同一种观察，可合并成最新一条。",
        "**冻结源 (Frozen Source)**：收集开始时冻住「哪个文件、哪一代、哪个会话」——卷序列号 + 文件索引 + `dbnum` + 目标会话 + 打开长度。",
        (1, "收集与落库之间它必须还是同一个：同文件追加接受；原子替换、dbnum 漂移、会话或长度回退一律拒绝。"),
        "**范围是两道门的交集**：MDB 声明的 DESI 库（本期执行范围）∩ `watch_dbnums`（监听限定域）。三种「跳过」在日志与 `/health` 上各有各的嗓音，不混同。",
        "**新库默认不自动收**：`auto_initialize_new_dbnums` 默认 false，从未初始化的库要人确认（`/update/preview` + `/update/execute`）。",
    ], size=14)
    foot(s, "来源：`CONTEXT.md` §候选库文件 / §文件观察 / §冻结源 / §本期执行范围 / §初始化准入门")
    return s


def slide_netwindow(prs):
    s = new_slide(prs)
    head(s, "阶段②：净窗口收集——两根索引差分就是终态", kicker="03 · 链路")

    card(s, ML, BODY_TOP, Inches(5.72), Inches(2.35), "怎么算",
         ["`IndexDiff(base@S, target@T)`：打开同一文件的两个会话视图，各建一根元素索引，直接比。",
          "两端记录经 `to_named_attmap` 定形，再 `diff_ele_data` 合成。",
          "输出：每个 `refno` **恰一条** `Add / Deleted / Modified`，挂 last-touch 会话。",
          "同一 `refno` 换了 noun → 按 `Add` 写终稿，并显式删旧 noun 行。"], accent=TEAL)
    card(s, Inches(6.85), BODY_TOP, Inches(5.7), Inches(2.35), "为什么不回放会话",
         ["回放要逐会话解释每一条操作，代价随窗口长度线性增长，且要自己维护「新增后又删除」这类合并规则。",
          "差分的结果**本身就是终态**：新增后删除在两根索引里根本不出现（`Cancelled`）。",
          "成员关系也不需要事后仲裁补删——ADR-036 那套随收集器换底座一并退役，`membership_deleted` 恒 0。"], accent=ACCENT)

    tf = textbox(s, ML, Inches(4.42), CONTENT_W, Inches(0.36))
    rich(tf.paragraphs[0], "净变化四态", 15.5, INK, bold=True)
    rows = [
        ["`Added`", "`Modified`", "`Deleted`", "`Cancelled`"],
        ["窗口内新出现",
         "两端都在、内容不同",
         "窗口内消失（`pe` 上留墓碑：`deleted=true`，noun / owner / name 照基侧保留）",
         "新增后又删除 = 无净变化，不产生任何行"],
    ]
    table(s, rows, ML, Inches(4.78), CONTENT_W,
          [Inches(2.2), Inches(2.6), Inches(4.4), Inches(2.57)], size=11.5,
          row_h=Inches(0.5))
    note(s, "正确性判据是 **E3D TTY 实机往返**（TTY 宏录下的会话链 + 现场对拍），不是拿第二套解析器互查。",
         top=Inches(6.3))
    foot(s, "来源：`CONTEXT.md` §净窗口收集器 / §净变化 / §墓碑；ADR-022 / ADR-031 / ADR-036（退役）")
    return s


def slide_root(prs):
    s = new_slide(prs)
    head(s, "阶段⑤：选根——变化元素怎么归到生成根", kicker="03 · 链路")
    caption(s, ML, Inches(1.66), CONTENT_W,
            "几何不是按元素生成的，是按「一个能独立成败、独立交付的单元」生成的——"
            "改一根 `TUBI` 要重算整条 `BRAN`。所以每个变化元素先**上卷**到恰好一个生成根。",
            size=12.5, color=INK_SOFT, align=PP_ALIGN.LEFT)

    codeblock(s, ML, Inches(2.12), Inches(7.55), Inches(3.42), [
        "变化元素 refno",
        "│",
        "├─ 自身或最近祖先命中 MDU 类型？",
        "│   └─ 是 ──▶ **交付单元根**            嵌套时取最近者",
        "│",
        "└─ 否 ──▶ 从 OWNER 上溯，跨过 loop 容器",
        "     ├─ 最近的非 SITE/ZONE 属主 ──▶ **正常颗粒根**",
        "     ├─ 上溯到头只剩 SITE/ZONE  ──▶ **元素自身作根**",
        "     └─ 元素自身即 SITE/ZONE/WORL ─▶ **不生成**",
        "",
        "OWNER 变了（搬迁）──▶ **原根 + 新根，两端都重生成**",
    ], title="上卷规则", size=11.5, accent=ACCENT)

    card(s, Inches(8.62), Inches(2.12), Inches(3.93), Inches(1.32),
         "MDU 类型集合",
         ["默认 `BRAN / HANG / SUPPO / EQUI`；`delivery_unit_types` 取代、`append_…` 追加。",
          "`WORL / SITE / ZONE` 与管件 `FTUB` **恒被拒绝**。"],
         accent=TEAL, size=11, title_size=12.5)
    card(s, Inches(8.62), Inches(3.56), Inches(3.93), Inches(1.0),
         "loop 容器",
         ["`LOOP / PLOO / VERT / PAVE` 自身不是几何生成根，上溯时跨过去。"],
         accent=TEAL, size=11, title_size=12.5)
    card(s, Inches(8.62), Inches(4.68), Inches(3.93), Inches(1.0),
         "父模型输入",
         ["`SPINE / POINSP / JLDATU / PLDATU / ENDATU` 只参与父元素几何，变化归并到父级根。"],
         accent=TEAL, size=11, title_size=12.5)

    note(s, "同一生成根在一个批次内只生成一次；根以 `refno` 为唯一身份，`dbnum` 由位解码得到、不属于身份的一部分。",
         top=Inches(5.82))
    foot(s, "来源：`CONTEXT.md` §生成根 / §最小交付单元 / §Significant Owner / §搬迁；ADR-009")
    return s


def slide_four_ways(prs):
    s = new_slide(prs)
    head(s, "同一次选根的四个去向：省算必须有判据", kicker="03 · 链路")

    src = rect(s, ML, Inches(1.92), Inches(2.25), Inches(2.32),
               fill=INK, radius=0.06)
    tf = src.text_frame
    tf.vertical_anchor = MSO_ANCHOR.MIDDLE
    tf.margin_left = Inches(0.16)
    tf.margin_right = Inches(0.16)
    p = tf.paragraphs[0]
    p.alignment = PP_ALIGN.CENTER
    p.space_after = Pt(6)
    rich(p, "窗口两端", 14, WHITE, bold=True)
    p = tf.add_paragraph()
    p.alignment = PP_ALIGN.CENTER
    rich(p, "出现过的每一个根", 14, WHITE, bold=True)
    p = tf.add_paragraph()
    p.alignment = PP_ALIGN.CENTER
    p.space_before = Pt(8)
    rich(p, "`roots_S ∪ roots_T`", 11.5, BLUE_200, mono_color=BLUE_200)

    ways = [
        ("regen　重生成", "被本窗真正触到 →  重算几何、重写模型行", ACCENT, "唯一真正花钱的去向"),
        ("delete　删除清理", "只在基侧存在 →  清模型行 / 房间边 / 空间树 / 账本", BLUE_700, "两端一比即知"),
        ("advance　凭证前移", "`plan_update(S→T)` 判定未被触到 ∧ 不在账本欠账里", BLUE_500, "什么都不做"),
        ("lazy　懒生成", "从来没生成过 →  本窗不生成，等按需 `ensure`", SLATE, "不拦水位"),
    ]
    bx = ML + Inches(3.45)
    bw = Inches(8.32)
    by = Inches(1.92)
    bh = Inches(0.5)
    bstep = Inches(0.61)
    for i, (title, cond, color, tag) in enumerate(ways):
        y = by + bstep * i
        box = rect(s, bx, y, bw, bh, fill=WHITE, line=LINE, radius=0.08)
        rect(s, bx, y, Pt(3.6), bh, fill=color)
        tf = box.text_frame
        tf.vertical_anchor = MSO_ANCHOR.MIDDLE
        tf.margin_left = Inches(0.2)
        tf.margin_right = Inches(0.14)
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.LEFT
        rich(p, title + "　│　", 12.5, color, bold=True)
        rich(p, cond, 11.5, INK_SOFT, mono_color=BLUE_700)
        caption(s, bx + bw - Inches(2.0), y + Inches(0.13), Inches(1.86), tag,
                size=10, color=MUTED, align=PP_ALIGN.RIGHT)
        arrow(s, ML + Inches(2.25), Inches(3.08), bx, y + half(bh),
              color=BLUE_300, width=1.3)

    tf = textbox(s, ML, Inches(4.58), CONTENT_W, Inches(0.36))
    rich(tf.paragraphs[0], "护栏：闭包不可信时整窗退化", 15.5, INK, bold=True)
    bullets(s, [
        "`advance` 候选数超过索引键数的 **30%**，或 `plan_update` 报 `unresolved` 非空 → 整窗不信闭包，`roots_S ∩ roots_T` **全部进 regen**，回执记 `credential_advance_degraded`。",
        "`advance` 现在是一笔**账**（这一窗省掉了多少根的重生成），不再是一次写库——逐根凭证行随 `gen_root` 表退役（ADR-079）。",
    ], top=Inches(4.98), size=13)
    note(s, "四个去向互不重叠，合起来覆盖窗口两端出现过的每一个根——这是「不漏算」的结构性保证。",
         top=Inches(6.3))
    foot(s, "来源：`CONTEXT.md` §模型凭证前移 / §启动退化窗口；ADR-079")
    return s


def slide_impact(prs):
    s = new_slide(prs)
    head(s, "改了属性，要不要重算几何：三态判定", kicker="04 · 算什么")
    caption(s, ML, Inches(1.62), Inches(8.3),
            "单一权威 `classify_operation_impact`：预览与执行问**同一个谓词**。"
            "「少算」的两档各要**两票**，判不准一律落回 `Regen`。",
            size=12, color=INK_SOFT, align=PP_ALIGN.LEFT)

    def decision(left, top, width, height, text):
        box = rect(s, left, top, width, height, fill=WHITE, line=ACCENT, radius=0.1)
        tf = box.text_frame
        tf.vertical_anchor = MSO_ANCHOR.MIDDLE
        tf.margin_left = Inches(0.12)
        tf.margin_right = Inches(0.12)
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.CENTER
        p.line_spacing = 1.08
        rich(p, text, 11.5, INK, mono_color=BLUE_700)
        return box

    def yn(x, y, text, ok=True):
        caption(s, x, y, Inches(0.42), text, size=9.5,
                color=BLUE_700 if ok else SLATE)

    trunk = Inches(2.62)
    statebox(s, ML, trunk, Inches(1.85), Inches(0.5), "一次属性写入", fill=INK,
             line=None, color=WHITE, size=11.5, radius=0.1)
    d1 = decision(ML + Inches(2.15), trunk, Inches(2.55), Inches(0.5),
                  "字典 `DCHC == 0` ？")
    d2 = decision(ML + Inches(5.15), Inches(1.98), Inches(3.0), Inches(0.5),
                  "生成读取集合含它 ？")
    d3 = decision(ML + Inches(5.15), Inches(3.3), Inches(3.0), Inches(0.62),
                  "效果表判为位姿类 ∧ `DCHC == 3` ？")

    terms = {
        "skip": (Inches(1.7), "`Skip`　纯业务元数据，不处理", SLATE),
        "regen": (Inches(2.66), "`Regen`　重生成几何", ACCENT),
        "xform": (Inches(3.66), "`TransformOnly`　只改位姿，网格不变", BLUE_300),
    }
    tx, tw = ML + Inches(8.65), Inches(3.12)
    for key, (ty, label, color) in terms.items():
        statebox(s, tx, ty, tw, Inches(0.5), label, fill=color, line=None,
                 color=WHITE, size=11, radius=0.1)

    arrow(s, ML + Inches(1.85), trunk + Inches(0.25), ML + Inches(2.15),
          trunk + Inches(0.25), color=BLUE_700)
    arrow(s, ML + Inches(4.7), trunk + Inches(0.14), ML + Inches(5.15),
          Inches(2.23), color=BLUE_700)
    yn(ML + Inches(4.74), Inches(2.3), "是")
    arrow(s, ML + Inches(4.7), trunk + Inches(0.38), ML + Inches(5.15),
          Inches(3.61), color=SLATE)
    yn(ML + Inches(4.74), Inches(3.24), "否", ok=False)

    arrow(s, ML + Inches(8.15), Inches(2.23), tx, Inches(1.95), color=SLATE)
    yn(ML + Inches(8.2), Inches(1.72), "否", ok=False)
    arrow(s, ML + Inches(8.15), Inches(2.23), tx, Inches(2.91), color=BLUE_700)
    yn(ML + Inches(8.2), Inches(2.66), "是")
    arrow(s, ML + Inches(8.15), Inches(3.61), tx, Inches(3.91), color=BLUE_700)
    yn(ML + Inches(8.2), Inches(3.76), "是")
    arrow(s, ML + Inches(8.15), Inches(3.61), tx, Inches(2.91), color=SLATE)
    yn(ML + Inches(8.2), Inches(3.16), "否", ok=False)

    tf = textbox(s, ML, Inches(4.42), CONTENT_W, Inches(0.3))
    rich(tf.paragraphs[0], "和 core.dll 交互路的对照", 14, INK, bold=True)
    for i, (text, color) in enumerate((
        ("`Skip` ≈ DCHC 类别 0 —— `VDESPT` 就不进 `EVALAT`，什么都不动", SLATE),
        ("`TransformOnly` ≈ DCHC 类别 3（`POS` / `ORI` / `BFORI`）—— `css/STRTRA`·`css/SEGTRA` 只改图形段矩阵", BLUE_300),
        ("`Regen` ≈ DCHC 类别 4 / 1 / 2 —— `add/REPELE` 整件重建、`css/SEGREP` 单图元重建", ACCENT),
    )):
        yy = Inches(4.8) + i * Inches(0.4)
        box = rect(s, ML, yy, CONTENT_W, Inches(0.34), fill=PANEL, line=None,
                   radius=0.1)
        rect(s, ML, yy, Pt(3.6), Inches(0.34), fill=color)
        tf = box.text_frame
        tf.vertical_anchor = MSO_ANCHOR.MIDDLE
        tf.margin_left = Inches(0.2)
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.LEFT
        rich(p, text, 11, INK_SOFT, mono_color=color)

    note(s, "参照系：core.dll 在 **GETWORK 路**（跨会话收变更，与我们的场景同构）**根本不分属性**"
            "——任何记录重写都整件重画。三档细分是我们在交互路语义上做的省算。",
         top=Inches(6.1))
    foot(s, "来源：`CONTEXT.md` §模型影响；`docs/2026-09-15_core-attribute-change-classes-and-model-update-paths.md`")
    return s


def slide_rigid(prs):
    s = new_slide(prs)
    head(s, "只动了位置：刚体变换重发布，不重算网格", kicker="04 · 算什么 · ADR-066")

    y = BODY_TOP
    card(s, ML, y, Inches(5.72), Inches(1.58), "识别：仅位置窗口 (Placement-only Window)",
         ["整窗的变更全部落在位姿类属性上（`POS` / `ORI` / `BFORI` 一族）；",
          "没有任何一条触及尺寸、目录引用、成员结构。"], accent=TEAL)
    card(s, Inches(6.85), y, Inches(5.7), Inches(1.58), "处置：按刚体运动重发布",
         ["对已有网格施加一个刚体变换（旋转 + 平移），",
          "重写实例姿态与包围盒，**一个三角形都不重新生成**。"], accent=GREEN)
    arrow(s, Inches(6.5), y + Inches(0.79), Inches(6.85), y + Inches(0.79))

    tf = textbox(s, ML, Inches(3.62), CONTENT_W, Inches(0.36))
    rich(tf.paragraphs[0], "为什么这条省算是安全的", 15.5, INK, bold=True)
    bullets(s, [
        "几何生成天然分成两段：**单位几何**（去掉实例姿态后可共享的规范内容，如目录截面）与**实例变换**（把它放到世界里的位姿与缩放）。",
        "位姿只进入后一段。单位网格身份（内容寻址键）不含位姿，所以搬一个设备不会让它的网格换身份。",
        "受影响的派生物照样要更新：世界包围盒、空间树条目、房间归属都按新位姿重算——省掉的只是网格生成本身。",
    ], top=Inches(4.02), size=13.5)
    note(s, "配套的三个词：单位几何 / 实例变换 / 单位网格身份。Core3D 没有持久化的「单位网格身份」这一层，是我们相对几何内核的唯一差别。",
         top=Inches(6.22))
    foot(s, "来源：ADR-066；`CONTEXT.md` §单位几何 / §实例变换 / §单位网格身份")
    return s


def slide_cascade(prs):
    s = new_slide(prs)
    head(s, "联动更新：A 变了，A 以外的模型也得跟着算", kicker="04 · 算什么")
    lx, lw = ML, Inches(4.5)
    rx, rw = ML + Inches(5.1), Inches(6.67)
    caption(s, lx, Inches(1.72), lw, "变更的是什么", size=12, color=MUTED,
            align=PP_ALIGN.LEFT)
    caption(s, rx, Inches(1.72), rw, "谁必须跟着重算", size=12, color=MUTED,
            align=PP_ALIGN.LEFT)

    edges = [
        ("容器位姿变了", "整棵子树的根 —— world transform 下推", TEAL),
        ("元素被删除", "它的全部子孙 —— `DeleteCleanup`", TEAL),
        ("`OWNER` 改挂（搬迁）", "**原根 + 新根，两端都重生成**", TEAL),
        ("父模型输入变了（`SPINE` 一族）", "父级生成根", TEAL),
        ("CATA 目录件变了", "引用它的 DESI 的根 —— 按依赖闭包找", ACCENT),
        ("面板 / 房间节点变了", "该库全部根的房间归属 —— 任务内 `RoomOnly`", BLUE_300),
    ]
    y = Inches(2.08)
    stepy = Inches(0.48)
    hgt = Inches(0.42)
    for src, dst, color in edges:
        box = rect(s, lx, y, lw, hgt, fill=WHITE, line=LINE, radius=0.08)
        rect(s, lx, y, Pt(3.6), hgt, fill=color)
        tf = box.text_frame
        tf.vertical_anchor = MSO_ANCHOR.MIDDLE
        tf.margin_left = Inches(0.18)
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.LEFT
        rich(p, src, 12, INK, mono_color=color)

        arrow(s, lx + lw, y + half(hgt), rx, y + half(hgt), color=color, width=1.5)

        box = rect(s, rx, y, rw, hgt, fill=PANEL, line=None, radius=0.08)
        tf = box.text_frame
        tf.vertical_anchor = MSO_ANCHOR.MIDDLE
        tf.margin_left = Inches(0.18)
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.LEFT
        rich(p, dst, 12, INK_SOFT, mono_color=color)
        y += stepy

    for top_y, text, color in (
        (Inches(5.0),
         "几何真变了的**每一个**根　→　空间树 / AABB 条目随模型块提交**就地收敛**并置脏；"
         "面板改名这类不改几何的变化不走它。", BLUE_300),
        (Inches(5.46),
         "两条引用边**默认关**：结构连接 `CREF` ↔ `JOIS`/`JOIE`、"
         "电缆端点 `STAREF`/`ENDREF` 反查 —— 已接线，按开关放行。", ACCENT),
    ):
        box = rect(s, ML, top_y, CONTENT_W, Inches(0.4), fill=WHITE, line=LINE,
                   radius=0.08)
        rect(s, ML, top_y, Pt(3.6), Inches(0.4), fill=color)
        tf = box.text_frame
        tf.vertical_anchor = MSO_ANCHOR.MIDDLE
        tf.margin_left = Inches(0.2)
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.LEFT
        rich(p, text, 11.5, INK_SOFT, mono_color=color)

    note(s, "**明确不做**：UDA 依赖递归 ✕　`XRPNTR` 正向依赖不复制 ✕（ADR-061）　"
            "`graphicsBehaviour` 中间祖先 ✕。全仓十一类联动边的唯一索引是 "
            "`docs/2026-09-04_model-cascade-update-inventory.md` §1 那张表——改任何一条先改它。",
         top=Inches(6.04), accent=MUTED)
    foot(s, "来源：`CONTEXT.md` §联动更新；`docs/2026-09-04_model-cascade-update-inventory.md`；ADR-061")
    return s


def slide_cata(prs):
    s = new_slide(prs)
    head(s, "元件库 (CATA)：按依赖入库，不做整库基线", kicker="04 · 算什么 · ADR-064")
    bullets(s, [
        "**问题**：一个设计元素画成什么形状，取决于它引用的目录构件（`SPRE` / `CATR` → `SCOM`/`SPCO` → 几何集 → 用设计参数展开）。目录库整库入持久层太贵，不入又会让设计行「缺腿」。",
        "**做法**：每一批 DESI 写回（基线 / 稳态窗口 / 模型根发布）都以本批 DESI 为 seed 算**依赖闭包**，把闭包里的 CATA 元素与 DESI 行一起写进 `pe` / `<NOUN>` / `ATT_UDA` / `pe_owner`。",
        (1, "seed 只取目录链引用属性：`SPRE CATR PRTREF PTRE GMRE GSTR PSTR HSTU LSTU HSRO LSRO`，与 e3d-model 的 `reference_reads_guard` 同源。"),
        (1, "闭包里的行标 `pe.dependency = true`——它标的是「按依赖进来的」，**不代表该 CATA 库整库在场**。"),
        (1, "在数据窗口里它排在 DESI 块与水位**之前**：闭包解析出错拦水位；「库不在 MDB / 元素不在」只 warning。"),
        "**目录自己变了怎么办**：`cata_dependency_state:{dbnum}` 记「这个目录库的依赖行按哪一版目录文件渲的」。目录文件前移时差分出变动的目录件，与已入库依赖行求交——改的 `Replace` 重渲、删的落墓碑，然后推进它。",
        (1, "差分不可用（会话不在链上 / 候选超 20,000）→ 退化成该库全部依赖行重渲，回执点名 `cata_dependency_full_refresh`。"),
        "**两级省算，都无运行时开关**：依赖闭包缓存（进程级，一个目录元素一个进程只解析一次，ams8000 冷 456 ms → 热 57 ms）+ 已落库记忆（同库同 sesno 已落的元素不渲不发）。",
    ], size=13.5)
    note(s, "纪律：`cata_dependency_state` **不是水位**——不进 `dbnum_watermark`、不参与任何放行判据。RocksDB 仍是唯一事实，缓存只省重复的 open + parse。",
         top=Inches(6.3))
    foot(s, "来源：`CONTEXT.md` §依赖入库 / §依赖状态 / §依赖闭包缓存 / §已落库记忆；ADR-064 / ADR-058")
    return s


def slide_write(prs):
    s = new_slide(prs)
    head(s, "阶段④：数据写回——直写 RocksDB，一条路", kicker="05 · 怎么落库 · ADR-056")

    steps = [
        ("CATA 依赖块", "闭包先于 DESI 入库"),
        ("属性行分块写", "按 `tx_limits` 切事务"),
        ("缓存失效", "`invalidate_caches`"),
        ("反向索引维护", "失败入补偿队列"),
        ("窗口语句批", ""),
        ("尾事务", "**最后**推进 `applied_sesno`"),
    ]
    x = ML
    w = Inches(1.86)
    for i, (title, sub) in enumerate(steps):
        color = GREEN if i == len(steps) - 1 else TEAL
        box = rect(s, x, BODY_TOP, w, Inches(0.95), fill=WHITE, line=LINE, radius=0.08)
        rect(s, x, BODY_TOP, w, Pt(3.2), fill=color)
        tf = box.text_frame
        tf.margin_left = Inches(0.08)
        tf.margin_right = Inches(0.07)
        tf.margin_top = Inches(0.17)
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.CENTER
        p.space_after = Pt(4)
        rich(p, title, 11.5, INK, bold=True)
        if sub:
            p = tf.add_paragraph()
            p.alignment = PP_ALIGN.CENTER
            p.line_spacing = 1.05
            rich(p, sub, 9.5, INK_SOFT)
        if i < len(steps) - 1:
            arrow(s, x + w, BODY_TOP + Inches(0.47), x + w + Inches(0.1),
                  BODY_TOP + Inches(0.47), color=RGBColor(0xAC, 0xB8, 0xC2), width=1.2)
        x += w + Inches(0.1)

    card(s, ML, Inches(3.0), Inches(5.72), Inches(2.3), "元素批 (Element Batch)：唯一写法",
         ["一块 = 前置块 → 按 id 批量**删边**（id 区间）→ `ATT_UDA` → noun 行 → 每表一条 `INSERT IGNORE`。",
          "**`pe` 行从不删**：它是图顶点，删它会级联删掉 `pe_owner` / `ref_rev` / `tubi_relate` 这些别的块、别的窗口、模型面挂在它身上的边。",
          "改为 `INSERT INTO pe … ON DUPLICATE KEY UPDATE`，**原地整行替换**。",
          "两档：`Fresh`（只插、已有行赢）与 `Replace`（增量 / 依赖行前移）。"], accent=ACCENT, size=11.5)
    card(s, Inches(6.85), Inches(3.0), Inches(5.7), Inches(2.3), "事务与失败语义",
         ["切块上限 `tx_limits`：2 MiB / 5000 行 / 1000 元素，三键可调。",
          "任一块失败 → **水位不动**，整窗按同一区间**幂等重放**。",
          "非水位读者可见秒级中间态；以水位为准的读者把「半个写回」视为不存在。",
          "不经任何暂存介质——kv-mem 暂存层（ADR-017）2026-09 已整体退役。"], accent=TEAL, size=11.5)

    note(s, "对库的**读**只允许水位 / sesno / 写入状态这类控制面信息，加启动水化一次、字典一次；其余一切操作是写或删。「先删后插」的删那一半也是谓词删，不查全集。",
         top=Inches(5.6))
    foot(s, "来源：`CONTEXT.md` §数据窗口直写 / §元素批 / §库读白名单；ADR-056 / ADR-064 / ADR-072 D5")
    return s


def slide_watermark(prs):
    s = new_slide(prs)
    head(s, "两枚水位：谁承诺什么", kicker="05 · 怎么落库 · ADR-060 / ADR-079")
    rows = [
        ["", "`applied_sesno`  应用水位", "`model_sesno`  模型水位"],
        ["承诺", "这个库的**数据**已整体落库到这一版。**不**承诺模型或房间派生已完成。",
         "这个库的**模型**已追平到这一版**数据窗口**。`room_membership=true` 时承诺包含房间归属。"],
        ["谁写、什么时候写", "稳态窗口的**尾事务**，与数据原子收口。",
         "**任务收尾那一笔**，条件是本批全部根生成并序列化成功、且 `T <= applied_sesno`。"],
        ["怎么判", "`> 0` 时必须有数据支撑（`pe` 里至少一行，或持有相等的空基线凭据）——否则是**幽灵水位**，路由到重建基线。",
         "**永不超过应用水位**。按需 `ensure`、启动退化窗口生成的根都不推进它。"],
        ["它是什么", "数据面的真值。", "**模型面唯一的持久事实**——库里不再有逐根凭证行（`gen_root` 表已退役）。"],
    ]
    table(s, rows, ML, BODY_TOP, CONTENT_W,
          [Inches(1.85), Inches(4.96), Inches(4.96)], size=11, row_h=Inches(0.84))
    note(s, "**它是仪表，不是闸门**：没有任何代码路径可以等它、轮询它、或以它为条件放行另一条链。落后是常态，不是告警。",
         top=Inches(6.1), accent=RED)
    foot(s, "来源：`CONTEXT.md` §应用水位 / §模型水位 / §数据支撑 / §幽灵水位；ADR-021 / ADR-060 M7 / ADR-079")
    return s


def slide_task(prs):
    s = new_slide(prs)
    head(s, "阶段⑤⑥：一个 dbnum 的一次增量 = 一个任务", kicker="05 · 怎么落库 · ADR-065 / ADR-071")
    bullets(s, [
        "**任务是隔离单元**：同一个 `dbnum` 的库行在任一时刻只有它的任务一个写者——所以写库**不需要 CAS、不需要 ACK 收口、不需要发布队列**。",
        "**任务内一气做完**：数据窗口 → 模型重算（每根算完紧接着算房间归属）→ 等本批根刷进 RocksDB → 盖模型水位。中间没有队列、没有回读库。",
        "**任务内序列化**：任务在内存里算出数据行、模型行、房间边与两枚水位，结束前**同步**写进库。库里的这份是内存结果的落盘副本，**不是计算输入、不是调度中枢**。",
        (1, "`room_membership=true` 时每根的 `room_relate` / `room_panel_relate` 与模型行**同块同事务**；合块可跨根，但一根不拆。"),
        "**内存镜像**：每个 `dbnum` 一份两枚水位 + 库级目录落款的工作副本，启动反序列化一次，运行期一切比对都读它，不逐次查库。真值仍在库里。",
        "**调度**：稳态任务**跨库并行、同库串行**；初始化意图的任务按入队先后串行。",
        "**ModelOnly 任务**：只有模型半边的同族任务（起点取 `model_sesno`，`0` = 整库重排），排进同一条队列、同一个 worker 执行。启动追平与 `model/rebuild` 都走它。",
    ], size=14)
    note(s, "任务结束时可能是 `Partial`：属性照落、写入标记照置 `finished`、但 `model_sesno` 不盖——下一次该库任务或启动核对按水位缺口排 `ModelOnly` 补。",
         top=Inches(6.3))
    foot(s, "来源：`CONTEXT.md` §dbnum 任务 / §任务内序列化 / §内存镜像 / §ModelOnly 任务；ADR-065 / ADR-071 / ADR-072")
    return s


def nest(slide, left, top, width, height, num, title, color, fill=WHITE,
         tsize=12):
    box = rect(slide, left, top, width, height, fill=fill, line=color, radius=0.05)
    tf = textbox(slide, left + Inches(0.16), top + Inches(0.09),
                 width - Inches(0.3), Inches(0.28))
    p = tf.paragraphs[0]
    p.alignment = PP_ALIGN.LEFT
    r = p.add_run()
    r.text = num + "  "
    r.font.size = Pt(tsize + 1)
    r.font.bold = True
    r.font.color.rgb = color
    r.font.name = CJK
    rich(p, title, tsize, INK, bold=True, mono_color=color)
    return box


def slide_concurrency(prs):
    s = new_slide(prs)
    head(s, "并发模型：四层并行，一层套一层", kicker="05 · 怎么落库")

    nest(s, ML, Inches(1.8), Inches(7.85), Inches(3.92), "①",
         "跨库并行　—　稳态任务按 `dbnum` 并行，**同库串行**", ACCENT, fill=PAPER)
    chip_w = Inches(2.35)
    for i, name in enumerate(("dbnum 1101", "dbnum 1112", "dbnum 7997")):
        statebox(s, ML + Inches(0.22) + i * (chip_w + Inches(0.12)), Inches(2.2),
                 chip_w, Inches(0.32), name, fill=PANEL, line=BLUE_200,
                 color=INK_SOFT, size=10.5, radius=0.3)

    nest(s, ML + Inches(0.22), Inches(2.66), Inches(7.41), Inches(2.9), "②",
         "写入 lane　—　不同 lane 并发、同 lane 串行", TEAL, fill=WHITE)
    for i, (label, detail) in enumerate((
            ("属性 lane", "单写者 → RocksDB，有界队列上界 100 笔"),
            ("模型 lane", "单写者 → RocksDB，深度 2 片"))):
        yy = Inches(3.06) + i * Inches(0.44)
        box = rect(s, ML + Inches(0.42), yy, Inches(7.01), Inches(0.36),
                   fill=PANEL, line=None, radius=0.1)
        tf = box.text_frame
        tf.vertical_anchor = MSO_ANCHOR.MIDDLE
        tf.margin_left = Inches(0.16)
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.LEFT
        rich(p, label + "　→　", 11, TEAL, bold=True)
        rich(p, detail, 10.5, INK_SOFT)

    nest(s, ML + Inches(0.42), Inches(4.0), Inches(7.01), Inches(1.4), "③",
         "按根并行　—　并行单位是**生成根**，切片只管内存不管并发", ACCENT,
         fill=PAPER, tsize=11.5)
    nest(s, ML + Inches(0.62), Inches(4.44), Inches(6.61), Inches(0.82), "④",
         "几何 CPU 闸　—　额度 = 物理核数", INK, fill=WHITE, tsize=11.5)
    caption(s, ML + Inches(0.78), Inches(4.86), Inches(6.3),
            "唯一入口 `run_gated_cpu` 只收**同步闭包**：「攥着许可去 await」在类型层就写不出来。",
            size=10, color=INK_SOFT, align=PP_ALIGN.LEFT)

    caption(s, ML, Inches(5.82), Inches(7.85),
            "四层各管各的，不能互相替代：同库串行 · 同 lane 串行 · 根之间并行 · 几何计算过闸。"
            "按**片**并行是被否掉的形态——整片要等最慢的根，长尾吃掉加速比。",
            size=11, color=INK_SOFT, align=PP_ALIGN.LEFT)

    card(s, Inches(9.0), Inches(1.8), Inches(3.55), Inches(2.22), "生成窗口",
         ["`model_regen_execution_group`：worker 最多跑在装配端前面多少根。",
          "同时答两件事——**无栏并行**（不在 cohort 边界上等最慢的根）与**内存有界**（在飞的几何最多一个窗口）。",
          "窗口满时 worker **把许可还回去再退出**，绝不攥着许可等。"],
         accent=ACCENT, size=11, title_size=13)
    card(s, Inches(9.0), Inches(4.18), Inches(3.55), Inches(2.1),
         "初始化双 lane（ADR-077）",
         ["解析（e3d-io 流式 → 元素批）与生成（并行生产者 → 切片 → 模型行 + 房间边）**并发**跑，各走一条 lane 直写。",
          "两 lane 排空、属性面计数核对后，收尾**一笔**盖两枚水位 + `finished`。"],
         accent=TEAL, size=11, title_size=13)

    foot(s, "来源：`CONTEXT.md` §写入 lane / §切片流水线 / §生成窗口 / §几何 CPU 执行域 / §初始化双 lane；ADR-041 / ADR-052 / ADR-077")
    return s


def slide_writebehind(prs):
    s = new_slide(prs)
    head(s, "阶段⑦：增量任务在哪一刻才算结束", kicker="05 · 怎么落库 · ADR-076")
    bullets(s, [
        "**结论：RocksDB 拿到它，任务才结束。** 持久档下任务进 `durable_pending` 阶段，等**自己单元清单里的根**拿到 durable ACK（`model_write_behind::await_roots_durable`）——同库别的根不是它的账。",
        "**为什么不是「内存投影里有了就算完」**：投影活在进程里，重启即空。以投影为准会让一次崩溃把「已经盖过章的模型水位」变成空头承诺。",
        "**write-behind 只刷根，不盖水位**：刷盘是按根去重 / 退避 / 死信的一台独立机器；模型水位那一笔排在本批根的 ACK **之后**，由任务收尾写。",
        "**刷盘预算用尽 / 判死的根**：放弃、逐出投影、生成根账本记 `failed` → 任务 `Partial`、写入标记照收、`model_sesno` 不盖 → 下一次该库任务或启动核对按水位缺口排 `ModelOnly`。",
        "**新鲜度从模型投影来**：「这个根现在是不是最新的」由模型投影 + 生成根账本回答；`model_sesno` 只回答「这个库整体追平到哪」。",
    ], size=14.5)
    card(s, ML, Inches(4.9), Inches(5.72), Inches(1.4), "根发布确认 (Root Publication ACK)",
         ["源会话、几何条数、房间开关、resident generation、介质全部匹配才写。",
          "**不落控制面、不推进水位、进程退出即丢**——它不是第三枚水位。"], accent=TEAL, size=11.5)
    card(s, Inches(6.85), Inches(4.9), Inches(5.7), Inches(1.4), "生成根账本 (Model Root Ledger)",
         ["进程内逐根成败：`Generated | Failed | Refused` + 它照的 `source_end_sesno`。",
          "**不是第二份真值**：没有一条恢复路读它，重启后欠什么由两枚水位回答。"], accent=ACCENT, size=11.5)
    foot(s, "来源：ADR-076 D2 / D5 / D6 / D8、ADR-079；`CONTEXT.md` §任务内序列化 / §生成根账本 / §完整根发布确认")
    return s


def slide_room(prs):
    s = new_slide(prs)
    head(s, "房间归属：从独立阶段退回到「跟着根一起算」", kicker="05 · 怎么落库 · ADR-073")

    codeblock(s, ML, Inches(1.74), Inches(7.72), Inches(3.5), [
        "本窗变更集",
        "│",
        "├─ 普通构件变了",
        "│   └─▶ 该根重生成 ──▶ **bake 那一刻**（网格还在手上）",
        "│         └─▶ 对候选面板做**点包含判定**",
        "│               └─▶ 两张边与模型行 **同一笔事务** 落库",
        "│",
        "└─ **面板自身**变了  PANE / FRMW / SBFR / CWALL / CFLOOR",
        "    └─▶ 任务内部排一条 **RoomOnly**（不重生成几何）",
        "          ├─ 能定向：旧盒 ∪ 新盒 查空间树 → 折成本库的根",
        "          │            └─▶ 只重算命中的那些根",
        "          └─ 退化：树不就绪 / 折不出根 ─▶ **整库全部根重算**",
    ], title="房间边是怎么被写出来的", size=11, accent=ACCENT)

    card(s, Inches(8.78), Inches(1.74), Inches(3.77), Inches(1.62),
         "判定点为什么在 bake",
         ["取的是**就要写盘的那份网格**经 `world_points_of`——与 flush 回读 `.mesh` 是同一个函数、数值一个不差。",
          "算不出 = 该件几何 `Err` → 这一根成死信、不发布。"],
         accent=TEAL, size=10.5, title_size=12.5)
    card(s, Inches(8.78), Inches(3.48), Inches(3.77), Inches(1.76),
         "RoomOnly 的三条触发",
         ["① 本窗变更集碰到房间定义族 noun（只按 noun 判）",
          "② 开工前对齐换了房间代号",
          "③ 库上「整库房间对齐代号」欠着——**电平型**，重启后仍记得，兜住前两条丢掉的情况"],
         accent=ACCENT, size=10.5, title_size=12.5)

    bullets(s, [
        "**没有房间水位。** 库上只有两枚水位；对齐状态由 `model_room_generation` 表达，它没有推进规则、不参与任何放行判决。",
        "**硬验收不变**（沿用 ADR-010 §9）：一组增量收敛后的两张边，与同一份数据从零初始化的结果**逐边相等**。",
        "**降级不判死**：缺文件的 DESI、缺几何的面板算「房间输入缺口」→ 生命周期 `Degraded`，根照常算、边照常写；指向缺口面板的存量边**本轮不删不写**（陈旧可恢复，抹平不可）。",
    ], top=Inches(5.42), size=12)
    foot(s, "来源：`CONTEXT.md` §逐根房间归属 / §整库房间重算 / §房间输入缺口 / §整库房间对齐代号；ADR-073 D1 / D13 / D14")
    return s


def slide_spatial(prs):
    s = new_slide(prs)
    head(s, "空间树：房间与邻近查询的共同底座", kicker="05 · 怎么落库")
    bullets(s, [
        "**是什么**：全库元素世界包围盒的进程级 R* 树（`GLOBAL_AABB_TREE`）。房间候选查询、整间分支的成员盒、邻近查询都从它取。",
        "**怎么维护**：带脏标记。模型块提交后按内存投影的世界盒**就地收敛**并置位，删除清理置位；空闲轮收尾原子写回单文件快照 `accel_tree_{project}.snapshot`。",
        "**怎么判快照能不能用**：快照头记各库的 `(applied_sesno, model_sesno)` 数对，启动时与水位镜像**逐库相等**才复用；不等即从 `inst_relate.aabb` 指针重建（只读路径，不重算几何、不回写库）。",
        "**`refno` 反查**：读写路径共用树内部那张 `refno_index`。禁止在调用方另建一份 refno → 包围盒映射。",
        (1, "理由不是性能：那会造出第二份「树上现在有什么」的真值——漂移的后果是**把已经不在那儿的构件算进某间房**，而这类错误没有任何东西会报出来。"),
        "**空间版本号 (Spatial Epoch)**：库侧单调递增的空间提交计数，每笔改变「树该有什么」的已提交事务同事务 +1。只比相等，不表达次数。",
    ], size=14)
    card(s, ML, Inches(5.3), CONTENT_W, Inches(1.05), "顺带支撑的对外能力",
         ["**中心线走廊**：一条 BRAN 的真实中心线（按成员序连成的线段表，含隐式管身），而不是它含子树的包围盒。",
          "**最近净距**：以一条 BRAN 为源，在给定半径内按目标分桶（墙 / 柱，或按 NOUN），每桶取最近的若干构件，每条候选带一对可直接画尺寸的端点。"],
         accent=TEAL, size=11.5)
    foot(s, "来源：`CONTEXT.md` §空间树 / §refno 反查 / §空间版本号 / §中心线走廊 / §最近净距；ADR-045 / ADR-072 D4")
    return s


def slide_failure(prs):
    s = new_slide(prs)
    head(s, "坏掉了怎么办：一种恢复形态，没有第二种", kicker="06 · 失败与恢复 · ADR-073 D3")

    # 写入标记的状态机
    sw, sh = Inches(1.95), Inches(0.55)
    sy = Inches(1.88)
    s1x, s2x, s3x = ML, ML + Inches(3.0), ML + Inches(6.0)
    statebox(s, s1x, sy, sw, sh, "无写入标记", fill=WHITE, line=LINE)
    statebox(s, s2x, sy, sw, sh, "`writing{S,T}`", fill=ACCENT, line=None, color=WHITE)
    statebox(s, s3x, sy, sw, sh, "`finished`", fill=WHITE, line=LINE)
    arrow(s, s1x + sw, sy + half(sh), s2x, sy + half(sh), color=BLUE_700, width=1.4)
    arrow(s, s2x + sw, sy + half(sh), s3x, sy + half(sh), color=BLUE_700, width=1.4)
    caption(s, s1x + sw, sy - Inches(0.26), Inches(1.05), "第一笔写库前", size=9.5)
    caption(s, s2x + sw, sy - Inches(0.26), Inches(1.05), "收尾同事务", size=9.5)

    s4x, s4y = ML + Inches(1.9), Inches(2.92)
    arrow(s, s4x + Inches(1.6), sy + sh, s4x + Inches(1.6), s4y, color=INK, width=1.6)
    statebox(s, s4x, s4y, Inches(3.2), Inches(0.5),
             "整库重初始化", fill=INK, line=None, color=WHITE)
    caption(s, s4x + Inches(1.78), Inches(2.55), Inches(4.0),
            "崩溃 / 断电 → 下次启动或开工前仍见 `writing`",
            size=10, color=INK_SOFT, align=PP_ALIGN.LEFT, mono_color=ACCENT)
    caption(s, ML, Inches(3.5), Inches(7.6),
            "`wipe_dbnum_for_reinit`：清数据 / 模型 / 房间边，两枚水位归零、spatial epoch 递增；"
            "再按首次导入重新解析当前文件、整库重生成模型。清库只在**冻结点**做。",
            size=11, color=INK_SOFT, align=PP_ALIGN.LEFT)

    card(s, ML + Inches(8.02), Inches(1.88), Inches(3.75), Inches(1.94),
         "被明确否决的那些做法",
         ["✕ 按 `(dbnum, sesno > S)` 谓词删痕迹",
          "✕ 回退到水位、排 S→T 重放",
          "✕ 部分回滚 / 分面回退 / 还原到快照",
          "✕ 把正常收尾的 `Partial` 当成没写完"],
         accent=MUTED, size=11.5)

    tf = textbox(s, ML, Inches(4.06), Inches(4.0), Inches(0.36))
    rich(tf.paragraphs[0], "三档失败，语义各不相同", 15.5, INK, bold=True)
    tf = textbox(s, Inches(5.2), Inches(4.14), Inches(7.35), Inches(0.36))
    p = tf.paragraphs[0]
    p.alignment = PP_ALIGN.RIGHT
    rich(p, "核心取舍：**模型失败不阻断数据窗口**（ADR-056 D1）——代价是「模型落后」成为常态。",
         12, INK_SOFT, mono_color=ACCENT)
    rows = [
        ["情形", "水位", "处置"],
        ["**窗口阻断**（收集器报错 / 写回被确定性拒绝）",
         "`applied_sesno` 停止推进，告警常驻",
         "修复后按**同一区间**幂等重放"],
        ["**模型根失败**（某根算不出 / 刷盘判死）",
         "数据水位照推；`model_sesno` 本轮不盖",
         "只留痕，**不排队、不自动重试**；下次按水位缺口排 `ModelOnly`"],
        ["**未完成写入**（标记停在 `writing`）",
         "两枚水位归零",
         "整库重初始化——唯一形态"],
    ]
    table(s, rows, ML, Inches(4.46), CONTENT_W,
          [Inches(4.0), Inches(3.6), Inches(4.17)], size=11, row_h=Inches(0.5))
    foot(s, "来源：`CONTEXT.md` §写入标记 / §未完成写入重初始化 / §窗口阻断 / §重建批次；ADR-056 D1 / ADR-072 / ADR-073 D3")
    return s


def slide_observability(prs):
    s = new_slide(prs)
    head(s, "对外只讲一个口径：库一致性判决", kicker="06 · 可观测")
    cards = [
        ("一致", GREEN, ["`applied_sesno > 0`", "**且** `model_sesno == applied_sesno`", "**且** 没有「还在赶」的根"]),
        ("落后", ACCENT, ["前述任一不成立", "附四格计数"]),
        ("不判", MUTED, ["没有可判等的水位对", "（读透形态数据面不摄入；或数据水位尚未建立）",
                        "**中性陈述**，不是落后、不是告警"]),
    ]
    x = ML
    for title, color, lines in cards:
        card(s, x, BODY_TOP, Inches(3.86), Inches(1.5), title, lines, accent=color, size=11.5)
        x += Inches(3.96)

    tf = textbox(s, ML, Inches(3.42), CONTENT_W, Inches(0.36))
    rich(tf.paragraphs[0], "生成根的四格（互不重叠）", 15.5, INK, bold=True)
    rows = [
        ["已追平", "还在赶", "已放弃", "没生成过"],
        ["这一版已经算过",
         "**拦水位**",
         "`http_422` 一类永久判定：**不拦水位，但必须并排陈述**",
         "懒生成，**不拦水位**"],
    ]
    table(s, rows, ML, Inches(3.82), CONTENT_W,
          [Inches(2.4), Inches(2.4), Inches(4.4), Inches(2.57)], size=11.5, row_h=Inches(0.56))

    bullets(s, [
        "**为什么四格要分开**：已放弃的根若算进「还在赶」，一个坏根就让整库的灯永红，且看不出被谁卡住。",
        "**谁可以自己拼口径：没有人。** 面板、健康页、批次回执、启动日志都只消费这一个结论，不得自己拿水位列或队列计数重新算一个。",
        "**房间必须并读**：`room_membership=false` 时「一致」只指数据与模型几何，读者必须同时展示这个开关状态。",
    ], top=Inches(5.05), size=13.5)
    foot(s, "来源：`CONTEXT.md` §库一致性判决 / §模型水位；ADR-060 D4 / D5")
    return s


def slide_evolution(prs):
    s = new_slide(prs)
    head(s, "这套架构是怎么走到今天的", kicker="07 · 演进")
    milestones = [
        ("ADR-017", "kv-mem 暂存窗口", "先写暂存库、提交时搬进正库——买的是原子提交与崩溃恢复", MUTED),
        ("ADR-056", "退暂存，直写 RocksDB", "稳态窗口唯一写回路径；模型失败与数据水位**解耦**", ACCENT),
        ("ADR-064", "CATA 按依赖入库", "元素批 delete-then-insert；取代「按需解析元件库」", TEAL),
        ("ADR-065/071", "模型并进 dbnum 任务", "退 `model_update_pending` 队列；库 = 任务结果的同步序列化", TEAL),
        ("ADR-072/073", "退发布阶段，统一恢复", "写标记 + 「任何未完成写入 → 整库重初始化」；房间退回逐根", GREEN),
        ("ADR-077/079", "双 lane 初始化 + 退 `gen_root`", "解析与生成并发；模型水位成为模型面**唯一**的持久事实", GREEN),
        ("ADR-076", "任务边界落在 durable ACK", "RocksDB 拿到它，任务才结束；新鲜度从模型投影来", GREEN),
    ]
    y = Inches(1.8)
    rect(s, ML + Inches(1.24), y + Inches(0.1), Pt(2), Inches(4.5), fill=LINE)
    for adr, title, sub, color in milestones:
        dot(s, ML + Inches(1.18), y + Inches(0.14), color)
        tf = textbox(s, ML - Inches(0.1), y + Inches(0.03), Inches(1.16), Inches(0.3))
        p = tf.paragraphs[0]
        p.alignment = PP_ALIGN.RIGHT
        rich(p, adr, 10.5, color, bold=True)
        tf = textbox(s, ML + Inches(1.5), y - Inches(0.02), Inches(3.1), Inches(0.3))
        rich(tf.paragraphs[0], title, 14, INK, bold=True)
        tf = textbox(s, ML + Inches(4.62), y, Inches(7.1), Inches(0.3))
        rich(tf.paragraphs[0], sub, 12, INK_SOFT)
        y += Inches(0.66)

    note(s, "一条贯穿的取向：**减少真值的份数**。每退掉一张表、一条队列、一枚水位，就少一处会漂移、且漂移了不报警的地方。",
         top=Inches(6.34))
    foot(s, "来源：`docs/adr/` ADR-017 → ADR-080")
    return s


def slide_closing(prs):
    s = new_slide(prs, dark=True)
    rect(s, Inches(0), Inches(0), Inches(0.26), SLIDE_H, fill=ACCENT_ON_DARK)
    tf = textbox(s, Inches(1.1), Inches(0.85), Inches(11.0), Inches(0.5))
    rich(tf.paragraphs[0], "小结：五句话", 30, WHITE, bold=True)
    rect(s, Inches(1.12), Inches(1.52), Inches(0.9), Pt(3.2), fill=ACCENT_ON_DARK)

    points = [
        "**一个 `dbnum` 的一次增量就是一个任务**——数据、模型、房间、两枚水位在同一个任务里一气做完，它同时是这个库唯一的写者。",
        "**差分而不是回放**——两根索引比出来的净变化就是终态，每个 `refno` 恰一条。",
        "**省算都要判据**——`Skip` 与 `TransformOnly` 各要两票，`advance` 有 30% 护栏，判不准就多算。",
        "**两枚水位是仪表不是闸门**——没有任何链路等它；模型落后于数据是常态。",
        "**只有一种恢复**——见到写入标记停在 `writing`，整库重初始化；没有部分回滚。",
    ]
    y = Inches(2.05)
    for i, p_text in enumerate(points, 1):
        tf = textbox(s, Inches(1.1), y, Inches(0.5), Inches(0.4))
        rich(tf.paragraphs[0], f"0{i}", 16, ACCENT_ON_DARK, bold=True)
        tf = textbox(s, Inches(1.75), y - Inches(0.02), Inches(10.4), Inches(0.6))
        p = tf.paragraphs[0]
        p.line_spacing = 1.22
        rich(p, p_text, 14.5, RGBColor(0xDB, 0xE4, 0xEB),
             mono_color=ACCENT_ON_DARK)
        y += Inches(0.78)

    tf = textbox(s, Inches(1.1), Inches(6.35), Inches(11.0), Inches(0.5))
    p = tf.paragraphs[0]
    p.line_spacing = 1.3
    rich(p, "术语真值 `CONTEXT.md`    ·    决策真值 `docs/adr/`    ·    "
            "联动边索引 `docs/2026-09-04_model-cascade-update-inventory.md` §1",
         11.5, RGBColor(0x8B, 0xA1, 0xB2), mono_color=RGBColor(0x8B, 0xA1, 0xB2))
    return s


def slide_adr_index(prs):
    s = new_slide(prs)
    head(s, "引用的关键决策记录", kicker="附录")
    rows = [
        ["ADR", "标题（要点）"],
        ["ADR-009", "`OWNER` 变化是「搬迁」而不是「属性改」——新旧两根都要重生成"],
        ["ADR-022 / 031", "净窗口靠会话索引差分收集；单一收集口径"],
        ["ADR-056", "增量直写 RocksDB，退 kv-mem 暂存；模型失败不阻断数据水位"],
        ["ADR-060 / 079", "模型水位是按 dbnum 记下的承诺；它是模型面唯一的持久事实，`gen_root` 表退役"],
        ["ADR-064", "元素行批量 delete-then-insert；CATA 按依赖入库"],
        ["ADR-065 / 071", "模型工作跑在 dbnum 任务里，pending 队列退役；库是任务结果的同步序列化"],
        ["ADR-066", "仅位置变更的窗口按刚体运动重发布，不重新生成网格"],
        ["ADR-072 / 073", "发布阶段退役、写入标记；房间按根算，任何未完成写入 → 整库重初始化"],
        ["ADR-076", "增量任务在 RocksDB 拿到它时结束，新鲜度从模型投影来"],
        ["ADR-077", "dbnum 初始化经两条并发写道持久化属性与模型，收尾一笔事务收口"],
    ]
    table(s, rows, ML, BODY_TOP, CONTENT_W,
          [Inches(2.3), Inches(9.47)], size=11.5, row_h=Inches(0.43))
    foot(s, "完整列表见 `docs/adr/`（ADR-001 … ADR-080）")
    return s


# ---------------------------------------------------------------- 组装


def build(out_path: Path) -> Path:
    prs = Presentation()
    prs.slide_width = SLIDE_W
    prs.slide_height = SLIDE_H

    slide_cover(prs)
    slide_agenda(prs)
    slide_why(prs)
    slide_two_faces(prs)
    slide_glossary(prs)
    slide_pipeline(prs)
    slide_sequence(prs)
    slide_discovery(prs)
    slide_netwindow(prs)
    slide_root(prs)
    slide_four_ways(prs)
    slide_impact(prs)
    slide_rigid(prs)
    slide_cascade(prs)
    slide_cata(prs)
    slide_write(prs)
    slide_watermark(prs)
    slide_task(prs)
    slide_concurrency(prs)
    slide_writebehind(prs)
    slide_room(prs)
    slide_spatial(prs)
    slide_failure(prs)
    slide_observability(prs)
    slide_evolution(prs)
    slide_adr_index(prs)
    slide_closing(prs)

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
    default = repo / "docs" / "slides" / "增量模型生成-技术架构.pptx"
    out = Path(args[0]).resolve() if args else default
    saved = build(out)
    print(f"brand : {BRAND_BLUE}  →  "
          + " ".join(f"#{c}" for c in (BLUE_900, BLUE_700, BLUE_500, BLUE_300, SLATE)))
    print(f"slides: {len(Presentation(str(saved)).slides._sldIdLst)}")
    print(f"saved : {saved}")


if __name__ == "__main__":
    main()
