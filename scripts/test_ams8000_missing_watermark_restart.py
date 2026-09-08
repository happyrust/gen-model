"""Live regression: recreate AMS 8000 watermarks after the table is removed.

The script targets the isolated RocksDB fixture created by Verify-RocksDb8000.ps1.
It preserves every watermark row, exercises a real service restart, and restores
the original table in ``finally`` so a failed assertion cannot strand the fixture.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import subprocess
import time
import urllib.request
from pathlib import Path
from typing import Any


REPO = Path(__file__).resolve().parents[1]
DEFAULT_RUN = REPO / "output/rocksdb-8000-20260905"


def sql(endpoint: str, namespace: str, database: str, query: str) -> list[Any]:
    request = urllib.request.Request(
        f"{endpoint}/sql",
        query.encode(),
        headers={
            "Authorization": "Basic " + base64.b64encode(b"root:root").decode(),
            "Surreal-NS": namespace,
            "Surreal-DB": database,
            "Accept": "application/json",
            "Content-Type": "text/plain",
        },
    )
    with urllib.request.urlopen(request, timeout=180) as response:
        result = json.load(response)
    for statement in result:
        if statement["status"] != "OK":
            raise RuntimeError(statement)
    return [statement["result"] for statement in result]


def api(url: str) -> Any:
    with urllib.request.urlopen(url, timeout=10) as response:
        return json.load(response)


def dbnum_row(api_base: str) -> dict[str, Any]:
    payload = api(f"{api_base}/api/v1/dbnums")
    rows = payload.get("dbnums", payload.get("rows", []))
    return next(row for row in rows if int(row["dbnum"]) == 8000)


def counts(endpoint: str, namespace: str, database: str) -> dict[str, int]:
    result = sql(
        endpoint,
        namespace,
        database,
        "SELECT count() AS count, math::max(sesno) AS max_sesno FROM pe "
        "WHERE dbnum=8000 GROUP ALL;"
        "SELECT count() AS count FROM inst_relate WHERE dbnum=8000 OR in.dbnum=8000 GROUP ALL;"
        "SELECT count() AS count FROM tubi_relate WHERE dbnum=8000 OR in.dbnum=8000 GROUP ALL;"
        "SELECT count() AS count FROM gen_root WHERE dbnum=8000 GROUP ALL;",
    )
    pe = result[0][0]
    return {
        "pe": int(pe["count"]),
        "max_sesno": int(pe["max_sesno"]),
        "inst_relate": int(result[1][0]["count"]) if result[1] else 0,
        "tubi_relate": int(result[2][0]["count"]) if result[2] else 0,
        "gen_root": int(result[3][0]["count"]) if result[3] else 0,
    }


def surreal_literal(key: str, value: Any) -> str:
    if value is None:
        return "NONE"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return str(value)
    if isinstance(value, list):
        return "[" + ", ".join(surreal_literal(key, item) for item in value) + "]"
    if isinstance(value, dict):
        return "{" + ", ".join(
            f"{json.dumps(str(k))}: {surreal_literal(str(k), item)}"
            for k, item in value.items()
        ) + "}"
    encoded = json.dumps(str(value), ensure_ascii=False)
    if key.endswith("_at") or key.endswith("_time"):
        return "d" + encoded
    return encoded


def restore_sql(rows: list[dict[str, Any]]) -> str:
    statements = ["REMOVE TABLE IF EXISTS dbnum_watermark;"]
    for row in rows:
        record_id = row["id"]
        content = {key: value for key, value in row.items() if key != "id"}
        body = ", ".join(
            f"{json.dumps(key)}: {surreal_literal(key, value)}"
            for key, value in content.items()
        )
        statements.append(f"UPSERT {record_id} CONTENT {{{body}}};")
    return "\n".join(statements) + "\n"


def process_path(pid: int) -> Path:
    command = (
        f"(Get-Process -Id {pid} -ErrorAction Stop).Path"
    )
    output = subprocess.check_output(
        ["powershell", "-NoProfile", "-Command", command],
        text=True,
        stderr=subprocess.DEVNULL,
    ).strip()
    return Path(output).resolve()


def stop_owned(pid: int, expected: Path) -> None:
    try:
        actual = process_path(pid)
    except subprocess.CalledProcessError:
        return
    if actual != expected.resolve():
        raise RuntimeError(f"PID {pid} belongs to {actual}, expected {expected}")
    subprocess.run(
        ["taskkill.exe", "/PID", str(pid), "/T", "/F"],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.STDOUT,
    )


def start_service(run: Path, executable: Path, log_prefix: str) -> subprocess.Popen[bytes]:
    env = os.environ.copy()
    env.update(
        DB_OPTION_FILE=str(run / "DbOption"),
        AIOS_STORE_MODE="rocksdb",
        AIOS_DATA_FACE="ingest",
        AIOS_STARTUP_AUTORUN="1",
        RUST_MIN_STACK="134217728",
    )
    with (run / f"{log_prefix}.stdout.log").open("wb") as stdout, (
        run / f"{log_prefix}.stderr.log"
    ).open("wb") as stderr:
        return subprocess.Popen(
            [str(executable), "serve", "--watch-dbnum", "8000"],
            cwd=run,
            env=env,
            stdout=stdout,
            stderr=stderr,
            creationflags=subprocess.CREATE_NO_WINDOW,
        )


def wait_for_reseed(
    process: subprocess.Popen[bytes], api_base: str, expected_sesno: int, timeout: int
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    last: Any = None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"service exited during reseed: {process.returncode}")
        try:
            last = dbnum_row(api_base)
            if (
                int(last.get("applied_sesno", -1)) == expected_sesno
                and int(last.get("model_sesno", -1)) == expected_sesno
                and last.get("model_verdict") == "in_sync"
            ):
                return last
        except Exception as error:
            last = repr(error)
        time.sleep(1)
    raise TimeoutError(f"watermarks did not converge in {timeout}s; last={last!r}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-dir", type=Path, default=DEFAULT_RUN)
    parser.add_argument("--db-endpoint", default="http://127.0.0.1:18049")
    parser.add_argument("--api-base", default="http://127.0.0.1:18050")
    parser.add_argument("--namespace", default="rocks8000_20260905")
    parser.add_argument("--database", default="AvevaMarineSample")
    parser.add_argument("--timeout", type=int, default=420)
    args = parser.parse_args()

    run = args.run_dir.resolve()
    state_path = run / "processes.json"
    state = json.loads(state_path.read_text(encoding="utf-8-sig"))
    executable = Path(state["service_exe"]).resolve()
    original_pid = int(state["service_pid"])
    evidence = run / "missing-watermark-restart" / time.strftime("%Y%m%d-%H%M%S")
    evidence.mkdir(parents=True)

    before_rows = sql(
        args.db_endpoint, args.namespace, args.database, "SELECT * FROM dbnum_watermark;"
    )[0]
    before_counts = counts(args.db_endpoint, args.namespace, args.database)
    before_8000 = next(row for row in before_rows if int(row["dbnum"]) == 8000)
    expected_sesno = before_counts["max_sesno"]
    if int(before_8000["applied_sesno"]) != expected_sesno:
        raise AssertionError("fixture is not initialized to pe.max(sesno)")
    if int(before_8000.get("model_sesno", -1)) != expected_sesno:
        raise AssertionError("fixture model watermark is not initialized")
    if before_counts["pe"] == 0 or before_counts["inst_relate"] == 0:
        raise AssertionError("fixture lacks AMS 8000 attributes or model projection")

    (evidence / "watermark-before.json").write_text(
        json.dumps(before_rows, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    recovery = restore_sql(before_rows)
    (evidence / "restore-watermark.surql").write_text(recovery, encoding="utf-8")

    test_process: subprocess.Popen[bytes] | None = None
    restored_process: subprocess.Popen[bytes] | None = None
    test_error: BaseException | None = None
    try:
        stop_owned(original_pid, executable)
        sql(
            args.db_endpoint,
            args.namespace,
            args.database,
            "REMOVE TABLE dbnum_watermark; INFO FOR DB;",
        )
        test_process = start_service(run, executable, "missing-watermark-test")
        state["service_pid"] = test_process.pid
        state_path.write_text(json.dumps(state, indent=2), encoding="utf-8")

        after_row = wait_for_reseed(
            test_process, args.api_base, expected_sesno, args.timeout
        )
        after_counts = counts(args.db_endpoint, args.namespace, args.database)
        if after_counts != before_counts:
            raise AssertionError(f"data/model counts changed: {before_counts} -> {after_counts}")
        result = {
            "passed": True,
            "dbnum": 8000,
            "removed": "dbnum_watermark",
            "expected_sesno": expected_sesno,
            "before": {
                "applied_sesno": before_8000["applied_sesno"],
                "model_sesno": before_8000["model_sesno"],
                "counts": before_counts,
            },
            "after_restart": {
                "applied_sesno": after_row["applied_sesno"],
                "model_sesno": after_row["model_sesno"],
                "model_verdict": after_row["model_verdict"],
                "counts": after_counts,
            },
        }
        (evidence / "result.json").write_text(
            json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8"
        )
        print(json.dumps({**result, "evidence": str(evidence)}, ensure_ascii=False))
    except BaseException as error:
        test_error = error
        raise
    finally:
        if test_process is not None:
            stop_owned(test_process.pid, executable)
        sql(args.db_endpoint, args.namespace, args.database, recovery)
        restored_process = start_service(run, executable, "service-restored")
        state["service_pid"] = restored_process.pid
        state_path.write_text(json.dumps(state, indent=2), encoding="utf-8")
        try:
            restored_8000 = wait_for_reseed(
                restored_process, args.api_base, expected_sesno, args.timeout
            )
            restored_rows = sql(
                args.db_endpoint,
                args.namespace,
                args.database,
                "SELECT * FROM dbnum_watermark;",
            )[0]
            original_ids = {row["id"] for row in before_rows}
            restored_ids = {row["id"] for row in restored_rows}
            if restored_ids != original_ids:
                raise AssertionError(
                    f"rollback watermark ids changed: {original_ids} -> {restored_ids}"
                )
            rollback = {
                "passed": True,
                "restored_rows": len(restored_rows),
                "service_pid": restored_process.pid,
                "dbnum_8000": {
                    "applied_sesno": restored_8000["applied_sesno"],
                    "model_sesno": restored_8000["model_sesno"],
                    "model_verdict": restored_8000["model_verdict"],
                },
            }
            (evidence / "rollback.json").write_text(
                json.dumps(rollback, indent=2, ensure_ascii=False), encoding="utf-8"
            )
        except BaseException:
            if test_error is None:
                raise


if __name__ == "__main__":
    main()
