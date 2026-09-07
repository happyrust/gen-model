"""Record an isolated scratch chain using E3D's guarded TTY driver.

Each leg has one SAVEWORK and its own RVM exports. Only names minted by this
run are edited/deleted. The original source is used by COPY only.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import uuid

from run_window import REPO, run_case
from verify import read_json, elements_by_id, same_geometry, business_residuals

PROJECT = Path("D:/AVEVA/Projects/E3D3.1/AvevaMarineSample")
FAMILIES = {
    "pipe": dict(dbnum=8000, noun="BRAN", parent="/1RX-345",
                 source="/C-OR-1R345-C", member_noun="FTUB", member_source="=24384/23262"),
    "equipment": dict(dbnum=8000, noun="EQUI", parent="/1RX03-CASE-DQ",
                      source="/1-LNR-Q005-PJ", member_noun="BOX", member_source="=24384/24777"),
    "hvac": dict(dbnum=7997, noun="BRAN", parent="=24381/100816",
                 source="/-CUP-S-3-M-1201", member_noun="DAMP", member_source="=24381/100819"),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("family", choices=FAMILIES)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--tools", type=Path, default=Path("D:/Rust/target/debug"))
    parser.add_argument("--project-dir", type=Path, default=PROJECT)
    parser.add_argument("--catalogue-7600", type=Path,
                        default=Path("D:/AVEVA/Projects/E3D3.1/ZDJ/ZDJ000/zdj7600_0001"),
                        help="Must match the live service's selected 7600 catalogue (repository default: ZDJ)")
    parser.add_argument("--live-url", help="Optional isolated durable service to observe after each window")
    args = parser.parse_args()
    project = args.project_dir.resolve()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    cfg = FAMILIES[args.family]
    prefix = f"/CODEX_CASCADE_{args.family.upper()}_{uuid.uuid4().hex[:8].upper()}"
    names = [prefix + "_A", prefix + "_B"]
    member = prefix + "_MEMBER"
    db = project / "ams000" / f"ams{cfg['dbnum']}_0001"
    common = dict(db=str(db), dbnum=cfg["dbnum"], attlib="E:/reverse/e3d/shadow_e3d31_aps_all/attlib.dat",
        template="E:/reverse/e3d/shadow_e3d31_aps_all", project_dirs=[str(project / "ams000"),
        "D:/AVEVA/Projects/E3D3.1/AvevaCatalogue/acp000"], catalogues=[str(project / "ams000/ams7000_0001"),
        str(args.catalogue_7600.resolve())])
    summary = {"family": args.family, "scratch": names, "legs": [], "cleanup": "not_run"}
    roots = []

    def tty(label, body, export=True):
        macro = out / f"{label}.mac"
        lines = [f'ALPHA LOG "{macro.with_suffix(".log").as_posix()}" OVER',
                 f"$P CODEX-CASCADE-{label}-ALIVE", *body]
        if export:
            lines += ["repre insu off", "repre obst off", "repre tube on", "repre lev hvac 6",
                "export implied tube into separate containers", "export system /expdri.so",
                "export holes on", "export autocolour displayexport on", "export repr on"]
            for i, name in enumerate(names):
                lines += [f'export file "{(out / f"{label}-{i}.rvm").as_posix()}"',
                          f"export {name}", "export finish"]
        lines += [f"SAVEWORK 'CODEX cascade {args.family} {label}'",
                  f"$P CODEX-CASCADE-{label}-DONE", "ALPHA LOG END"]
        macro.write_text("\n".join(lines) + "\n", encoding="ascii")
        argv = [str(args.tools / "l3_suite.exe"), "--check-driver", str(macro),
            "--project-dir", str(project), "--target-db-file", str(db), "--target-dbnum", str(cfg["dbnum"]),
            "--aios-project", "AvevaMarineSample", "--output", str(out / f"{label}-driver")]
        with (out / f"{label}-driver.log").open("w", encoding="utf-8") as log:
            env = os.environ.copy()
            env.update(L3_PROJECT_WORK=str(project), L3_E3D_PROJECTS_DIR=str(project.parent),
                       L3_E3D_PROJECT_EVAR=str(project / "evarsAvevaMarineSample.bat"))
            proc = subprocess.run(argv, cwd=REPO, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=300)
        if proc.returncode:
            raise RuntimeError(f"{label}: driver exit {proc.returncode}; inspect {label}-driver.log")
        evidence = read_json(out / f"{label}-driver/check-driver-evidence.json")
        if evidence["outcome"] != "completed":
            raise RuntimeError(f"{label}: TTY did not complete; inspect {label}-driver.log")
        return evidence, macro.with_suffix(".log").read_text(encoding="utf-8-sig")

    def verify(label, evidence, before, after, changes=None, export=True):
        case = {**common, "base": evidence["before_sesno"], "target": evidence["after_sesno"],
                "before_roots": before, "after_roots": after, "expected_changes": changes or {},
                "rvms": {r: str(out / f"{label}-{i}.rvm") for i, r in enumerate(roots)} if export else {}}
        (out / f"{label}.json").write_text(json.dumps(case, indent=2), encoding="utf-8")
        result = run_case(case, out / f"{label}-verify", args.tools.resolve())
        behavior = True
        if result.get("delta") and result.get("data"):
            if label in ("01-create", "04-add-member"):
                behavior = result["data"]["counts"]["added"] > 0
            elif label in ("06-delete-member", "07-cleanup"):
                behavior = result["data"]["counts"]["deleted"] > 0
            elif label == "02-move":
                a, b = [elements_by_id(read_json(out / f"{label}-verify/{side}.json"))
                        for side in ("before", "after")]
                behavior = a.keys() == b.keys() and any(not same_geometry(a[k], b[k]) for k in a)
            elif label == "03-restore-position":
                a = elements_by_id(read_json(out / "01-create-verify/after.json"))
                b = elements_by_id(read_json(out / f"{label}-verify/after.json"))
                behavior = a.keys() == b.keys() and all(same_geometry(a[k], b[k]) for k in a)
        else:
            behavior = False
        result["operation_observed"] = behavior
        result["passed"] = result["passed"] and behavior
        if args.live_url and result["passed"]:
            from live import observe
            live_result = observe(args.live_url, case, out / f"{label}-verify/live")
            result["live"] = {"passed": live_result["passed"], "evidence": f"{label}-verify/live/acceptance.json"}
            result["passed"] = result["passed"] and live_result["passed"]
        (out / f"{label}-verify/acceptance.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
        summary["legs"].append({"label": label, "base": case["base"], "target": case["target"],
                                "passed": result["passed"], "error": result.get("error")})
        (out / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
        print(json.dumps(summary["legs"][-1]), flush=True)
        if not result["passed"]:
            raise RuntimeError(f"{label}: acceptance failed; inspect {label}-verify/acceptance.json")

    try:
        create = []
        for i, name in enumerate(names):
            create += [cfg["parent"], f"NEW {cfg['noun']} {name}", f"COPY {cfg['source']}",
                       f"$P ROOT-{i}-BEGIN", "Q REF", f"$P ROOT-{i}-END"]
        evidence, log = tty("01-create", create)
        for i in range(2):
            block = re.search(rf"ROOT-{i}-BEGIN(.*?)ROOT-{i}-END", log, re.S).group(1)
            roots.append(re.search(r"Ref\s*=\s*(\d+/\d+)", block).group(1))
        verify("01-create", evidence, [], roots)
        evidence, _ = tty("02-move", [names[0], "BY U 500"])
        verify("02-move", evidence, roots, roots)
        # This fixture moves exactly 500 mm along U. Reverse that same move
        # without rewriting unchanged coordinates through lossy query/parser
        # formatting. The next gate checks every geometry against creation.
        evidence, _ = tty("03-restore-position", [names[0], "BY D 500"])
        verify("03-restore-position", evidence, roots, roots)
        evidence, _ = tty("04-add-member", [names[0], f"NEW {cfg['member_noun']} {member}",
                                             f"COPY {cfg['member_source']}", "Q REF", "Q OWNE"])
        verify("04-add-member", evidence, roots, roots)
        evidence, _ = tty("05-reparent", [names[1], f"INCLUDE {member}", member, "Q REF", "Q OWNE"])
        verify("05-reparent", evidence, roots, roots, {"reparented": 1})
        evidence, _ = tty("06-delete-member", [member, f"DELETE {cfg['member_noun']}"])
        verify("06-delete-member", evidence, roots, roots)
    except (OSError, RuntimeError, ValueError, AttributeError, subprocess.TimeoutExpired) as error:
        summary["error"] = str(error)
        print(str(error), flush=True)
    finally:
        if len(roots) == 2:
            try:
                evidence, _ = tty("07-cleanup", [command for name in names
                    for command in (name, f"DELETE {cfg['noun']}")], export=False)
                summary["cleanup"] = "committed"
                verify("07-cleanup", evidence, roots, [], export=False)
            except (OSError, RuntimeError, ValueError, subprocess.TimeoutExpired) as error:
                summary["cleanup_error"] = str(error)
        if summary["cleanup"] == "committed":
            try:
                first = read_json(out / "01-create.json")["base"]
                last = read_json(out / "07-cleanup.json")["target"]
                merged = out / "merged-net-window.json"
                with (out / "merged-net-window.log").open("w", encoding="utf-8") as log:
                    subprocess.run([str(REPO / "python/.venv/Scripts/python.exe"),
                        str(REPO / "docs/evidence/2026-09-06-hvac-100817-increment-tty-rvm/net_window.py"),
                        str(db), str(first + 1), str(last), str(merged)], check=True, stdout=log, stderr=log)
                summary["business_residuals"] = business_residuals(read_json(merged)["summary"])
                if summary["business_residuals"]:
                    summary["cleanup_error"] = "merged window retains business changes"
            except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
                summary["cleanup_error"] = f"merged-window check: {error}"
        summary["passed"] = not summary.get("error") and not summary.get("cleanup_error") and summary["cleanup"] == "committed"
        (out / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
