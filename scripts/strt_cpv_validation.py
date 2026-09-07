"""Read-only E3D TTY/RVM captures for the CPV ventilation regression.

Uses an isolated E3D user/work directory and one export per TTY session.
No SAVEWORK or design edits.
"""
from pathlib import Path
import os
import subprocess
import sys
import json
import re
import numpy as np

REPO = Path(__file__).resolve().parents[1]
OUT = REPO / "output/strt-cpv-20260905"
MODEL = REPO.parent / "vendor/e3d-model"
TARGET = REPO.parent / "target-e3d-model"


def capture():
    OUT.mkdir(parents=True, exist_ok=True)
    for name in ("user", "work"):
        (OUT / name).mkdir(exist_ok=True)
    source = (REPO / "scripts/e3d/run_ams_c_entrymacro.bat").read_text()
    source = source.replace(
        "set AVEVA_DESIGN_ENTRYMACRO=",
        f"set AVEVA_DESIGN_USER={OUT / 'user'}\\\n"
        f"set AVEVA_DESIGN_WORK={OUT / 'work'}\\\n"
        "set AVEVA_DESIGN_ENTRYMACRO=",
    ).replace("%~dp0launch_detached.ps1", str(REPO / "scripts/e3d/launch_detached.ps1"))
    launcher = OUT / "run-isolated-user.bat"
    launcher.write_text(source)
    for root, level, name in (
        ("=24381/76729", 0, "branch-hvac0"),
        ("=24381/76729", 6, "branch-hvac6"),
        ("/1CPV-HVACHB", 6, "site-hvac6"),
    ):
        macro = OUT / f"{name}.mac"
        alpha = OUT / f"{name}.log"
        lines = [f'ALPHA LOG "{alpha.as_posix()}" OVER', "$P STRT-CPV-START",
                  root, "Q REF", "repre lev 6", f"repre level hvac {level}",
                  "repre insu off", "repre obst off", "repre tube on", "Q REPRE",
                  "export implied tube into separate containers", "export system /expdri.so",
                  f'export file "{OUT.as_posix()}/{name}.rvm"', "export holes on",
                  "export autocolour displayexport on", "export repr on", "export CE",
                  "export finish", "$P STRT-CPV-DONE", "ALPHA LOG END", "QUIT", "FINISH", "FINISH", ""]
        macro.write_text("\n".join(lines), encoding="utf-8")
        env = os.environ.copy()
        env["L3_E3D_TIMEOUT_SECONDS"] = "180"
        with (OUT / f"{name}-launcher.log").open("w") as log:
            subprocess.run([str(launcher), str(macro)], cwd=REPO, env=env,
                           stdout=log, stderr=subprocess.STDOUT, check=True, timeout=210)
        text = alpha.read_text(encoding="utf-8-sig", errors="replace")
        assert "STRT-CPV-DONE" in text
        assert (OUT / f"{name}.rvm").stat().st_size > 128, name
        dump = "site-strt" if name == "site-hvac6" else name
        subprocess.run([str(TARGET / "release/examples/rvm_facets_dump.exe"),
                        "--rvm", str(OUT / f"{name}.rvm"), "--contains", "STRT",
                        "--out", str(OUT / f"{dump}.json")], check=True)
    print("E3D TTY exported branch HVAC 0/6 and CPV site HVAC 6:", OUT)


def compare(elements_path, report_path):
    """Compare all CPV STRTs to native PRIM dimensions/frames (not record bbox)."""
    elements = json.loads(Path(elements_path).read_text(encoding="utf-8"))
    groups = json.loads((OUT / "site-strt.json").read_text(encoding="utf-8"))
    by_key = {(e["owner_desc"].split(" ", 1)[1], e["ordinal"]): e
              for e in elements if e["noun"] == "STRT"}
    pairs = []
    for group in groups:
        match = re.fullmatch(r"STRT (\d+) of BRANCH (.+)", group["name"])
        assert match, group["name"]
        element = by_key[(match[2], int(match[1]))]
        world = np.eye(4)
        world[:3, :] = np.array(element["world"]).T
        inverse = np.linalg.inv(world)
        bounds = []
        for geometry in group["geometries"]:
            transform = np.eye(4)
            transform[:3, :] = np.array(geometry["transform_cols_m"]).reshape(4, 3).T * 1000
            local = inverse @ transform
            if geometry["kind"] == "Cylinder":
                match = re.search(r"radius: ([\d.eE+-]+), height: ([\d.eE+-]+)", geometry["params"])
                radius, height = map(float, match.groups())
                extents = radius * np.sqrt(local[:3, 0]**2 + local[:3, 1]**2) + height / 2 * abs(local[:3, 2])
            elif geometry["kind"] == "Box":
                lengths = np.array(json.loads(re.search(r"lengths: (\[[^]]+\])", geometry["params"])[1]))
                extents = abs(local[:3, :3]) @ (lengths / 2)
            else:
                raise AssertionError("Unhandled native primitive: " + geometry["kind"])
            bounds.append((local[:3, 3] - extents, local[:3, 3] + extents))
        expected = np.r_[np.min([b[0] for b in bounds], axis=0), np.max([b[1] for b in bounds], axis=0)]
        actual = np.array(element["local_aabb_min"] + element["local_aabb_max"])
        error = float(max(abs(expected - actual)))
        pairs.append(dict(refno=element["refno"], name=group["name"], error_mm=error, passed=error <= .51,
                          expected_local_bounds_mm=expected.tolist(), actual_local_bounds_mm=actual.tolist()))
    assert len(pairs) == 82, "CPV site must contain 82 STRT members"
    # When given a site-only generation, require both sides to have identical membership.
    if len(by_key) == 82:
        assert {p["refno"] for p in pairs} == {e["refno"] for e in by_key.values()}
    report = dict(passed=all(p["passed"] for p in pairs), count=len(pairs),
                  failures=sum(not p["passed"] for p in pairs), tolerance_mm=.51,
                  max_error_mm=max(p["error_mm"] for p in pairs),
                  source="E3D TTY site-hvac6.rvm; native Box/Cylinder dimensions and PRIM transforms", pairs=pairs)
    Path(report_path).write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    print(json.dumps({k:v for k,v in report.items() if k != "pairs"}))
    assert report["passed"], "STRT geometry differs from E3D RVM"


def verify():
    OUT.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.update(CARGO_TARGET_DIR=str(TARGET), E3D_REPRE_LEVEL_HVAC="6", E3D_REPRE_OBST="off")
    with (OUT / "verification-build.log").open("w") as log:
        subprocess.run(["cargo", "build", "--release", "--bin", "gen_ams", "--example", "rvm_facets_dump"],
                       cwd=MODEL, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
    capture()
    ams = Path("D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000")
    template = Path("E:/reverse/e3d/shadow_e3d31_aps_all")
    with (OUT / "verification-generation.log").open("w") as log:
        subprocess.run([str(TARGET / "release/gen_ams.exe"), "--attlib", str(template / "attlib.dat"),
                        "--db", str(ams / "ams7997_0001"), "--template", str(template),
                        "--catalogue", str(ams / "ams7000_0001"), "--project-dir", str(ams),
                        "--project-dir", "D:/AVEVA/Projects/E3D3.1/AvevaCatalogue/acp000",
                        "--root", "24381/76564", "--out", str(OUT / "after-site")],
                       env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
    compare(OUT / "after-site/elements.json", OUT / "after-comparison.json")


if __name__ == "__main__":
    if sys.argv[1:] == ["capture"]:
        capture()
    elif len(sys.argv) == 4 and sys.argv[1] == "compare":
        compare(sys.argv[2], sys.argv[3])
    elif sys.argv[1:] == ["verify"]:
        verify()
    else:
        raise SystemExit("Usage: strt_cpv_validation.py verify | capture | compare elements.json report.json")
