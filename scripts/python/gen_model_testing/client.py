"""Standard-library client for the Rust test-facing HTTP API."""

from __future__ import annotations

import json
import time
from dataclasses import dataclass
from typing import Any, Iterable, Mapping
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode
from urllib.request import Request, urlopen


class ApiError(RuntimeError):
    def __init__(self, message: str, *, status: int | None = None, body: Any = None):
        super().__init__(message)
        self.status = status
        self.body = body


@dataclass(frozen=True)
class ProjectIdentity:
    project: str
    mdb: str
    namespace: str

    def as_dict(self) -> dict[str, str]:
        return {
            "project": self.project,
            "mdb": self.mdb,
            "namespace": self.namespace,
        }


class GenModelClient:
    """Typed Python facade; Rust remains the authoritative implementation."""

    def __init__(
        self,
        base_url: str,
        identity: ProjectIdentity,
        *,
        timeout: float = 30.0,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.identity = identity
        self.timeout = timeout

    def _request(
        self,
        method: str,
        path: str,
        *,
        body: Mapping[str, Any] | None = None,
        query: Mapping[str, Any] | None = None,
    ) -> Any:
        query_items = {k: v for k, v in (query or {}).items() if v is not None}
        url = f"{self.base_url}{path}"
        if query_items:
            url += "?" + urlencode(query_items)
        payload = None if body is None else json.dumps(body).encode("utf-8")
        request = Request(
            url,
            data=payload,
            method=method,
            headers={"Accept": "application/json", "Content-Type": "application/json"},
        )
        try:
            with urlopen(request, timeout=self.timeout) as response:
                raw = response.read()
        except HTTPError as exc:
            raw = exc.read()
            parsed = self._decode(raw)
            raise ApiError(
                f"{method} {path} returned HTTP {exc.code}",
                status=exc.code,
                body=parsed,
            ) from exc
        except URLError as exc:
            raise ApiError(f"{method} {path} failed: {exc.reason}") from exc
        return self._decode(raw)

    @staticmethod
    def _decode(raw: bytes) -> Any:
        if not raw:
            return None
        text = raw.decode("utf-8", errors="replace")
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return text

    def health(self) -> Any:
        return self._request("GET", "/api/v1/health")

    def query(self, tool: str, arguments: Mapping[str, Any]) -> Any:
        return self._request(
            "POST",
            "/api/v1/query",
            body={**self.identity.as_dict(), "tool": tool, "arguments": dict(arguments)},
        )

    def preview(self) -> Any:
        return self._request("POST", "/api/v1/update/preview", body=self.identity.as_dict())

    def execute(self, dbnums: Iterable[int] | None = None) -> Any:
        body: dict[str, Any] = self.identity.as_dict()
        if dbnums is not None:
            body["dbnums"] = list(dbnums)
        return self._request("POST", "/api/v1/update/execute", body=body)

    def pending_units(self) -> Any:
        return self._request(
            "GET", "/api/v1/update/pending-units", query=self.identity.as_dict()
        )

    def retry_pending_unit(self, target_refno: str, action: str | None = None) -> Any:
        body: dict[str, Any] = {**self.identity.as_dict(), "target_refno": target_refno}
        if action is not None:
            body["action"] = action
        return self._request("POST", "/api/v1/update/pending-units/retry", body=body)

    def tasks(self, *, state: str | None = None, kind: str | None = None, limit: int | None = None) -> Any:
        return self._request(
            "GET", "/api/v1/tasks", query={"state": state, "kind": kind, "limit": limit}
        )

    def task(self, task_id: str) -> Any:
        return self._request("GET", f"/api/v1/tasks/{quote(task_id, safe='')}")

    def ensure_model(self, refno: str, *, force: bool = False) -> Any:
        return self._request(
            "POST",
            "/api/v1/model/ensure",
            body={**self.identity.as_dict(), "refno": refno, "force": force},
        )

    def dbnums(self) -> Any:
        return self._request("GET", "/api/v1/dbnums", query=self.identity.as_dict())

    def dbnum(self, dbnum: int) -> Mapping[str, Any]:
        rows = self.dbnums()
        if isinstance(rows, dict):
            rows = rows.get("rows", rows.get("dbnums", []))
        for row in rows or []:
            if int(row.get("dbnum", -1)) == dbnum:
                return row
        raise ApiError(f"dbnum {dbnum} is absent from /api/v1/dbnums", body=rows)

    def fast_delete_dbnum(self, dbnum: int, *, confirm: int | None = None) -> Any:
        return self._request(
            "DELETE",
            f"/api/v1/dbnums/{dbnum}/data",
            query={**self.identity.as_dict(), "confirm": dbnum if confirm is None else confirm},
        )

    def prune_above_preview(self, dbnum: int, watermark: int) -> Any:
        return self._request(
            "GET",
            f"/api/v1/dbnums/{dbnum}/data/above/{watermark}",
            query=self.identity.as_dict(),
        )

    def prune_above(self, dbnum: int, watermark: int, *, confirm: str | None = None) -> Any:
        return self._request(
            "DELETE",
            f"/api/v1/dbnums/{dbnum}/data/above/{watermark}",
            query={
                **self.identity.as_dict(),
                "confirm": confirm or f"{dbnum}:{watermark}",
            },
        )

    def queue(self) -> Any:
        return self._request("GET", "/api/v1/queue")

    def pause_queue(self) -> Any:
        return self._request("POST", "/api/v1/queue/pause", body={})

    def resume_queue(self) -> Any:
        return self._request("POST", "/api/v1/queue/resume", body={})

    def wait_for_health(self, *, timeout: float = 60.0, interval: float = 0.25) -> Any:
        deadline = time.monotonic() + timeout
        last_error: Exception | None = None
        while time.monotonic() < deadline:
            try:
                return self.health()
            except ApiError as exc:
                last_error = exc
                time.sleep(interval)
        raise TimeoutError(f"service did not become healthy within {timeout}s: {last_error}")

    def wait_for_watermark(
        self,
        dbnum: int,
        expected: int,
        *,
        timeout: float = 120.0,
        interval: float = 0.5,
        fail_on_block: bool = True,
    ) -> Mapping[str, Any]:
        deadline = time.monotonic() + timeout
        last: Mapping[str, Any] | None = None
        while time.monotonic() < deadline:
            last = self.dbnum(dbnum)
            applied = int(last.get("applied_sesno", last.get("watermark", 0)))
            if applied >= expected:
                return last
            blocked = last.get("blocked") or last.get("block_reason")
            if fail_on_block and blocked:
                raise RuntimeError(f"dbnum {dbnum} is blocked before watermark {expected}: {last}")
            time.sleep(interval)
        raise TimeoutError(f"dbnum {dbnum} did not reach watermark {expected}; last={last}")

    def wait_for_task(
        self,
        task_id: str,
        *,
        timeout: float = 120.0,
        interval: float = 0.5,
        terminal_states: tuple[str, ...] = (
            "succeeded",
            "completed",
            "yielded",
            "failed",
            "cancelled",
        ),
    ) -> Any:
        deadline = time.monotonic() + timeout
        last: Any = None
        while time.monotonic() < deadline:
            last = self.task(task_id)
            state = str(last.get("state", last.get("status", ""))).lower()
            if state in terminal_states:
                return last
            time.sleep(interval)
        raise TimeoutError(f"task {task_id} did not finish; last={last}")
