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

def resolve_window(path: str, skip_cata: bool = False) -> dict[str, Any]:
    """只读预览下一增量窗口（不执行、不动水位；连接层即可用）。"""

def drain_side_effects() -> int:
    """消化 SystDerived / RefRevMaintain 副作用（不含空间收敛），返回完成数。"""

def queue_pause() -> bool:
    """暂停队列出队（持久化）。"""

def queue_resume() -> bool:
    """解除队列的持久化暂停。"""

def queue_status() -> dict[str, Any]:
    """队列状态快照（连接层只读）：{paused, rows}；rows 是本进程的调度器队列。"""
