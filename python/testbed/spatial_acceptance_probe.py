#!/usr/bin/env python
"""空间树一致性闭环——沙箱验收探针（V2 快照生命周期 + 崩溃注入烟测）。

方案 docs/plans/2026-08-12-spatial-tree-consistency-closure-plan.md §8 的沙箱可测子集，
每个场景一个子进程 = 一次「服务重启」：

  A 无快照首启      -> 指针重建 + 发布 V2（verdict=rebuilt，entries==usable）
  B 正常重启        -> V2 快路径复用（verdict=reused，SHA 未变、无漂移）
  C 截断快照        -> 校验失败自动重建（verdict=rebuilt，条目收敛）
  D 删除快照        -> 自动重建（快照重新在场）
  E 注入 rename 前崩溃（AIOS_FAILPOINT=spatial_snapshot_tmp_written）
                    -> 子进程 abort（非零退出），正式快照不出现
  F 崩溃后重启      -> 自动重建收敛（与基线同一棵规范化树）

树文件落在隔离 cwd（testbed/out/spatial-acceptance），不碰仓库根的生产工件。
E3D 侧场景（TTY 复制恢复对拍、伪造旧 epoch、房间边对拍）见验收 runbook：
docs/2026-08-12_spatial-tree-consistency-acceptance.md。

前置：
  * python/testbed/Start-TestSurreal.ps1 已起 8019，且 7997 基线在位
    （python/testbed/run_full_loop.py 首跑完成）；
  * maturin develop 已安装最新绑定。

用法：
  .venv\\Scripts\\python.exe testbed\\spatial_acceptance_probe.py
"""

import json
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent  # python/testbed
PY_DIR = HERE.parent  # python/
CONFIG = HERE / "DbOption-pytest"
WORK = HERE / "out" / "spatial-acceptance"  # 隔离 cwd：树文件只落这里
SNAPSHOT = WORK / "accel_tree_AvevaMarineSample.snapshot"

CHILD_CODE = r"""
import json, sys
import aios_db
aios_db.set_config(sys.argv[1])
aios_db.full_init(cwd=sys.argv[2], force=True)
status = aios_db.spatial.tree_status()
print("STATUS::" + json.dumps(status, ensure_ascii=False))
"""


def run_child(env_extra=None):
    env = os.environ.copy()
    env.pop("AIOS_FAILPOINT", None)
    env["PYTHONIOENCODING"] = "utf-8"
    if env_extra:
        env.update(env_extra)
    started = time.time()
    proc = subprocess.run(
        [sys.executable, "-c", CHILD_CODE, str(CONFIG), str(WORK)],
        cwd=str(PY_DIR),
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=600,
    )
    elapsed = time.time() - started
    status = None
    for line in (proc.stdout or "").splitlines():
        if line.startswith("STATUS::"):
            status = json.loads(line[len("STATUS::") :])
    return proc, status, elapsed


def expect(cond, message, proc=None):
    if not cond:
        print(f"[FAIL] {message}")
        if proc is not None:
            print("--- stdout（尾部） ---")
            print((proc.stdout or "")[-4000:])
            print("--- stderr（尾部） ---")
            print((proc.stderr or "")[-4000:])
        sys.exit(1)
    print(f"[ok] {message}")


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    for stale in WORK.glob("accel_tree*"):
        stale.unlink()

    # A 无快照首启：指针重建 + 发布 V2。
    proc, status, elapsed = run_child()
    expect(proc.returncode == 0 and status is not None, f"A 首启完成（{elapsed:.1f}s）", proc)
    expect(
        status["startup_verdict"] == "rebuilt",
        f"A verdict=rebuilt（实得 {status['startup_verdict']}）",
        proc,
    )
    expect(status["ready"] is True, f"A ready（state={status['state']}）", proc)
    expect(
        status["entries"] == status["usable_pointer_rows"],
        f"A entries==usable_pointer_rows（{status['entries']}，invalid {status['invalid_pointer_rows']}）",
        proc,
    )
    expect(
        status["format_version"] == 2 and status["snapshot_sha256"],
        "A 发布 V2（format_version=2 + 哈希在场）",
        proc,
    )
    expect(SNAPSHOT.is_file(), "A 快照文件已落盘", proc)
    baseline_entries = status["entries"]
    baseline_sha = status["snapshot_sha256"]
    print(f"    基线：{baseline_entries} 条，sha256={baseline_sha[:16]}…")

    # B 正常重启：快路径复用。
    proc, status, elapsed = run_child()
    expect(proc.returncode == 0 and status is not None, f"B 重启完成（{elapsed:.1f}s）", proc)
    expect(
        status["startup_verdict"] == "reused",
        f"B verdict=reused（实得 {status['startup_verdict']}）",
        proc,
    )
    expect(status["entries"] == baseline_entries, "B 条目数与首启一致", proc)
    expect(status["snapshot_sha256"] == baseline_sha, "B 快照哈希未变（没有重建重写）", proc)
    expect(status["drift"] is False, "B 指纹无漂移", proc)
    expect(status["pending"] == 0, "B 无待重放空间意图", proc)

    # C 截断快照：校验失败自动重建。
    data = SNAPSHOT.read_bytes()
    SNAPSHOT.write_bytes(data[: len(data) // 2])
    proc, status, elapsed = run_child()
    expect(proc.returncode == 0 and status is not None, f"C 截断后重启完成（{elapsed:.1f}s）", proc)
    expect(
        status["startup_verdict"] == "rebuilt",
        f"C 截断快照必须自动重建（实得 {status['startup_verdict']}）",
        proc,
    )
    expect(status["entries"] == baseline_entries, "C 重建后条目数收敛", proc)

    # D 删除快照：自动重建。
    SNAPSHOT.unlink()
    proc, status, elapsed = run_child()
    expect(proc.returncode == 0 and status is not None, f"D 删除后重启完成（{elapsed:.1f}s）", proc)
    expect(status["startup_verdict"] == "rebuilt", "D 快照缺失必须自动重建", proc)
    expect(SNAPSHOT.is_file(), "D 重建后快照重新在场", proc)

    # E 崩溃注入：.tmp 写完 sync 后、rename 前 abort（崩溃窗口 3）。
    SNAPSHOT.unlink()
    proc, status, elapsed = run_child({"AIOS_FAILPOINT": "spatial_snapshot_tmp_written"})
    expect(
        proc.returncode != 0,
        f"E 注入点必须让子进程非正常退出（实得 {proc.returncode}）",
        proc,
    )
    expect(not SNAPSHOT.is_file(), "E 正式快照不得出现（rename 未发生）", proc)

    # F 崩溃后重启：自动重建收敛到同一规范化集合。
    #
    # 刻意不比载荷字节 SHA：AccelerationTree 序列化含 HashMap 段，迭代顺序随
    # 每进程 SipHash 种子变化，同一集合两次重建的字节不同。tree_sha256 的职责
    # 是**单文件完整性自校验**（写入什么、读出什么），跨进程集合对拍走
    # entries/usable 口径与 Rust 侧 e2e 的逐边比对。
    proc, status, elapsed = run_child()
    expect(proc.returncode == 0 and status is not None, f"F 崩溃后重启完成（{elapsed:.1f}s）", proc)
    expect(status["startup_verdict"] == "rebuilt", "F 重启自动重建收敛", proc)
    expect(
        status["entries"] == baseline_entries
        and status["entries"] == status["usable_pointer_rows"]
        and status["drift"] is False,
        f"F 收敛到同一规范化集合（{status['entries']} 条，无漂移）",
        proc,
    )
    expect(SNAPSHOT.is_file(), "F 快照重新发布在场", proc)

    print("\n全部场景通过。")


if __name__ == "__main__":
    main()
