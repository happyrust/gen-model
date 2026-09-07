"""Replay one pinned window; require strict artifacts and preserve command evidence.

This entry point reads database files only. A JSON case declares its DB, S/T,
before_roots/after_roots and optional RVMs keyed by target root refno.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

from verify import read_json, verify_delta, verify_rvm

REPO = Path(__file__).resolve().parents[2]


def fingerprint(path):
    path = Path(path).resolve()
    stat = path.stat()
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    return {"path": str(path), "bytes": stat.st_size, "mtime_ns": stat.st_mtime_ns, "sha256": digest}


def run_case(case, out, tools):
    out.mkdir(parents=True, exist_ok=False)
    result = {"case": case, "passed": False, "commands": [], "live": "not_run"}

    def command(exe, args, label):
        executable = tools / exe
        argv = [str(executable), *map(str, args)]
        with (out / f"{label}.log").open("w", encoding="utf-8") as log:
            proc = subprocess.run(argv, cwd=REPO, stdout=log, stderr=subprocess.STDOUT,
                                  timeout=600, encoding="utf-8")
        result["commands"].append({"argv": argv, "exit_code": proc.returncode,
                                   "binary": fingerprint(executable)})
        if proc.returncode:
            raise RuntimeError(f"{label}: exit {proc.returncode}; see {out / (label + '.log')}")

    try:
        result["database_before"] = fingerprint(case["db"])
        command(str(REPO / "python/.venv/Scripts/python.exe"),
                [REPO / "docs/evidence/2026-09-06-hvac-100817-increment-tty-rvm/net_window.py",
                 case["db"], case["base"] + 1, case["target"], out / "net-window.json"], "net-window")
        net = read_json(out / "net-window.json")["summary"]
        refs = [op["refno"] for op in net["ops"]]
        result["data"] = {"counts": net["counts"], "unparseable_finals": net["unparseable_finals"],
                          "unique_terminal_ops": len(refs) == len(set(refs)),
                          "owner_changes": sum("OWNER" in op["modified_attrs"] for op in net["ops"])}
        common = ["--attlib", case["attlib"], "--template", case["template"], "--db", case["db"]]
        for catalogue in case.get("catalogues", []):
            common += ["--catalogue", catalogue]
        for directory in case["project_dirs"]:
            common += ["--project-dir", directory]
        command("update_ams.exe", common + ["--base-sesno", case["base"], "--target-sesno", case["target"],
                                           "--out", out / "update"], "update")
        snapshots = {}
        for side, sesno in (("before", case["base"]), ("after", case["target"])):
            snapshots[side] = []
            for root in case[f"{side}_roots"]:
                folder = out / f"{side}-{root.replace('/', '-')}"
                command("gen_ams.exe", common + ["--root", root, "--sesno", sesno, "--out", folder], folder.name)
                report = read_json(folder / "report.json")
                # gen_ams reports generation defects separately from process exit.
                if any(report.get(k) for k in ("failed", "failures", "catalog_pending", "catalogue_chain_broken")):
                    raise RuntimeError(f"{folder.name}: failed generation: {report}")
                snapshots[side] += read_json(folder / "elements.json")
            (out / f"{side}.json").write_text(json.dumps(snapshots[side]), encoding="utf-8")
        result["delta"] = verify_delta(snapshots["before"], snapshots["after"],
                                       read_json(out / "update/upserts.json"), read_json(out / "update/removals.json"))
        report = read_json(out / "update/update-report.json")
        result["generation_errors"] = {f: report[f] for f in
            ("unresolved", "regen_failed", "derived_failed", "derived_stale_unreadable") if report.get(f)}
        result["expected_changes"] = {k: {"expected": v, "actual": report["changes"].get(k)}
                                      for k, v in case.get("expected_changes", {}).items()}
        result["rvm"] = {}
        for root, source in case.get("rvms", {}).items():
            label = root.replace("/", "-")
            snapshot, facets, comparison = [out / f"{label}-rvm-{name}.json" for name in ("snapshot", "facets", "compare")]
            command("rvm_verify.exe", ["import", "--rvm", source, "--dbnum", case["dbnum"], "--scope", "narrow",
                                       "--root-refno", root, "--out", snapshot], f"rvm-import-{label}")
            command("examples/rvm_facets_dump.exe", ["--rvm", source, "--contains", "/", "--out", facets], f"rvm-facets-{label}")
            command("rvm_compare.exe", ["--baseline", snapshot, "--elements", out / f"after-{label}/elements.json",
                                        "--facets", facets, "--out", comparison], f"rvm-compare-{label}")
            result["rvm"][root] = {**verify_rvm(read_json(comparison)), "source": fingerprint(source)}
        result["database_after"] = fingerprint(case["db"])
        result["passed"] = (result["delta"]["passed"] and not result["generation_errors"]
            and not result["data"]["unparseable_finals"] and result["data"]["unique_terminal_ops"]
            and ("reparented" not in case.get("expected_changes", {}) or
                 result["data"]["owner_changes"] == case["expected_changes"]["reparented"])
            and all(v["expected"] == v["actual"] for v in result["expected_changes"].values())
            and all(r["passed"] for r in result["rvm"].values())
            and result["database_before"] == result["database_after"])
        result["rvm_status"] = "checked" if result["rvm"] else "not_run"
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.TimeoutExpired) as error:
        result["error"] = str(error)
    (out / "acceptance.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("case", type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--tools", type=Path, default=Path("D:/Rust/target/debug"))
    args = parser.parse_args()
    result = run_case(read_json(args.case), args.out.resolve(), args.tools.resolve())
    print(json.dumps({k: v for k, v in result.items() if k not in ("commands", "case")}, ensure_ascii=False))
    sys.exit(0 if result["passed"] else 1)
