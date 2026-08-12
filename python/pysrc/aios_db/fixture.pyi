from typing import Any

def create(mesh_dir: str | None = None) -> None:
    """建合成房间夹具（幂等：先 drop 再建）。与 Rust room_fixture live 测试同一套：
    1 间房 /ZZ-R-K100 + 2 块 PANE + 5 个盒形构件，保留 refno 段 4000000001。
    只对一次性测试库使用；mesh_dir 缺省取配置的 meshes 目录。"""

def drop(mesh_dir: str | None = None) -> None:
    """清夹具（库内记录 + zzfx_*.mesh 文件），幂等。"""

def move_body(
    seq: int,
    min: tuple[float, float, float] | list[float],
    max: tuple[float, float, float] | list[float],
    mesh_dir: str | None = None,
) -> None:
    """把一个夹具几何体搬到新包围盒（世界坐标）。只动几何侧，不碰
    inst_relate.aabb——之后调 model.update_aabbs 触发「包围盒真的变了」。"""

def refnos() -> dict[str, Any]:
    """夹具清单：{room_num, pane_a, pane_b, in_a, in_b, straddler, seqs}。
    refno 为 a_b 形态；seqs 是 move_body 用的几何体序号。"""
