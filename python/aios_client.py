# -*- coding: utf-8 -*-
"""gen-model Web 服务（REST + WebSocket）的薄客户端，按 docs/specs/web-service-api.md 1:1 封装。

与 pyo3 绑定（aios_db）的分工：**服务在跑 → 用本客户端观察**（任务注册表 / 队列
快照 / 实时进度只活在服务进程内存里）；**服务停了 → 用 aios_db 深入**（直调内部
函数）。两边字段同源（同一份 serde 输出）。

REST 只用标准库；`watch_tasks()` 需要 `websocket-client`（缺了会提示安装）。

用法:
    from aios_client import AiosClient
    c = AiosClient()                     # 默认 http://127.0.0.1:8022
    print(c.health())
    print(c.update_preview())
    for ev in c.watch_tasks():
        print(ev["type"], ev.get("task_id"))
"""

from __future__ import annotations

import json
import urllib.error
import urllib.parse
import urllib.request
import warnings
from typing import Any, Iterator

# 本文件跟着走的服务端版本（= 仓库根 Cargo.toml 的 package.version）。字段面按
# docs/specs/web-service-api.md 演进，客户端与在跑服务不同版时先打个招呼——实测
# 踩过 0.1.13 的绑定对着 0.1.16 的部署包查半天的坑。只 warning 不报错：跨版本
# 多数字段仍然通用，硬拦会把「凑合能用」变成「完全不能用」。
#
# 手抄常量，靠测试自锁：`python/tests/test_client_offline.py::
# test_expected_version_tracks_the_crate_version` 会拿它跟 Cargo.toml 对表，
# release bump 忘了同步这里，CI 的离线档立刻红。
EXPECTED_SERVER_VERSION = "0.1.18"


class AiosVersionWarning(UserWarning):
    """在跑服务的版本与 `EXPECTED_SERVER_VERSION` 不一致。"""


class AiosApiError(RuntimeError):
    """非 2xx 响应；携带服务端错误结构 { code, message, detail }。"""

    def __init__(self, status: int, payload: Any):
        self.status = status
        self.payload = payload
        code = payload.get("code") if isinstance(payload, dict) else None
        message = payload.get("message") if isinstance(payload, dict) else payload
        super().__init__(f"HTTP {status} {code}: {message}")


class AiosClient:
    def __init__(self, base: str = "http://127.0.0.1:8022", timeout: float = 130.0,
                 expected_version: str | None = EXPECTED_SERVER_VERSION):
        # timeout 默认 130s：/model/ensure 的服务端等待预算是 120s，客户端不能更短。
        self.base = base.rstrip("/")
        self.timeout = timeout
        # 传 None 关掉版本告警（明知在连老部署包时用）。
        self.expected_version = expected_version
        self._version_warned = False

    # ── 基础设施 ────────────────────────────────────────────────────────────

    def _request(self, method: str, path: str, body: dict | None = None,
                 query: dict | None = None) -> Any:
        url = f"{self.base}/api/v1{path}"
        if query:
            filtered = {k: v for k, v in query.items() if v is not None}
            if filtered:
                url += "?" + urllib.parse.urlencode(filtered)
        data = json.dumps(body).encode("utf-8") if body is not None else None
        request = urllib.request.Request(
            url, data=data, method=method,
            headers={"Content-Type": "application/json; charset=utf-8"},
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                raw = response.read()
        except urllib.error.HTTPError as error:
            raw = error.read()
            try:
                payload = json.loads(raw) if raw else {}
            except json.JSONDecodeError:
                payload = raw.decode("utf-8", "replace")
            raise AiosApiError(error.code, payload) from None
        return json.loads(raw) if raw else None

    # ── REST 端点（§4）──────────────────────────────────────────────────────

    def health(self) -> dict:
        payload = self._request("GET", "/health")
        self._check_version(payload)
        return payload

    def _check_version(self, health: Any) -> None:
        """版本漂移只告警一次（同一 client 反复 health 不刷屏）。"""
        if self._version_warned or not self.expected_version:
            return
        actual = health.get("version") if isinstance(health, dict) else None
        if actual and actual != self.expected_version:
            self._version_warned = True
            warnings.warn(
                f"{self.base} 在跑的是 {actual}，本客户端跟的是 "
                f"{EXPECTED_SERVER_VERSION}——字段面可能有出入，对不上时先核版本",
                AiosVersionWarning,
                stacklevel=3,
            )

    def update_preview(self, project: str | None = None) -> dict:
        return self._request("POST", "/update/preview",
                             {"project": project} if project else {})

    def update_execute(self, project: str | None = None,
                       dbnums: list[int] | None = None) -> dict:
        body: dict[str, Any] = {}
        if project:
            body["project"] = project
        if dbnums is not None:
            body["dbnums"] = dbnums
        return self._request("POST", "/update/execute", body)

    def tasks(self, state: str | None = None, kind: str | None = None,
              limit: int | None = None) -> dict:
        return self._request("GET", "/tasks",
                             query={"state": state, "kind": kind, "limit": limit})

    def task(self, task_id: str) -> dict:
        return self._request("GET", f"/tasks/{urllib.parse.quote(task_id)}")

    def model_ensure(self, refno: str, force: bool = False) -> dict:
        body: dict[str, Any] = {"refno": refno}
        if force:
            body["force"] = True
        return self._request("POST", "/model/ensure", body)

    def pending_units(self) -> dict:
        return self._request("GET", "/update/pending-units")

    def pending_unit_retry(self, action: str, target_refno: str) -> dict:
        return self._request("POST", "/update/pending-units/retry",
                             {"action": action, "target_refno": target_refno})

    def dbnums(self, project: str | None = None) -> dict:
        return self._request("GET", "/dbnums", query={"project": project})

    # `realign_dbnum`（POST /dbnums/{dbnum}/realign）随 ADR-021 移除：回退由服务
    # 自动入队重建批次并在 worker 复核后整库重建，无需客户端动作。

    def queue(self) -> dict:
        return self._request("GET", "/queue")

    def queue_pause(self) -> dict:
        return self._request("POST", "/queue/pause", {})

    def queue_resume(self) -> dict:
        return self._request("POST", "/queue/resume", {})

    # ── WebSocket（§5）──────────────────────────────────────────────────────

    def watch_tasks(self, topics: list[str] | None = None,
                    ping_interval: float = 30.0) -> Iterator[dict]:
        """订阅 tasks 主题事件，逐条 yield 信封 dict（type/seq/ts/task_id/payload）。

        阻塞迭代器：Ctrl+C 退出。服务端 90s 无入站消息会断开，这里按
        `ping_interval` 自动发心跳（pong 事件不往外抛）。
        """
        try:
            import websocket  # websocket-client
        except ImportError as error:
            raise RuntimeError(
                "watch_tasks 需要 websocket-client：uv pip install websocket-client"
            ) from error

        ws_url = self.base.replace("http://", "ws://").replace("https://", "wss://")
        conn = websocket.create_connection(f"{ws_url}/api/v1/ws", timeout=ping_interval)
        try:
            conn.send(json.dumps({"type": "subscribe", "topics": topics or ["tasks"]}))
            while True:
                try:
                    raw = conn.recv()
                except websocket.WebSocketTimeoutException:
                    conn.send(json.dumps({"type": "ping"}))
                    continue
                if not raw:
                    break
                event = json.loads(raw)
                if event.get("type") == "pong":
                    continue
                yield event
        finally:
            conn.close()
