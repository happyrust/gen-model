# -*- coding: utf-8 -*-
"""HTTP 薄客户端的离线用例：对着打桩服务跑，不需要真的 gen-model 服务。

打桩模式与仓内 `tests/python/test_gen_model_testing.py` 同款（ThreadingHTTPServer
录请求）。这一档钉的是**路由与报文形状**：方法、路径、查询串、请求体——
`docs/specs/web-service-api.md` 改了而客户端没跟上时在这里红，而不是等到对着
真服务调试时才发现。
"""

from __future__ import annotations

import json
import threading
import warnings
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from aios_client import (
    EXPECTED_SERVER_VERSION,
    AiosApiError,
    AiosClient,
    AiosVersionWarning,
)

pytestmark = pytest.mark.offline


class _StubHandler(BaseHTTPRequestHandler):
    requests: list[dict] = []
    version: str = EXPECTED_SERVER_VERSION

    def log_message(self, *_args):  # 不往 pytest 输出里灌访问日志
        pass

    def _handle(self):
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length) if length else b""
        body = json.loads(raw) if raw else None
        type(self).requests.append(
            {"method": self.command, "path": self.path, "body": body}
        )
        if isinstance(body, dict) and body.get("refno") == "boom":
            status, payload = 409, {"code": "conflict", "message": "夹具冲突"}
        elif self.path.startswith("/api/v1/health"):
            status, payload = 200, {
                "status": "ok",
                "project": "AvevaMarineSample",
                "version": type(self).version,
            }
        else:
            status, payload = 200, {"ok": True}
        encoded = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    do_GET = _handle
    do_POST = _handle


@pytest.fixture()
def stub():
    """起一个随机端口的打桩服务，yield (base_url, handler 类)。"""
    _StubHandler.requests = []
    _StubHandler.version = EXPECTED_SERVER_VERSION
    server = ThreadingHTTPServer(("127.0.0.1", 0), _StubHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}", _StubHandler
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def test_rest_methods_hit_documented_routes(stub):
    base, handler = stub
    client = AiosClient(base, timeout=10)

    client.health()
    client.update_preview("AMS")
    client.update_execute("AMS", [7997, 7998])
    client.tasks(state="running", limit=5)
    client.task("task-1")
    client.model_ensure("24381_100677", force=True)
    client.pending_units()
    client.pending_unit_retry("regen", "24381_100677")
    client.dbnums("AMS")
    client.queue()
    client.queue_pause()
    client.queue_resume()

    assert [(r["method"], r["path"]) for r in handler.requests] == [
        ("GET", "/api/v1/health"),
        ("POST", "/api/v1/update/preview"),
        ("POST", "/api/v1/update/execute"),
        ("GET", "/api/v1/tasks?state=running&limit=5"),
        ("GET", "/api/v1/tasks/task-1"),
        ("POST", "/api/v1/model/ensure"),
        ("GET", "/api/v1/update/pending-units"),
        ("POST", "/api/v1/update/pending-units/retry"),
        ("GET", "/api/v1/dbnums?project=AMS"),
        ("GET", "/api/v1/queue"),
        ("POST", "/api/v1/queue/pause"),
        ("POST", "/api/v1/queue/resume"),
    ]
    bodies = {r["path"]: r["body"] for r in handler.requests}
    assert bodies["/api/v1/update/preview"] == {"project": "AMS"}
    assert bodies["/api/v1/update/execute"] == {
        "project": "AMS",
        "dbnums": [7997, 7998],
    }
    assert bodies["/api/v1/model/ensure"] == {
        "refno": "24381_100677",
        "force": True,
    }
    assert bodies["/api/v1/update/pending-units/retry"] == {
        "action": "regen",
        "target_refno": "24381_100677",
    }


def test_optional_arguments_are_dropped_not_sent_as_null(stub):
    """None 的查询参数要整个不出现——服务端把 `project=` 当空字符串会走错分支。"""
    base, handler = stub
    client = AiosClient(base, timeout=10)
    client.dbnums()
    client.tasks()
    client.update_preview()
    assert [r["path"] for r in handler.requests] == [
        "/api/v1/dbnums",
        "/api/v1/tasks",
        "/api/v1/update/preview",
    ]
    assert handler.requests[-1]["body"] == {}


def test_non_2xx_raises_api_error_with_payload(stub):
    base, _handler = stub
    client = AiosClient(base, timeout=10)
    with pytest.raises(AiosApiError) as caught:
        client.model_ensure("boom")
    assert caught.value.status == 409
    assert caught.value.payload["code"] == "conflict"
    assert "409" in str(caught.value)


def test_version_mismatch_warns_once(stub):
    base, handler = stub
    handler.version = "0.1.13"
    client = AiosClient(base, timeout=10)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        client.health()
        client.health()
    drift = [w for w in caught if issubclass(w.category, AiosVersionWarning)]
    assert len(drift) == 1, "同一个 client 反复 health 不该刷屏"
    assert "0.1.13" in str(drift[0].message)
    assert EXPECTED_SERVER_VERSION in str(drift[0].message)


def test_matching_version_is_silent(stub):
    base, _handler = stub
    client = AiosClient(base, timeout=10)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        client.health()
    assert [w for w in caught if issubclass(w.category, AiosVersionWarning)] == []


def test_version_check_can_be_disabled(stub):
    base, handler = stub
    handler.version = "9.9.9"
    client = AiosClient(base, timeout=10, expected_version=None)
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        client.health()
    assert [w for w in caught if issubclass(w.category, AiosVersionWarning)] == []
