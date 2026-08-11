from typing import Any

def status() -> dict[str, Any]:
    """空间收敛积压状态（连接层只读）：{pending, retries, last_error, stalled}。"""

def reconcile() -> int:
    """消化待收敛的空间意图（树刷新/删除 + 文件持久化），返回收敛条数。

    零售组合（apply_file / drain_data / room.drain / model.gen*）收工前必须调，
    否则空间意图滞留 pending 表、内存树不落盘。
    """

def persist(force: bool = False) -> bool:
    """把内存空间树落盘。force=False 只在脏时写（返回是否真的写了）。"""

def rebuild() -> None:
    """从库内包围盒指针全量重建空间树并立即落盘（兜底）。"""
