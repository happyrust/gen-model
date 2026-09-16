from typing import Any

def header(path: str) -> dict[str, Any]:
    """库文件头：db_type / dbnum / latest_sesno / latest_ses_time / file_size。"""

def is_db_file(path: str) -> bool:
    """是不是候选 AVEVA 库文件（名字形态 + 文件头两道门）。"""

def collapse_extract_files(
    entries: list[tuple[str, int, str]],
) -> dict[str, Any]:
    """同项目抽取家族归并（ADR-028）。entries 为 (project, header_dbnum, path)。

    返回 {selected: [{project, dbnum, leaf_path, parent_path}],
    shadowed_parents, duplicate_keys: [[project, dbnum], ...],
    mismatches: [{path, filename_dbnum, header_dbnum}]}。"""

def parent_gap_refno_count(leaf: str, parent: str) -> int:
    """父层索引独有、叶子没有的 refno 个数（基线只在 gap>0 时补缺）。"""

def sessions(path: str) -> list[dict[str, Any]]:
    """全部会话页（升序）：sesno / pgno / end_pgno / index_root_pageno / user / comments / date。

    user 是 core.dll 的会话属性 USER（写这一会话的登录名），不是机器名——rs-core 时代的误称
    computer_name 曾作别名保留，已于 0.1.26 删除（0.1.25 的 changelog 预告过）。"""

def net_window(
    path: str, start: int, end: int, detail: bool = False
) -> dict[str, Any]:
    """唯一的窗口口径：解析器语义净窗口（纯文件，e3d-io 底座）。与增量管线 / 手动预览同实现。

    过滤换页但内容相同的原样重写。返回 {requested_start/end,
    window: {sesno: [op, ...]}, counts, warnings, unchanged_rewrites,
    unparseable_finals}；detail=True 时 Modified 携带属性旧值/新值。
    E3D 保存期元数据（如 CACHID）仍会如实返回。

    old-pdms-io 底座的 collect_changes / net_changes / attmap / subtree /
    generate_* / noun_dict 2026-09-04 已随 old-pdms-io 一起删除；判据是 E3D TTY 录下的会话。"""

def element(path: str, refno: str, sesno: int | None = None) -> dict[str, Any]:
    """从库文件直读单元素在某会话时点的持久属性（e3d-io 底座，与生产收集器同一读法）。

    sesno 缺省读文件最新会话；给定时把文件钉在该会话（历史版本 / TTY 往返前后读数）。
    元素在该会话索引里不存在时抛 RuntimeError。返回 {refno, sesno, noun_hash, noun, name,
    owner, children, attrs, explicit_attrs}；attrs / explicit_attrs 为 NamedAttrMap 的 serde 形态。"""
