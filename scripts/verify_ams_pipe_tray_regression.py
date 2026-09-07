"""Fail-closed regression gate for the 2026-09-06 AMS pipe/tray audit.

The offline checks consume the saved full-library audit and native E3D evidence.
Unless ``--offline`` is supplied, the script also asks the backend used by
plant-ui to ensure five native-reference roots and verifies its published model
record and mesh-instance counts.
"""

from __future__ import annotations

import argparse
from collections import Counter
import json
from pathlib import Path
import re
from typing import Any
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[1]
AUDIT_ROOT = ROOT / "output" / "ams-pipe-tray-audit-20260906"
NATIVE_EVIDENCE = (
    ROOT.parent
    / "vendor"
    / "e3d-model"
    / "evidence"
    / "20260902-ams8000-bran-audit"
    / "rvm-pending-items-2026-09-02.md"
)

PIPE_EXPECTED = {
    7997: {
        "branches": 614,
        "verdicts": {"ok": 502, "data": 112},
        "slots": {
            "built": 4622,
            "zero_length": 1633,
            "reversed": 105,
            "misaligned": 62,
            "native_head_filtered": 9,
            "missing_ppoint": 2,
        },
        "resolver_diagnostics": {},
    },
    7999: {
        "branches": 1256,
        "verdicts": {"ok": 508, "data": 748},
        "slots": {
            "built": 2301,
            "zero_length": 4534,
            "misaligned": 633,
            "reversed": 506,
            "native_head_filtered": 31,
            "missing_ppoint": 12,
        },
        "resolver_diagnostics": {
            "db 7320 没注册进这个 DbSet，resolver 也给不出它的文件": 3191,
        },
    },
    8000: {
        "branches": 558,
        "verdicts": {"ok": 334, "data": 224},
        "slots": {
            "zero_length": 2510,
            "misaligned": 182,
            "reversed": 120,
            "built": 32,
            "native_head_filtered": 30,
            "missing_ppoint": 6,
        },
        "resolver_diagnostics": {
            "db 7320 没注册进这个 DbSet，resolver 也给不出它的文件": 239,
        },
    },
}

TRAY_EXPECTED = {
    "CWBRAN": 1365,
    "CTELE": 2067,
    "POSREF missing": 0,
    "CTRAY": 1652,
    "success": 1652,
    "no_geometry": 0,
}

LIVE_SAMPLES = {
    "24384/22803": {
        "name": "/M-BR-1R330-E1",
        "records": 17,
        "meshes": 17,
        "screenshot": "plant-ui-m-br-reversed-isolated.png",
    },
    "24384/23903": {
        "name": "/M-YE-1R320-C",
        "records": 8,
        "meshes": 8,
        "screenshot": "plant-ui-m-ye-misaligned-isolated.png",
    },
    "24384/22404": {
        "name": "/C-IY-1R330-B",
        "records": 35,
        "meshes": 35,
        "screenshot": "plant-ui-c-iy-3d.png",
    },
    "24384/23257": {
        "name": "/C-OR-1R345-C",
        "records": 9,
        "meshes": 9,
        "screenshot": "plant-ui-c-or-isolated-final.png",
    },
    "24383/74956": {
        "name": "/1RCV0243A-60.3-VMB-R41-R520[R510",
        # The records API returns seven TUBI rows sharing the BRAN refno.
        # plant-ui groups that shared identity and reports 14 elements / 18
        # visible mesh instances; the publication receipt counts 20 rows.
        "records": 20,
        "meshes": 20,
        "screenshot": "plant-ui-db7999-valve-isolated.png",
    },
}


def require_equal(label: str, actual: Any, expected: Any) -> None:
    if actual != expected:
        raise ValueError(f"{label}: expected {expected!r}, got {actual!r}")


def load_json(path: Path) -> Any:
    if not path.is_file():
        raise FileNotFoundError(path)
    return json.loads(path.read_text(encoding="utf-8-sig"))


def verify_pipe_audits() -> dict[str, Any]:
    result: dict[str, Any] = {}
    totals = Counter()
    for dbnum, expected in PIPE_EXPECTED.items():
        path = AUDIT_ROOT / f"bran-{dbnum}-final" / "bran_audit.json"
        branches = [row for row in load_json(path) if row.get("owner_noun") == "PIPE"]
        verdicts = Counter(row.get("verdict") for row in branches)
        slots = Counter(slot.get("outcome") for row in branches for slot in row.get("slots", []))
        resolver_diagnostics = Counter(
            failure.split(": ", 1)[1] if ": " in failure else failure
            for row in branches
            for failure in row.get("failed", [])
        )
        require_equal(f"AMS{dbnum} PIPE BRAN", len(branches), expected["branches"])
        require_equal(f"AMS{dbnum} verdicts", dict(verdicts), expected["verdicts"])
        require_equal(f"AMS{dbnum} slots", dict(slots), expected["slots"])
        require_equal(
            f"AMS{dbnum} resolver diagnostics",
            dict(resolver_diagnostics),
            expected["resolver_diagnostics"],
        )
        totals.update(slots)
        result[str(dbnum)] = {
            "branches": len(branches),
            "verdicts": dict(verdicts),
            "slots": dict(slots),
            "resolver_diagnostics": dict(resolver_diagnostics),
        }

    require_equal("all PIPE BRAN", sum(row["branches"] for row in result.values()), 2428)
    require_equal("all PIPE slots", sum(totals.values()), 17330)
    require_equal("all built", totals["built"], 6955)
    return {"databases": result, "totals": dict(totals)}


def verify_tray_audit() -> dict[str, int]:
    path = AUDIT_ROOT / "ctray-all-cway-generate.txt"
    text = path.read_text(encoding="utf-8-sig")
    patterns = {
        "CWBRAN": r"(?m)^CWBRAN\s+(\d+)$",
        "CTELE": r"(?m)^CTELE\s+\{[^\n]+\}$",
        "POSREF missing": r"POSREF 缺\s+(\d+)",
        "CTRAY summary": r"目录生成：CTRAY\s+(\d+)\s+成功\s+(\d+)\s+无几何\s+(\d+)\s+出错\s+\{\}",
    }
    matches = {key: re.search(pattern, text) for key, pattern in patterns.items()}
    missing = [key for key, match in matches.items() if match is None]
    if missing:
        raise ValueError(f"tray audit summary missing: {missing}")
    ctele_counts = json.loads(re.search(patterns["CTELE"], text).group(0).split(None, 1)[1])
    observed = {
        "CWBRAN": int(matches["CWBRAN"].group(1)),
        "CTELE": sum(int(value) for value in ctele_counts.values()),
        "POSREF missing": int(matches["POSREF missing"].group(1)),
        "CTRAY": int(matches["CTRAY summary"].group(1)),
        "success": int(matches["CTRAY summary"].group(2)),
        "no_geometry": int(matches["CTRAY summary"].group(3)),
    }
    require_equal("tray totals", observed, TRAY_EXPECTED)
    return observed


def verify_native_evidence() -> dict[str, Any]:
    text = NATIVE_EVIDENCE.read_text(encoding="utf-8-sig")
    anchors = {
        "reversed": "/M-BR-1R330-E1",
        "misaligned": "/M-YE-1R320-C",
        "one_mrad": "1.000 mrad",
        "native_gate": "两端共轴 + 正向",
        "post_fix": "0 / 0",
    }
    for label, anchor in anchors.items():
        if anchor not in text:
            raise ValueError(f"native evidence lost {label} anchor: {anchor}")
    return {"path": str(NATIVE_EVIDENCE), "anchors": list(anchors)}


def verify_png(path: Path) -> None:
    if not path.is_file() or path.stat().st_size < 1024:
        raise ValueError(f"missing or empty screenshot: {path}")
    if path.read_bytes()[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"invalid PNG signature: {path}")


def request_json(base: str, method: str, path: str, body: dict[str, Any] | None = None) -> Any:
    payload = None if body is None else json.dumps(body).encode("utf-8")
    request = Request(
        base.rstrip("/") + path,
        data=payload,
        method=method,
        headers={"Accept": "application/json", "Content-Type": "application/json"},
    )
    with urlopen(request, timeout=120 if path.endswith("/ensure") else 30) as response:
        return json.loads(response.read().decode("utf-8"))


def verify_live_backend(base: str) -> dict[str, Any]:
    health = request_json(base, "GET", "/api/v1/health")
    require_equal("backend status", health.get("status"), "ok")
    require_equal("backend core", health.get("core", {}).get("status"), "ready")
    require_equal("backend project", health.get("project"), "AvevaMarineSample")
    require_equal("backend MDB", health.get("mdb"), "/ALL")
    identity = {
        "project": health["project"],
        "mdb": health["mdb"],
        "namespace": health["namespace"],
    }
    samples: dict[str, Any] = {}
    for refno, expected in LIVE_SAMPLES.items():
        receipt = request_json(
            base,
            "POST",
            "/api/v1/model/ensure",
            {**identity, "refno": refno, "force": False},
        )
        require_equal(f"{refno} publication", receipt.get("publication_status"), "ready")
        require_equal(f"{refno} model available", receipt.get("model_available"), True)
        require_equal(f"{refno} receipt meshes", receipt.get("model_instance_count"), expected["meshes"])
        page = request_json(
            base,
            "POST",
            "/api/v1/model/records",
            {"generation_root": refno, "limit": 5000},
        )
        require_equal(f"{refno} pagination", page.get("next_cursor"), None)
        items = page.get("items", [])
        meshes = sum(len(item.get("insts", [])) for item in items)
        require_equal(f"{refno} records", len(items), expected["records"])
        require_equal(f"{refno} record meshes", meshes, expected["meshes"])
        verify_png(AUDIT_ROOT / expected["screenshot"])
        samples[refno] = {
            "name": expected["name"],
            "records": len(items),
            "meshes": meshes,
            "publication": "ready",
            "screenshot": expected["screenshot"],
        }
    return {"base_url": base, "namespace": identity["namespace"], "samples": samples}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--backend", default="http://127.0.0.1:18206")
    parser.add_argument("--offline", action="store_true", help="skip live plant-ui backend checks")
    parser.add_argument(
        "--out",
        type=Path,
        default=AUDIT_ROOT / "regression-verification.json",
    )
    args = parser.parse_args()

    report = {
        "status": "passed",
        "pipe": verify_pipe_audits(),
        "tray": verify_tray_audit(),
        "native_e3d": verify_native_evidence(),
        "plant_ui_backend": "not_run" if args.offline else verify_live_backend(args.backend),
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
