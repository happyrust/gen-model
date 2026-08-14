from typing import Any

def header(path: str) -> dict[str, Any]:
    """库文件头：db_type / dbnum / latest_sesno / latest_ses_time / file_size。"""

def is_db_file(path: str) -> bool:
    """是不是候选 AVEVA 库文件（名字形态 + 文件头两道门）。"""

def sessions(path: str) -> list[dict[str, Any]]:
    """全部会话页（升序）：sesno / pgno / computer_name / comments / date。"""

def collect_changes(
    path: str, start: int, end: int, detail: bool = False
) -> dict[str, list[dict[str, Any]]]:
    """增量窗口变更收集（纯函数）：{sesno: [op, ...]}。"""

def net_changes(
    path: str, start: int, end: int, with_noun: bool = False
) -> dict[str, Any]:
    """会话索引差分（纯函数，不查库、不逐会话解析）：窗口净增删改。

    返回 {requested_start/end, base_sesno, target_sesno,
    added/deleted/modified: [{refno, record_pgno, record_offset,
    last_touch_sesno, noun}], counts, stats}。净口径：窗口内加了又删不出现，
    删了又建判 modified。与 collect_changes（逐会话回放）互为对拍。"""

def net_window(
    path: str, start: int, end: int, detail: bool = False
) -> dict[str, Any]:
    """解析器语义净窗口（纯文件）：过滤换页但内容相同的原样重写。

    返回 {requested_start/end, window: {sesno: [op, ...]}, counts,
    warnings, unchanged_rewrites, unparseable_finals}；detail=True 时 Modified
    携带属性旧值/新值。E3D 保存期元数据（如 CACHID）仍会如实返回，适合直接
    验收 TTY apply/restore 的业务改动与伴随元数据。"""

def element(path: str, refno: str, sesno: int | None = None) -> dict[str, Any]:
    """从文件直读单元素属性 dump；sesno 给定时读该会话或之前的最后版本。"""

def noun_dict(attlib_path: str) -> dict[str, Any]:
    """attlib 字典：{noun_count, field_count, nouns: [NounCapabilities...]}。"""
