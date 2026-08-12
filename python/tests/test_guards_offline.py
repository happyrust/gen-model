# -*- coding: utf-8 -*-
"""三层硬守护的离线用例：未初始化就调用必须响亮报错。

守护状态（`CONNECTED` / `FULL_INIT`）是**进程级** AtomicBool，一旦本进程连过库
就再也回不到「未初始化」态——所以整档在一个干净子解释器里跑，而不是靠 fixture
复位。子进程只 import、不连库、不读配置（`ensure_connected` / `ensure_full` 是
纯原子位检查，先于任何配置读取），所以这一档在 CI 上零依赖。

一次子进程跑完全部用例（导入 80MB 扩展要秒级，逐条起进程不划算），结果按名字
回填给参数化断言。
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = pytest.mark.offline

REPO_ROOT = Path(__file__).resolve().parents[2]

# 子进程驱动：逐个调用，记下「拦没拦住、拦的话说了什么」。刻意不 set_config——
# 守护若真的先于配置读取生效（设计如此），这里就不该需要任何配置。
_DRIVER = r"""
import json
import aios_db

CASES = [
    # 执行层（未 full_init 必须拒绝）
    ("incr.apply_file", lambda: aios_db.incr.apply_file("no-such-file")),
    ("incr.execute_manual", lambda: aios_db.incr.execute_manual()),
    ("incr.drain_data", lambda: aios_db.incr.drain_data()),
    ("incr.drain_side_effects", lambda: aios_db.incr.drain_side_effects()),
    ("incr.queue_pause", lambda: aios_db.incr.queue_pause()),
    ("incr.queue_resume", lambda: aios_db.incr.queue_resume()),
    ("model.ensure", lambda: aios_db.model.ensure("1_1")),
    ("model.gen", lambda: aios_db.model.gen(["1_1"])),
    ("model.gen_dbnum", lambda: aios_db.model.gen_dbnum(8000)),
    ("model.update_aabbs", lambda: aios_db.model.update_aabbs(["1_1"])),
    ("model.delete_subtree", lambda: aios_db.model.delete_subtree(["1_1"])),
    ("room.build_all", lambda: aios_db.room.build_all()),
    ("room.drain", lambda: aios_db.room.drain()),
    ("room.enqueue", lambda: aios_db.room.enqueue([])),
    ("spatial.reconcile", lambda: aios_db.spatial.reconcile()),
    ("spatial.persist", lambda: aios_db.spatial.persist()),
    ("spatial.rebuild", lambda: aios_db.spatial.rebuild()),
    ("sync.baseline", lambda: aios_db.sync.baseline(8000)),
    ("fixture.create", lambda: aios_db.fixture.create()),
    ("fixture.drop", lambda: aios_db.fixture.drop()),
    ("fixture.move_body", lambda: aios_db.fixture.move_body(1, [0, 0, 0], [1, 1, 1])),
    # 连接层（未 connect 必须拒绝）
    ("db.query", lambda: aios_db.db.query("INFO FOR DB;")),
    ("db.by_name", lambda: aios_db.db.by_name("/X")),
    ("db.pe", lambda: aios_db.db.pe("1_1")),
    ("db.inst", lambda: aios_db.db.inst("1_1")),
    ("db.watermark", lambda: aios_db.db.watermark(8000)),
    ("db.window_blocks", lambda: aios_db.db.window_blocks()),
    ("incr.resolve_window", lambda: aios_db.incr.resolve_window("no-such-file")),
    ("incr.queue_status", lambda: aios_db.incr.queue_status()),
    ("model.export_obj", lambda: aios_db.model.export_obj("1_1", ".")),
    ("room.code", lambda: aios_db.room.code("1_1")),
    ("spatial.status", lambda: aios_db.spatial.status()),
    ("spatial.tree_status", lambda: aios_db.spatial.tree_status()),
]

out = {}
for name, call in CASES:
    try:
        call()
        out[name] = None                                 # 没拦住
    except RuntimeError as error:
        out[name] = str(error)
    except BaseException as error:                       # 拦了，但类型不对
        out[name] = "!%s: %s" % (type(error).__name__, error)

# 无守护的纯函数：只要能在未初始化态下拿到结果，说明它确实不碰库。
try:
    out["fixture.refnos"] = "ok" if aios_db.fixture.refnos()["room_num"] else "empty"
except BaseException as error:
    out["fixture.refnos"] = "!%s: %s" % (type(error).__name__, error)

print(json.dumps(out, ensure_ascii=False))
"""

FULL_INIT_GUARDED = [
    "incr.apply_file",
    "incr.execute_manual",
    "incr.drain_data",
    "incr.drain_side_effects",
    "incr.queue_pause",
    "incr.queue_resume",
    "model.ensure",
    "model.gen",
    "model.gen_dbnum",
    "model.update_aabbs",
    "model.delete_subtree",
    "room.build_all",
    "room.drain",
    "room.enqueue",
    "spatial.reconcile",
    "spatial.persist",
    "spatial.rebuild",
    "sync.baseline",
    "fixture.create",
    "fixture.drop",
    "fixture.move_body",
]

CONNECT_GUARDED = [
    "db.query",
    "db.by_name",
    "db.pe",
    "db.inst",
    "db.watermark",
    "db.window_blocks",
    "incr.resolve_window",
    "incr.queue_status",
    "model.export_obj",
    "room.code",
    "spatial.status",
    "spatial.tree_status",
]


@pytest.fixture(scope="session")
def guard_messages() -> dict[str, str | None]:
    env = dict(os.environ, PYTHONUTF8="1", PYTHONIOENCODING="utf-8")
    proc = subprocess.run(
        [sys.executable, "-c", _DRIVER],
        cwd=str(REPO_ROOT),
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=300,
    )
    if proc.returncode != 0:
        pytest.fail(f"守护探针子进程退出码 {proc.returncode}\nstderr:\n{proc.stderr}")
    return json.loads(proc.stdout.strip().splitlines()[-1])


@pytest.mark.parametrize("name", FULL_INIT_GUARDED)
def test_mutating_entry_requires_full_init(guard_messages, name):
    message = guard_messages[name]
    assert message is not None, f"{name} 未 full_init 居然放行了"
    assert not message.startswith("!"), f"{name} 抛的不是 RuntimeError: {message}"
    assert "full_init" in message, f"{name} 的报错没指出修法: {message}"


@pytest.mark.parametrize("name", CONNECT_GUARDED)
def test_readonly_entry_requires_connect(guard_messages, name):
    message = guard_messages[name]
    assert message is not None, f"{name} 未 connect 居然放行了"
    assert not message.startswith("!"), f"{name} 抛的不是 RuntimeError: {message}"
    assert "connect" in message, f"{name} 的报错没指出修法: {message}"


def test_pure_entry_needs_no_init(guard_messages):
    """`fixture.refnos` 是纯常量清单，不该被任何一道门挡住。"""
    assert guard_messages["fixture.refnos"] == "ok"


def test_every_guarded_entry_is_covered(guard_messages):
    """新增 mutating 入口忘了加守护时，这条会连同参数化一起漏——所以顺带钉住
    覆盖面本身：探针跑过的名字必须与两张清单严格对齐。"""
    probed = set(guard_messages) - {"fixture.refnos"}
    assert probed == set(FULL_INIT_GUARDED) | set(CONNECT_GUARDED)
