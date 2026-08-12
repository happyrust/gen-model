# -*- coding: utf-8 -*-
"""绑定测试的进程级编排（两条轨：离线档 + 房间增量档）。

一个 pytest 进程 = 一个 aios_db 绑定实例 = 一次 full_init：配置是进程级
OnceCell、SUL_DB 是全局连接，所以 SurrealDB 实例与执行层初始化都做成 session
夹具；夹具数据（合成房间）做成 function 夹具，每条用例先建后清。

两条轨：

- **离线档**（`-m offline`）：只吃仓内 issue-019 夹具与打桩 HTTP 服务，不连
  SurrealDB、不碰项目目录、不做任何初始化——CI 唯一能跑的一档。
- **房间增量档**（默认全跑）：对 conftest 自起的一次性内存 SurrealDB 做
  「增量 == 全量」逐边对拍，需要 `bin/surreal.exe` 与 testbed 项目副本。

隔离与残留纪律：
- SurrealDB 用 bin/surreal.exe（fork 2.1.4）的一次性内存实例 @8071，进程退出
  即全部丢弃；8009 / 8019 / 9099 一概不碰。端口被占直接 skip 并说明。
- full_init 的 load_project_tree_verified / 空间树落盘会在仓库根写
  accel_tree_{project}.bin(.meta.json)——与真实项目树文件同名。session 开始前
  把已存在的挪到 .bak-roomtest，结束后删掉测试产物并挪回来，谁的都不毁。
- full_init 拿 testbed 项目副本的单实例锁：测试期间勿并行跑 testbed 脚本。
"""

from __future__ import annotations

import socket
import subprocess
import sys
import time
from pathlib import Path

import pytest

TESTS = Path(__file__).resolve().parent
REPO_ROOT = TESTS.parents[1]
CONFIG = TESTS / "DbOption-roomtest"
CI_CONFIG = TESTS / "DbOption-ci"
SURREAL_EXE = REPO_ROOT / "bin" / "surreal.exe"
SURREAL_PORT = 8071
MESH_DIR = TESTS / ".meshes"

# aios_client.py 是 python/ 下的单文件模块，不在包里；pytest 只会把用例自己的
# 目录（python/tests）放进 sys.path。
sys.path.insert(0, str(TESTS.parent))

# 本进程认定的那一份配置，由 pytest_collection_modifyitems 按选中集合裁定。
_SESSION_CONFIG: Path = CI_CONFIG


def pytest_collection_modifyitems(config, items):
    """裁定本进程唯一的一份 DbOption（OnceCell，换库只能换进程）。

    只要选中集合里有非 offline 用例，就必须用 roomtest 配置——它要连 8071 并按
    `room_key_word` 圈住夹具房。离线用例在任何一份合法配置下都成立（纯文件解析
    只读全局 debug 选项），所以让给房间档，避免两轨互斥、非要分两次跑。
    """
    global _SESSION_CONFIG
    _SESSION_CONFIG = (
        CONFIG if any("offline" not in item.keywords for item in items) else CI_CONFIG
    )


@pytest.fixture(scope="session")
def configured():
    """进程内唯一一次 `set_config`，返回 `aios_db` 模块（不连库、不初始化）。"""
    import aios_db

    aios_db.set_config(str(_SESSION_CONFIG))
    return aios_db

# 与 DbOption-roomtest.toml 的 project_name 对应的空间树文件（写在仓库根 = cwd）。
TREE_ARTIFACTS = [
    REPO_ROOT / "accel_tree_AvevaMarineSample.bin",
    REPO_ROOT / "accel_tree_AvevaMarineSample.meta.json",
]
BACKUP_SUFFIX = ".bak-roomtest"


def _port_in_use(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(0.5)
        return probe.connect_ex(("127.0.0.1", port)) == 0


def _wait_port(port: int, timeout: float = 30.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if _port_in_use(port):
            return True
        time.sleep(0.2)
    return False


@pytest.fixture(scope="session")
def surreal():
    """一次性内存 SurrealDB @8071，session 结束杀进程（内存后端零残留）。"""
    if not SURREAL_EXE.exists():
        pytest.skip(f"缺 {SURREAL_EXE}（仓库自带 fork 2.1.4 服务端）")
    if _port_in_use(SURREAL_PORT):
        pytest.skip(
            f"127.0.0.1:{SURREAL_PORT} 已被占用——先停掉占用者（可能是上次没清理的"
            " surreal，或有人手动起了 8071）"
        )
    log_path = TESTS / ".surreal-roomtest.log"
    with open(log_path, "w", encoding="utf-8") as log:
        proc = subprocess.Popen(
            [
                str(SURREAL_EXE),
                "start",
                "--user", "root",
                "--pass", "root",
                "--bind", f"127.0.0.1:{SURREAL_PORT}",
                "memory",
            ],
            cwd=str(REPO_ROOT),
            stdout=log,
            stderr=subprocess.STDOUT,
        )
    if not _wait_port(SURREAL_PORT):
        proc.kill()
        proc.wait()
        pytest.skip(f"SurrealDB 没能在 30s 内监听 {SURREAL_PORT}，日志见 {log_path}")
    yield proc
    proc.kill()
    proc.wait()


def _shelve_existing_tree_files() -> list[tuple[Path, Path]]:
    shelved = []
    for artifact in TREE_ARTIFACTS:
        if artifact.exists():
            backup = artifact.with_name(artifact.name + BACKUP_SUFFIX)
            backup.unlink(missing_ok=True)
            artifact.rename(backup)
            shelved.append((backup, artifact))
    return shelved


def _restore_tree_files(shelved: list[tuple[Path, Path]]) -> None:
    # 先删测试期间写出的产物（对着内存库的指纹，对真库毫无意义），再把原件挪回。
    for artifact in TREE_ARTIFACTS:
        artifact.unlink(missing_ok=True)
    for backup, original in shelved:
        backup.rename(original)


@pytest.fixture(scope="session")
def binding(surreal, configured):
    """full_init 一次（配置由 `configured` 钉死，本档必须是 roomtest 那份）。"""
    if _SESSION_CONFIG != CONFIG:
        pytest.skip(f"本进程的配置是 {_SESSION_CONFIG.name}，房间档要 {CONFIG.name}")
    MESH_DIR.mkdir(exist_ok=True)
    shelved = _shelve_existing_tree_files()
    try:
        # force=True：沙箱与在跑服务只是**工程重名**，不是真冲突——库是本
        # conftest 自起的 8071 内存实例、锁在 testbed 项目副本根、mesh 在
        # tests/.meshes，三条资源全独立。而 full_init 的活服务探测只能按
        # /health 的 project 字段比对（响应里没有「它连的是哪个 SurrealDB」），
        # 同名即判冲突，对沙箱是误伤，所以这里显式跳过。
        configured.full_init(cwd=str(REPO_ROOT), force=True)
        yield configured
    finally:
        _restore_tree_files(shelved)


@pytest.fixture()
def room_fixture(binding):
    """每条用例一套干净夹具：建（内部先 drop，幂等）→ 用 → 清。

    返回 fixture.refnos() 的清单：{room_num, pane_a, pane_b, in_a, in_b,
    straddler, seqs}。注意 GLOBAL_AABB_TREE 是进程级的——基线必须对全部夹具
    refno 跑一次 update_aabbs(replace=True)，把上一条用例可能留下的旧树条目
    同步掉（与 Rust 夹具 fixture_baseline 同一前提）。
    """
    binding.fixture.create()
    try:
        yield binding.fixture.refnos()
    finally:
        binding.fixture.drop()
