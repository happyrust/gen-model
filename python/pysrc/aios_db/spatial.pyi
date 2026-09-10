from typing import Any

def tree_status() -> dict[str, Any]:
    """空间树状态：原样透出 /health spatial_tree 那份渲染（连接层只读）。

    键面以 Rust 侧渲染半边为唯一权威（形状钉也在那边），这里不复述全集。
    稳定核：entries / file_epoch / db_epoch / drift / startup_verdict。
    指纹现读现比：没有任务在飞而 drift=true，就是静默漂移。
    """

def tree_dump() -> list[dict[str, Any]]:
    """导出内存空间树全部条目（连接层只读，不做状态门）：
    [{refno, noun, mins: [x,y,z], maxs: [x,y,z]}, ...]，refno 为 a_b 形态。
    「树 == 已提交指针」的值级校验通道；量级 == 库内可用指针数。"""

def persist(force: bool = False) -> bool:
    """把内存空间树落盘。force=False 只在脏时写（返回是否真的写了）。"""

def rebuild() -> None:
    """从库内包围盒指针全量重建空间树并立即落盘（兜底）。"""
