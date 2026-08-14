# -*- coding: utf-8 -*-
"""`src/bin/db_session_fixture/session_cut.rs` 的 Python 逐字节镜像。

按 sesno 从 append-only dabacon 文件切出历史时刻的完整快照：文件头 offset 40 存
最新 session page 号；每个 session page 在 `+4` 记 previous（上一 session page）、
`+12` 记 sesno、`+20` 记 latest_page（该会话结束时文件最后一页）。沿链回溯枚举全部
会话；把文件截断到 `(latest_page + 1)` 页、并把头指针改回该会话的 session page，
就得到该会话时刻的完整文件。

**Rust 是权威实现，本模块只是镜像**——存在只为让 `test_net_window_ab.py` 能在
Python 侧造「窗口起点 K 时刻的存量库」（T11b）。正确性由两道对拍钉死：
  1) 合成文件不变量单测（与 `session_cut.rs::synthetic_two_sessions` 同构，离线常跑）；
  2) 真实文件上与 Rust `db_session_fixture inspect` 的会话链逐条对齐、并对现切结果
     再 `inspect` 回读确认 latest==K（live 用例里做）。

任何一处 offset / 截断规则与 Rust 漂移，(1) 立刻红。
"""

from __future__ import annotations

import hashlib
import json
import struct
import subprocess
from pathlib import Path

# 与 session_cut.rs 常量逐字对齐。
PAGE_SIZE = 0x800
HEADER_SESSION_PAGE_OFFSET = 40
_TERMINATORS = (0, 0xFFFFFFFF)


def _be_u32(data: bytes, offset: int) -> int:
    return struct.unpack_from(">I", data, offset)[0]


def session_chain(data: bytes) -> tuple[int, dict[int, tuple[int, int]]]:
    """沿头指针回溯 session page 链。

    返回 `(latest_sesno, {sesno: (session_page, latest_page)})`——与 Rust
    `SessionChain{latest_sesno, cuts}` 同构。文件不合规（不足一页 / 未按页对齐 /
    指针越界 / 重复 sesno / 头指针空）一律抛，绝不给静默错答案。
    """
    if len(data) < PAGE_SIZE:
        raise ValueError(f"PDMS 文件不足一页：{len(data)}")
    if len(data) % PAGE_SIZE:
        raise ValueError(f"PDMS 文件大小未按页对齐：{len(data)}")

    page = _be_u32(data, HEADER_SESSION_PAGE_OFFSET)
    cuts: dict[int, tuple[int, int]] = {}
    latest_sesno: int | None = None
    seen: set[int] = set()
    while page not in _TERMINATORS and page not in seen:
        seen.add(page)
        start = page * PAGE_SIZE
        if start + PAGE_SIZE > len(data):
            raise ValueError(f"session page {page} 超出文件范围")
        previous = _be_u32(data, start + 4)
        sesno = _be_u32(data, start + 12)
        latest_page = _be_u32(data, start + 20)
        if sesno in cuts:
            raise ValueError(f"会话链里出现重复 sesno={sesno}（page {page}）")
        cuts[sesno] = (page, latest_page)
        if latest_sesno is None:
            latest_sesno = sesno
        page = previous

    if latest_sesno is None:
        raise ValueError("头指针没有指向任何 session page")
    return latest_sesno, cuts


def cut_bytes(source: bytes, sesno: int) -> bytes:
    """把 `source` 截断为 `sesno` 时刻的快照字节（头指针一并回写），不落盘。"""
    _, cuts = session_chain(source)
    if sesno not in cuts:
        raise ValueError(f"会话链里没有 sesno={sesno}")
    session_page, latest_page = cuts[sesno]
    end = (latest_page + 1) * PAGE_SIZE
    if end > len(source):
        raise ValueError(f"快照截断点 {end} 超出源文件大小 {len(source)}")
    snapshot = bytearray(source[:end])
    struct.pack_into(">I", snapshot, HEADER_SESSION_PAGE_OFFSET, session_page)
    return bytes(snapshot)


def write_cut(source_path: Path, sesno: int, out_path: Path) -> str:
    """从 `source_path` 切 `sesno` 快照写到 `out_path`，返回其 SHA256。"""
    snapshot = cut_bytes(Path(source_path).read_bytes(), sesno)
    Path(out_path).write_bytes(snapshot)
    return sha256_bytes(snapshot)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(Path(path).read_bytes())


def rust_inspect_chain(fixture_exe: Path, source_path: Path) -> tuple[int, list[int]]:
    """调 Rust 权威 `db_session_fixture inspect`（JSON stdout），返回 (latest_sesno, sesnos)。

    这是「Python 镜像 == Rust 权威」在**真实文件**上的对拍腿：inspect 走的正是
    阶段一切割用的同一份 `session_cut::session_chain`。
    """
    completed = subprocess.run(
        [str(fixture_exe), "inspect", "--source", str(source_path)],
        capture_output=True,
        text=True,
        check=True,
    )
    report = json.loads(completed.stdout)
    return int(report["latest_sesno"]), [int(s) for s in report["sesnos"]]
