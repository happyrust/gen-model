from typing import Any

def build_all() -> None:
    """房间归属全量重建。"""

def drain() -> dict[str, Any]:
    """消化待重算的房间归属目标，返回 DrainReport 的 JSON 形态。"""

def code(refno: str) -> str | None:
    """元素的房间编码（fn::room_code 直通，连接层只读；无归属返回 None）。"""

def nodes(refno: str) -> list[str]:
    """元素（BRAN 等）穿过的房间 PANE refno 列表（fn::get_room_nodes）。"""

def names(refno: str) -> list[Any]:
    """元素穿过的房间号列表（fn::get_room_names）。"""
