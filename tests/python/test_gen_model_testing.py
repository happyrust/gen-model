from __future__ import annotations

import json
import os
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "python"))

from gen_model_testing import (  # noqa: E402
    ApiError,
    GenModelClient,
    ProjectIdentity,
    RustToolError,
    RustTools,
    ToolResult,
    normalize_macro,
)


class RecordingHandler(BaseHTTPRequestHandler):
    requests: list[dict[str, object]] = []

    def log_message(self, *_args: object) -> None:
        pass

    def _handle(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length)
        body = json.loads(raw) if raw else None
        type(self).requests.append({"method": self.command, "path": self.path, "body": body})
        if self.path.startswith("/api/v1/fail"):
            self.send_response(409)
            payload = {"error": "fixture conflict"}
        elif self.path.startswith("/api/v1/dbnums") and self.command == "GET":
            self.send_response(200)
            payload = {"rows": [{"dbnum": 8000, "applied_sesno": 26, "blocked": False}]}
        elif self.path.startswith("/api/v1/tasks/"):
            self.send_response(200)
            payload = {"id": self.path.rsplit("/", 1)[1], "state": "succeeded"}
        else:
            self.send_response(200)
            payload = {"ok": True}
        encoded = json.dumps(payload).encode()
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    do_GET = _handle
    do_POST = _handle
    do_DELETE = _handle


class ClientTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        RecordingHandler.requests.clear()
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), RecordingHandler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.client = GenModelClient(
            f"http://127.0.0.1:{cls.server.server_port}",
            ProjectIdentity("AMS", "/ALL", "ams"),
        )

    @classmethod
    def tearDownClass(cls) -> None:
        cls.server.shutdown()
        cls.thread.join()
        cls.server.server_close()

    def test_execute_sends_identity_and_dbnums(self) -> None:
        self.client.execute([8000])
        request = RecordingHandler.requests[-1]
        self.assertEqual(request["method"], "POST")
        self.assertEqual(request["path"], "/api/v1/update/execute")
        self.assertEqual(
            request["body"],
            {"project": "AMS", "mdb": "/ALL", "namespace": "ams", "dbnums": [8000]},
        )

    def test_routes_cover_query_queue_prune_and_retry(self) -> None:
        self.client.query("owner", {"refno": "1/2"})
        self.client.pause_queue()
        self.client.resume_queue()
        self.client.prune_above_preview(8000, 24)
        self.client.prune_above(8000, 24)
        self.client.retry_pending_unit("1/2", "DeleteCleanup")
        paths = [entry["path"] for entry in RecordingHandler.requests[-6:]]
        self.assertEqual(paths[0], "/api/v1/query")
        self.assertIn("/api/v1/dbnums/8000/data/above/24?", paths[3])
        self.assertIn("/api/v1/dbnums/8000/data/above/24?", paths[4])
        self.assertIn("confirm=8000%3A24", paths[4])
        self.assertEqual(paths[5], "/api/v1/update/pending-units/retry")

    def test_fast_delete_confirmation_is_a_query_parameter(self) -> None:
        self.client.fast_delete_dbnum(8000)
        request = RecordingHandler.requests[-1]
        self.assertEqual(request["method"], "DELETE")
        self.assertIn("/api/v1/dbnums/8000/data?", request["path"])
        self.assertIn("confirm=8000", request["path"])
        self.assertIsNone(request["body"])

    def test_dbnum_and_task_wait_helpers(self) -> None:
        row = self.client.wait_for_watermark(8000, 26, timeout=1)
        self.assertEqual(row["applied_sesno"], 26)
        task = self.client.wait_for_task("task with space", timeout=1)
        self.assertEqual(task["state"], "succeeded")
        self.assertIn("task%20with%20space", RecordingHandler.requests[-1]["path"])

    def test_http_errors_are_structured(self) -> None:
        with self.assertRaises(ApiError) as caught:
            self.client._request("GET", "/api/v1/fail")
        self.assertEqual(caught.exception.status, 409)
        self.assertEqual(caught.exception.body, {"error": "fixture conflict"})


class MacroTests(unittest.TestCase):
    def test_normalize_redirects_log_and_removes_terminal_commands(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            log = Path(temp) / "evidence.log"
            actual = normalize_macro(
                'ALPHA LOG "old.log" OVER\n/TEST\nFINISH\nquit\n', alpha_log=log
            )
        self.assertIn(log.resolve().as_posix(), actual)
        self.assertIn("/TEST", actual)
        self.assertNotIn("FINISH", actual.upper())
        self.assertNotIn("QUIT", actual.upper())

    def test_runner_uses_log_name_expected_by_l3_suite(self) -> None:
        class FakeTools:
            def run_l3_driver(self, macro: Path, **_kwargs: object) -> ToolResult:
                self.macro = macro
                return ToolResult(("l3_suite",), 0, "ok", "", 0.0)

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source.mac"
            source.write_text('ALPHA LOG "old.log" OVER\nQ CE\nFINISH\n', encoding="utf-8")
            tools = FakeTools()
            from gen_model_testing import E3dTtyRunner

            E3dTtyRunner(tools, root, root, root).run(source, label="apply")
            generated = tools.macro.read_text(encoding="utf-8")
        self.assertIn((root / "apply.log").resolve().as_posix(), generated)
        self.assertNotIn("apply.alpha.log", generated)


class RustToolsTests(unittest.TestCase):
    def test_l3_driver_exports_launcher_path_not_boolean_flag(self) -> None:
        class CapturingTools(RustTools):
            def run(self, name: str, args: object, **kwargs: object) -> ToolResult:
                self.captured = (name, args, kwargs)
                return ToolResult((name,), 0, "ok", "", 0.0)

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            tools = CapturingTools(root, bin_dir=root)
            tools.run_l3_driver(root / "case.mac", project_dir=root, e3d_install=root)
            env = tools.captured[2]["env"]
        self.assertTrue(env["L3_E3D_DRIVER"].endswith("run_ams_c_entrymacro.bat"))
        self.assertNotEqual(env["L3_E3D_DRIVER"], "1")

    def test_run_captures_output_and_exit_status(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            if os.name == "nt":
                executable = root / "echo_tool.exe"
                executable.write_bytes(Path(sys.executable).read_bytes())
                result = RustTools(root, bin_dir=root).run(
                    "echo_tool", ["-c", "print('seen:value', end='')"]
                )
            else:
                executable = root / "echo_tool"
                executable.write_text("#!/bin/sh\nprintf 'seen:%s' \"$1\"\n", encoding="utf-8")
                executable.chmod(0o755)
                result = RustTools(root, bin_dir=root).run("echo_tool", ["value"])
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "seen:value")

    def test_missing_executable_reports_checked_locations(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            with self.assertRaises(FileNotFoundError) as caught:
                RustTools(Path(temp), bin_dir=Path(temp)).executable("missing")
        self.assertIn("missing", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
