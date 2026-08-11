"""E3D TTY macro preparation and execution through the Rust l3_suite driver."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from .rust_tools import RustTools, ToolResult

_TERMINATORS = {"FINISH", "QUIT"}
_ALPHA_LOG = re.compile(r'^\s*ALPHA\s+LOG\s+"[^"]*"\s+OVER\s*$', re.IGNORECASE)


def normalize_macro(source: str, *, alpha_log: Path) -> str:
    """Make a macro TTY-driver-safe and redirect its evidence log."""
    output: list[str] = []
    replaced_log = False
    for line in source.splitlines():
        if line.strip().upper() in _TERMINATORS:
            continue
        if not replaced_log and _ALPHA_LOG.match(line):
            log_path = alpha_log.resolve().as_posix()
            output.append(f'ALPHA LOG "{log_path}" OVER')
            replaced_log = True
        else:
            output.append(line)
    if not replaced_log:
        log_path = alpha_log.resolve().as_posix()
        output.insert(0, f'ALPHA LOG "{log_path}" OVER')
    return "\n".join(output).rstrip() + "\n"


@dataclass
class E3dTtyRunner:
    tools: RustTools
    project_dir: Path
    e3d_install: Path
    evidence_dir: Path
    e3d_project: str = "AMS"
    e3d_mdb: str = "/ALL"

    def run(self, source_macro: Path, *, label: str, timeout: float = 300.0) -> ToolResult:
        self.evidence_dir.mkdir(parents=True, exist_ok=True)
        generated_macro = self.evidence_dir / f"{label}.mac"
        # l3_suite derives the evidence file with Path::with_extension("log").
        # Keep the macro's ALPHA target identical so the Rust driver can verify it.
        alpha_log = generated_macro.with_suffix(".log")
        generated_macro.write_text(
            normalize_macro(source_macro.read_text(encoding="utf-8"), alpha_log=alpha_log),
            encoding="utf-8",
        )
        return self.tools.run_l3_driver(
            generated_macro,
            project_dir=self.project_dir,
            e3d_install=self.e3d_install,
            e3d_project=self.e3d_project,
            e3d_mdb=self.e3d_mdb,
            timeout=timeout,
        )
