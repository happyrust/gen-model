from typing import Any

def apply_file(
    path: str, start: int | None = None, end: int | None = None
) -> dict[str, Any]:
    """对单个库文件执行一个增量窗口（默认 = 水位+1..=文件最新）。"""

def execute_manual(
    project: str | None = None,
    mdb: str | None = None,
    dbnums: list[int] | None = None,
) -> dict[str, Any]:
    """扫描 + 入队 + 当场消费到空（等价一次手动更新执行）。"""

def drain_data() -> int:
    """消化 durable pending 的前两个数据阶段，返回消化条数。"""

def queue_pause() -> bool:
    """暂停队列出队（持久化）。"""

def queue_resume() -> bool:
    """解除队列的持久化暂停。"""
