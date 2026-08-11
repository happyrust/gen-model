from typing import Any

def baseline(dbnum: int, project: str | None = None) -> dict[str, Any]:
    """给一个从未解析过的 dbnum 补一次全量基线（幂等收口水位与生成工作）。"""
