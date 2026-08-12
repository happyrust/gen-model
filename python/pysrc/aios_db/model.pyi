from typing import Any

def ensure(refno: str, force: bool = False) -> dict[str, Any]:
    """按需生成单个构件的模型（幂等；force 才重生成）。"""

def gen(refnos: list[str]) -> None:
    """对指定 refno 集重建深层网格数据。"""

def gen_dbnum(dbnum: int) -> None:
    """整库模型生成。"""

def update_aabbs(
    refnos: list[str], replace: bool = False, durable: bool = False
) -> list[dict[str, Any]]:
    """刷新 inst_relate aabb，返回真变化的元素列表 [{refno, noun}, ...]。

    durable=True 走定向增量入口：直写时 AABB 指针、room_recalc 任务与
    spatial epoch 同事务（房间任务的发布受 room_incremental 开关门控）。
    返回形态可直接喂给 room.enqueue。
    """

def delete_subtree(refnos: list[str], chunk_size: int = 100) -> None:
    """删除元素含 pe 子树的全部模型数据（级联删几何、清房间边、摘空间树；幂等）。"""

def export_obj(refno: str, dir: str) -> dict[str, Any]:
    """把元素（含子树）的已生成网格导出为世界坐标 OBJ（连接层即可用）。"""
