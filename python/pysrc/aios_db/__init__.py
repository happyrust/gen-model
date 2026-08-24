# -*- coding: utf-8 -*-
"""aios-database 调试绑定（设计见 docs/plans/2026-08-11-python-binding-api-plan.md）。

导入扩展模块之前把原生运行时目录注册进 DLL 搜索路径：Python 3.8 起加载扩展模块
不再查 PATH，而主程序 exe 走的是 legacy PATH 搜索。这里把 PATH 上所有存在的目录
整体注册，让绑定的 DLL 搜索面与 exe 一致。
"""

import os


def _add_native_dll_dirs() -> None:
    seen = set()
    for entry in os.environ.get("PATH", "").split(os.pathsep):
        entry = entry.strip().strip('"')
        if not entry or entry in seen:
            continue
        seen.add(entry)
        try:
            if os.path.isdir(entry):
                os.add_dll_directory(entry)
        except OSError:
            continue
    # 注册失败不在这里中断——让下面的 import 给出准确的加载错误。


_add_native_dll_dirs()

from ._aios_db import (  # noqa: E402
    connect,
    db,
    fixture,
    full_init,
    incr,
    model,
    parse,
    room,
    set_config,
    spatial,
    sync,
)

__all__ = [
    "connect",
    "db",
    "fixture",
    "full_init",
    "incr",
    "model",
    "parse",
    "room",
    "set_config",
    "spatial",
    "sync",
]
