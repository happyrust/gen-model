from typing import Any

def ensure(refno: str, force: bool = False) -> dict[str, Any]:
    """按需生成单个构件的模型（幂等；force 才重生成）。"""

def gen(refnos: list[str]) -> None:
    """对指定 refno 集重建深层网格数据。"""

def gen_dbnum(dbnum: int) -> None:
    """整库模型生成。"""

def update_aabbs(refnos: list[str], replace: bool = False) -> list[dict[str, Any]]:
    """刷新 inst_relate aabb，返回真变化的元素列表。"""

def export_obj(refno: str, dir: str) -> dict[str, Any]:
    """把元素（含子树）的已生成网格导出为世界坐标 OBJ（连接层即可用）。"""
