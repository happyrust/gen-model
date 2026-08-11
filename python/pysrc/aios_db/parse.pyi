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

def element(path: str, refno: str, sesno: int | None = None) -> dict[str, Any]:
    """从文件直读单元素属性 dump；sesno 给定时读该会话或之前的最后版本。"""

def noun_dict(attlib_path: str) -> dict[str, Any]:
    """attlib 字典：{noun_count, field_count, nouns: [NounCapabilities...]}。"""
