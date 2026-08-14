param(
    [string]$DbFile = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001',
    [string]$ProjectDir = 'D:\AVEVA\Projects\E3D3.1\AvevaMarineSample',
    [string]$AiosProject = 'AvevaMarineSample',
    [string]$Refno = '24384_23262',
    [string]$ApplyMacro = 'scripts/e3d/db8000_bran_ftub_move_apply.mac',
    [string]$RestoreMacro = 'scripts/e3d/db8000_bran_ftub_move_restore.mac',
    [string]$ExpectedApplyPos = '10887,12332,3400',
    [string]$ExpectedRestorePos = '10887,12332,2900',
    [string]$Output
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$python = Join-Path $repo 'python\.venv\Scripts\python.exe'
$l3 = Join-Path (Split-Path $repo -Parent) 'target\debug\l3_suite.exe'
if (-not (Test-Path -LiteralPath $l3)) { $l3 = 'D:\Rust\target\debug\l3_suite.exe' }
if (-not $Output) { $Output = Join-Path $repo ('output\e3d-tty-net-window\' + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
New-Item -ItemType Directory -Force -Path $Output | Out-Null

foreach ($path in @($python, $l3, $DbFile, $ProjectDir, (Join-Path $repo $ApplyMacro), (Join-Path $repo $RestoreMacro))) {
    if (-not (Test-Path -LiteralPath $path)) { throw "required path is missing: $path" }
}

$runner = Join-Path $Output 'run.py'
$template = @'
from __future__ import annotations
import hashlib, json, os, shutil, subprocess, sys
from pathlib import Path

repo, evidence, db_file, project_dir, l3, refno, apply_macro, restore_macro = map(Path, sys.argv[1:9])
refno = str(sys.argv[6])
aios_project = sys.argv[9]
expected_apply = [float(value) for value in sys.argv[10].split(",")]
expected_restore = [float(value) for value in sys.argv[11].split(",")]

import aios_db
aios_db.set_config(str(repo / "python/tests/DbOption-ci"))

def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()

def header() -> dict:
    return aios_db.parse.header(str(db_file))

def element(sesno: int) -> dict:
    return aios_db.parse.element(str(db_file), refno, sesno=sesno)

def pos(row: dict) -> list[float]:
    return list(row["attrs"]["POS"]["F32VecType"])

def semantic(start: int, end: int) -> dict:
    return aios_db.parse.net_window(str(db_file), start, end, detail=True)

def ops(result: dict) -> list[dict]:
    return [operation for session in result["window"].values() for operation in session]

def op_for(result: dict, wanted: str) -> dict | None:
    return next((operation for operation in ops(result) if operation["refno"] == wanted), None)

def run_macro(macro: Path, label: str) -> dict:
    output = evidence / f"{label}-driver"
    command = [
        str(l3),
        "--check-driver", str(macro),
        "--target-db-file", str(db_file),
        "--aios-project", aios_project,
        "--project-dir", str(project_dir),
        "--output", str(output),
    ]
    env = os.environ.copy()
    env["L3_ALLOW_EXISTING_E3D_SESSION"] = "1"
    completed = subprocess.run(command, cwd=repo, env=env, text=True, encoding="utf-8", errors="replace", capture_output=True)
    record = {"command": command, "exit_status": completed.returncode, "stdout": completed.stdout, "stderr": completed.stderr}
    (evidence / f"{label}-driver.json").write_text(json.dumps(record, ensure_ascii=False, indent=2), encoding="utf-8")
    if completed.returncode:
        raise RuntimeError(f"{label} failed ({completed.returncode}): {completed.stdout}\n{completed.stderr}")
    return record

before = header()
before_sesno = int(before["latest_sesno"])
before_element = element(before_sesno)
backup = evidence / "baseline-db-file.copy"
shutil.copy2(db_file, backup)
summary = {
    "input": {"db_file": str(db_file), "project_dir": str(project_dir), "refno": refno},
    "baseline": {"header": before, "element": before_element, "backup": str(backup), "backup_sha256": sha256(backup)},
}
apply_error: BaseException | None = None
apply_op: dict | None = None
restore_op: dict | None = None
try:
    summary["apply_driver"] = run_macro(apply_macro, "apply")
    apply_header = header()
    apply_sesno = int(apply_header["latest_sesno"])
    if apply_sesno != before_sesno + 1:
        raise AssertionError(f"apply session must be {before_sesno + 1}, got {apply_sesno}")
    apply_element = element(apply_sesno)
    apply_net = semantic(apply_sesno, apply_sesno)
    apply_op = op_for(apply_net, refno)
    if apply_op is None or apply_op["op"] != "modified":
        raise AssertionError(f"apply must report {refno} modified: {apply_net['counts']}")
    if pos(apply_element) != expected_apply:
        raise AssertionError(f"apply POS mismatch: {pos(apply_element)} != {expected_apply}")
    summary["apply"] = {"header": apply_header, "element": apply_element, "net_window": apply_net}
except BaseException as error:
    apply_error = error
    summary["apply_error"] = f"{type(error).__name__}: {error}"
finally:
    try:
        current_sesno = int(header()["latest_sesno"])
        if current_sesno > before_sesno or pos(element(current_sesno)) != expected_restore:
            summary["restore_driver"] = run_macro(restore_macro, "restore")
        restore_header = header()
        restore_sesno = int(restore_header["latest_sesno"])
        restore_element = element(restore_sesno)
        restore_net = semantic(restore_sesno, restore_sesno)
        restore_op = op_for(restore_net, refno)
        combined = semantic(before_sesno + 1, restore_sesno)
        combined_op = op_for(combined, refno)
        if pos(restore_element) != expected_restore:
            raise AssertionError(f"restore POS mismatch: {pos(restore_element)} != {expected_restore}")
        if restore_element["attrs"] != before_element["attrs"] or restore_element["explicit_attrs"] != before_element["explicit_attrs"]:
            raise AssertionError("restore did not return target element attributes to baseline")
        if restore_op is None or restore_op["op"] != "modified":
            raise AssertionError(f"restore must report {refno} modified: {restore_net['counts']}")
        if combined_op is not None:
            raise AssertionError(f"apply+restore must cancel target business change: {combined_op}")
        non_metadata = [operation for operation in ops(combined) if set(operation.get("modified_explicit", {})) - {"CACHID"}]
        if non_metadata:
            raise AssertionError(f"combined window has non-CACHID explicit changes: {non_metadata}")
        summary["restore"] = {"header": restore_header, "element": restore_element, "net_window": restore_net}
        summary["combined"] = combined
        summary["rollback"] = {"command": summary.get("restore_driver", {}).get("command"), "verified": True}
        summary["modified_artifact"] = str(db_file)
        patch_diff = evidence / "semantic-window-diff.json"
        patch_diff.write_text(json.dumps({
            "apply_target": apply_op,
            "restore_target": restore_op,
            "combined_window": combined,
        }, ensure_ascii=False, indent=2, default=str), encoding="utf-8")
        summary["patch_diff"] = str(patch_diff)
        summary["verification_record"] = str(evidence / "summary.json")
    except BaseException as error:
        summary["restore_error"] = f"{type(error).__name__}: {error}"
        (evidence / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2, default=str), encoding="utf-8")
        raise

(evidence / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2, default=str), encoding="utf-8")
if apply_error is not None:
    raise apply_error
print(json.dumps({
    "evidence": str(evidence),
    "sessions": [before_sesno, summary["apply"]["header"]["latest_sesno"], summary["restore"]["header"]["latest_sesno"]],
    "apply_counts": summary["apply"]["net_window"]["counts"],
    "restore_counts": summary["restore"]["net_window"]["counts"],
    "combined_counts": summary["combined"]["counts"],
    "combined_unchanged_rewrites": summary["combined"]["unchanged_rewrites"],
    "rollback_verified": summary["rollback"]["verified"],
}, ensure_ascii=False, indent=2))
'@
Set-Content -LiteralPath $runner -Value $template -Encoding utf8

& $python $runner $repo (Resolve-Path $Output).Path (Resolve-Path $DbFile).Path (Resolve-Path $ProjectDir).Path $l3 $Refno (Resolve-Path (Join-Path $repo $ApplyMacro)).Path (Resolve-Path (Join-Path $repo $RestoreMacro)).Path $AiosProject $ExpectedApplyPos $ExpectedRestorePos
if ($LASTEXITCODE) { throw "TTY net-window test failed (exit $LASTEXITCODE); evidence: $Output" }
