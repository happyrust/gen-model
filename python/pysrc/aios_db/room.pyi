from typing import Any

def build_all() -> None:
    """房间归属全量重建。"""

def drain() -> dict[str, Any]:
    """消化待重算的房间归属目标，返回 DrainReport 的 JSON 形态。"""

def enqueue(changes: list[dict[str, Any]]) -> int:
    """按 [{refno, noun}, ...]（model.update_aabbs 的返回形态）入队房间重算，
    PANE 走整间分支、其它走元素分支；返回入队条数。不受 room_incremental 开关影响。"""

def code(refno: str) -> str | None:
    """元素的房间编码（fn::room_code 直通，连接层只读；无归属返回 None）。"""

def nodes(refno: str) -> list[str]:
    """元素（BRAN 等）穿过的房间 PANE refno 列表（fn::get_room_nodes）。"""

def names(refno: str) -> list[Any]:
    """元素穿过的房间号列表（fn::get_room_names）。"""
