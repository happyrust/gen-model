"""Process adapters for Rust executables used by Python regression scripts."""

from __future__ import annotations

import os
import subprocess
import time
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator, Mapping, Sequence

from .client import GenModelClient


@dataclass(frozen=True)
class ToolResult:
    command: tuple[str, ...]
    returncode: int
    stdout: str
    stderr: str
    duration_seconds: float


class RustToolError(RuntimeError):
    def __init__(self, result: ToolResult):
        super().__init__(
            f"Rust tool exited {result.returncode}: {' '.join(result.command)}\n{result.stderr}"
        )
        self.result = result


class RustTools:
    def __init__(self, repo: Path, *, bin_dir: Path | None = None) -> None:
        self.repo = Path(repo).resolve()
        self.bin_dir = Path(bin_dir).resolve() if bin_dir else None

    def executable(self, name: str) -> Path:
        suffix = ".exe" if os.name == "nt" else ""
        filename = name if name.endswith(suffix) else name + suffix
        candidates: list[Path] = []
        if self.bin_dir:
            candidates.append(self.bin_dir / filename)
        if os.environ.get("CARGO_TARGET_DIR"):
            candidates.append(Path(os.environ["CARGO_TARGET_DIR"]) / "debug" / filename)
        candidates.append(self.repo / "target" / "debug" / filename)
        for candidate in candidates:
            if candidate.is_file():
                return candidate.resolve()
        raise FileNotFoundError(f"Rust executable {filename} not found; checked {candidates}")

    def run(
        self,
        name: str,
        args: Sequence[str | os.PathLike[str]],
        *,
        cwd: Path | None = None,
        env: Mapping[str, str] | None = None,
        timeout: float = 300.0,
        check: bool = True,
    ) -> ToolResult:
        command = (str(self.executable(name)), *(str(arg) for arg in args))
        merged_env = os.environ.copy()
        if env:
            merged_env.update(env)
        started = time.monotonic()
        completed = subprocess.run(
            command,
            cwd=str(cwd or self.repo),
            env=merged_env,
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            timeout=timeout,
            check=False,
        )
        result = ToolResult(
            command=command,
            returncode=completed.returncode,
            stdout=completed.stdout,
            stderr=completed.stderr,
            duration_seconds=time.monotonic() - started,
        )
        if check and result.returncode != 0:
            raise RustToolError(result)
        return result

    def run_increment_fold(
        self, db_file: Path, from_sesno: int, to_sesno: int, *, dbnum: int = 8000
    ) -> ToolResult:
        return self.run(
            "incr_fold_probe",
            [
                "--file",
                db_file,
                "--from",
                str(from_sesno),
                "--to",
                str(to_sesno),
                "--dbnum",
                str(dbnum),
            ],
        )

    def run_l3_driver(
        self,
        macro: Path,
        *,
        project_dir: Path,
        e3d_install: Path,
        e3d_project: str = "AMS",
        e3d_mdb: str = "/ALL",
        timeout: float = 300.0,
    ) -> ToolResult:
        return self.run(
            "l3_suite",
            [
                "--check-driver",
                macro,
                "--project-dir",
                project_dir,
                "--e3d-project",
                e3d_project,
                "--e3d-mdb",
                e3d_mdb,
            ],
            cwd=self.bin_dir or self.repo,
            env={
                "L3_E3D_DRIVER": str(
                    (self.repo / "scripts" / "e3d" / "run_ams_c_entrymacro.bat").resolve()
                ),
                "L3_E3D_INSTALL_DIR": str(Path(e3d_install).resolve()),
            },
            timeout=timeout,
        )

    @contextmanager
    def service(
        self,
        client: GenModelClient,
        *,
        cwd: Path,
        log_dir: Path,
        env: Mapping[str, str] | None = None,
        startup_timeout: float = 90.0,
    ) -> Iterator[subprocess.Popen[str]]:
        log_dir.mkdir(parents=True, exist_ok=True)
        stdout_path = log_dir / "aios-database.stdout.log"
        stderr_path = log_dir / "aios-database.stderr.log"
        merged_env = os.environ.copy()
        if env:
            merged_env.update(env)
        with stdout_path.open("w", encoding="utf-8") as stdout, stderr_path.open(
            "w", encoding="utf-8"
        ) as stderr:
            process = subprocess.Popen(
                [str(self.executable("aios-database"))],
                cwd=str(cwd),
                env=merged_env,
                stdout=stdout,
                stderr=stderr,
                text=True,
            )
            try:
                client.wait_for_health(timeout=startup_timeout)
                yield process
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=15)
