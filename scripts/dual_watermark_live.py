"""Capture an AMS8000 data/model watermark transition around one real E3D save."""

from __future__ import annotations

import json
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

import equi8000_coup_publication_regression as coup


REPO = Path(__file__).resolve().parents[1]
EVIDENCE_DIR = REPO / "output/rocksdb-8000-20260905/dual-watermark"


def dbnum_status() -> dict:
    with urllib.request.urlopen(
        "http://127.0.0.1:18050/api/v1/dbnums", timeout=10
    ) as response:
        payload = json.load(response)
    return next(row for row in payload["dbnums"] if row["dbnum"] == 8000)


def sample(started: float) -> dict:
    row = dbnum_status()
    return {
        "elapsed_ms": round((time.monotonic() - started) * 1000),
        "file_latest_sesno": row["file_latest_sesno"],
        "applied_sesno": row["applied_sesno"],
        "model_sesno": row["model_sesno"],
        "model_verdict": row["model_verdict"],
        "model_chasing_roots": row["model_chasing_roots"],
        "model_dead_roots": row["model_dead_roots"],
        "model_lazy_roots": row["model_lazy_roots"],
    }


def main(label: str) -> None:
    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    output = EVIDENCE_DIR / f"{label}.json"
    edit_log = EVIDENCE_DIR / f"{label}-edit.log"
    source_before = coup.source_session()
    started = time.monotonic()
    observations: list[dict] = []
    last_state: tuple | None = None

    with edit_log.open("w", encoding="utf-8") as log:
        editor = subprocess.Popen(
            [
                sys.executable,
                str(REPO / "scripts/equi8000_coup_publication_regression.py"),
                "edit",
                "oriented",
                label,
            ],
            cwd=REPO,
            stdout=log,
            stderr=subprocess.STDOUT,
        )

        deadline = time.monotonic() + 240
        target = source_before
        while time.monotonic() < deadline:
            try:
                current = sample(started)
            except Exception as error:  # retain transient HTTP failures as evidence
                current = {
                    "elapsed_ms": round((time.monotonic() - started) * 1000),
                    "error": repr(error),
                }
            state = tuple((key, value) for key, value in current.items() if key != "elapsed_ms")
            if state != last_state:
                observations.append(current)
                last_state = state

            target = coup.source_session()
            if (
                editor.poll() is not None
                and editor.returncode == 0
                and target > source_before
                and current.get("applied_sesno") == target
                and current.get("model_sesno") == target
                and current.get("model_verdict") == "in_sync"
            ):
                break
            if editor.poll() is not None and editor.returncode != 0:
                raise RuntimeError(f"E3D edit failed with exit code {editor.returncode}")
            time.sleep(0.1)
        else:
            editor.kill()
            raise TimeoutError("dual-watermark transition did not converge")

    evidence = {
        "dbnum": 8000,
        "edit": "COUP ORI Y IS N AND Z IS U",
        "source_before": source_before,
        "source_after": target,
        "observations": observations,
        "passed": target > source_before
        and observations[-1]["applied_sesno"] == target
        and observations[-1]["model_sesno"] == target
        and observations[-1]["model_verdict"] == "in_sync",
    }
    output.write_text(json.dumps(evidence, indent=2, ensure_ascii=False), encoding="utf-8")
    print(json.dumps(evidence, ensure_ascii=False))


if __name__ == "__main__":
    main(sys.argv[1])
