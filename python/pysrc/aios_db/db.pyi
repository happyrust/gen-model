from typing import Any

def query(sql: str, binds: dict[str, Any] | None = None) -> list[Any]:
    """SurrealQL 直通：按语句返回干净 JSON 数组。"""

def by_name(name: str, dbnum: int | None = None) -> list[str]:
    """名字精确匹配的 refno 列表（a_b 形态）。"""

def child_of(parent: str, noun: str, dbnum: int | None = None) -> list[str]:
    """名为 parent 的元素下、类型为 noun 的子元素 refno 列表。"""

def pe(refno: str) -> dict[str, Any] | None:
    """一个元素的 pe 行（不存在返回 None）。"""

def members(refno: str) -> list[dict[str, Any]]:
    """直接成员（owner 反查，未删除）。"""

def owner_chain(refno: str) -> list[dict[str, Any]]:
    """沿 owner 一路到 WORL 的链（含自己）。"""

def inst(refno: str) -> list[dict[str, Any]]:
    """元素及其子树的 inst_relate 边（FETCH aabb / world_trans）。"""

def watermark(dbnum: int) -> int:
    """一个库的权威应用水位（未登记为 0）。"""

def dbnum_statuses(
    project: str | None = None, mdb: str | None = None
) -> dict[str, Any]:
    """水位状态 + 阻断/排除（GET /dbnums 同源）。"""

def preview_manual_update(
    project: str | None = None, mdb: str | None = None
) -> dict[str, Any]:
    """手动更新只读预览（POST /update/preview 同源）。"""

def pending_model_units() -> list[dict[str, Any]]:
    """全部模型待重试任务（含死信）。"""

def window_blocks() -> list[dict[str, Any]]:
    """全部窗口阻断状态。"""

def root_attempts(dbnum: int) -> dict[str, Any]:
    """一个库全部生成根的失败记录。"""
