#!/usr/bin/env python3
"""Run a DB8000 increment case with Rust parsers and optional E3D TTY mutation."""

from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime
from pathlib import Path

from gen_model_testing import E3dTtyRunner, GenModelClient, ProjectIdentity, RustTools


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    result.add_argument("--bin-dir", type=Path, required=True)
    result.add_argument("--db-file", type=Path, required=True)
    result.add_argument("--from-sesno", type=int, required=True)
    result.add_argument("--to-sesno", type=int, required=True)
    result.add_argument("--dbnum", type=int, default=8000)
    result.add_argument("--api", default="http://127.0.0.1:3000")
    result.add_argument("--project", default="AMS")
    result.add_argument("--mdb", default="/ALL")
    result.add_argument("--namespace", default="ams")
    result.add_argument("--evidence-dir", type=Path)
    result.add_argument("--macro", type=Path, help="E3D macro to execute before collection")
    result.add_argument("--restore-macro", type=Path, help="Always executed after --macro")
    result.add_argument("--project-dir", type=Path)
    result.add_argument("--e3d-install", type=Path)
    result.add_argument("--execute", action="store_true", help="Submit the normal Rust increment API")
    return result


def main() -> int:
    args = parser().parse_args()
    evidence = (args.evidence_dir or args.bin_dir / "output" / (
        "python-db8000-" + datetime.now().strftime("%Y%m%d-%H%M%S")
    )).resolve()
    evidence.mkdir(parents=True, exist_ok=False)
    tools = RustTools(args.repo, bin_dir=args.bin_dir)
    client = GenModelClient(
        args.api,
        ProjectIdentity(args.project, args.mdb, args.namespace),
    )
    runner = None
    if args.macro:
        if not args.restore_macro or not args.project_dir or not args.e3d_install:
            raise SystemExit("--macro requires --restore-macro, --project-dir and --e3d-install")
        runner = E3dTtyRunner(tools, args.project_dir, args.e3d_install, evidence)

    summary: dict[str, object] = {"dbnum": args.dbnum, "evidence_dir": str(evidence)}
    failure: BaseException | None = None
    restore_failure: BaseException | None = None
    try:
        if runner:
            applied = runner.run(args.macro, label="apply")
            summary["e3d_apply"] = {"returncode": applied.returncode, "stdout": applied.stdout}

        folded = tools.run_increment_fold(
            args.db_file, args.from_sesno, args.to_sesno, dbnum=args.dbnum
        )
        summary["fold"] = {
            "returncode": folded.returncode,
            "stdout": folded.stdout,
            "stderr": folded.stderr,
        }
        client.wait_for_health()
        summary["preview"] = client.preview()
        if args.execute:
            summary["execute"] = client.execute([args.dbnum])
            summary["watermark"] = client.wait_for_watermark(args.dbnum, args.to_sesno)
    except BaseException as exc:
        failure = exc
        summary["error"] = f"{type(exc).__name__}: {exc}"
    finally:
        if runner:
            try:
                restored = runner.run(args.restore_macro, label="restore")
                summary["e3d_restore"] = {
                    "returncode": restored.returncode,
                    "stdout": restored.stdout,
                }
            except BaseException as exc:
                restore_failure = exc
                summary["restore_error"] = f"{type(exc).__name__}: {exc}"
        (evidence / "summary.json").write_text(
            json.dumps(summary, ensure_ascii=False, indent=2, default=str) + "\n",
            encoding="utf-8",
        )
    print(json.dumps(summary, ensure_ascii=False, indent=2, default=str))
    if failure is not None:
        raise failure
    if restore_failure is not None:
        raise restore_failure
    return 0


if __name__ == "__main__":
    sys.exit(main())
