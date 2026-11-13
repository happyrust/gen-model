"""
DB_UserChanges 运行时事件采集器（P79–P85 的事件级证据来源）。

背景：`ams-inplace-manifest.json` 里 P79–P85 的 `blocked_reason` 一律含
`requires-db-userchanges-event-observation`。RVM 只能给最终态，created 祖先去重、
`ATT_MEMB` 与 `elementReordered` 的分流、qualifier 清空后的 comb key、`combine` 的
合并次数都必须看 core.dll `DB_UserChanges` 的**写入调用**。

做法：只读 hook。`Interceptor.attach` 到 core.dll 已导出的
`DB_UserChanges::elementCreated/elementDeleted/elementIncluded/elementReordered/
attributeModified(×2 重载)/combine`，把每次调用的 (事件, refno, 附加 refno/属性指针)
写成 jsonl。**不调用目标函数、不替换实现、不改返回值、不写进程内存**——与
`E:\\reverse\\e3d\\e3d-license-hook\\m66_hook_pmlcommand.py` 同一纪律。

DB_Element 的 refno 偏移来自 `.ida_scratch/analysis/db_userchanges.c` 的
`elementCreated`：函数体末尾 `*((_DWORD *)this + 15) = *((_DWORD *)a2 + 3);
*((_DWORD *)this + 16) = *((_DWORD *)a2 + 4);` 说明 `DB_Element` 的 +0xC / +0x10
就是 refno 的两个 32 位字（PDMS refno 是 (word0, word1)）。

用法（E3D TTY 跑案例时并行起一个）：

    python scripts/e3d/increment_suite/tools/dc_events_probe.py \
        --out output/dc-events/<run-id>.jsonl --timeout 600

进程名默认 `des.exe`，模块名默认 `core.dll`；`--pid` 可直接指定。
退出码 0 = 至少抓到一条事件；2 = 连上但零事件；1 = 连不上进程。
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any

import frida

# 导出名用 MSVC mangled 全名：core.dll 的导出表在磁盘上是混淆过的（直接解析
# AddressOfNames 得到的是代码字节），交给 loader 自己的解析器才可靠。
HOOKS = [
    ("elementCreated", "?elementCreated@DB_UserChanges@@QAEXABVDB_Element@@@Z", 1),
    ("elementDeleted", "?elementDeleted@DB_UserChanges@@QAEXABVDB_Element@@@Z", 1),
    ("elementIncluded", "?elementIncluded@DB_UserChanges@@QAEXABVDB_Element@@0@Z", 2),
    ("elementReordered", "?elementReordered@DB_UserChanges@@QAEXABVDB_Element@@@Z", 1),
    (
        "attributeModified",
        "?attributeModified@DB_UserChanges@@QAEXABVDB_Element@@PBVDB_Attribute@@@Z",
        1,
    ),
    (
        "attributeModifiedQual",
        "?attributeModified@DB_UserChanges@@QAEXABVDB_Element@@PBVDB_Attribute@@ABVDB_Qualifier@@@Z",
        1,
    ),
    ("combine", "?combine@DB_UserChanges@@QAEXABV1@@Z", 0),
]

SCRIPT = r"""
'use strict';

const HOOKS = %(hooks)s;

// 这是 32 位 MSVC 的 __thiscall：`this` 在 ecx，不在栈参数里。
// `onEnter(args)` 的 args[i] 是**栈上**第 i 个参数（args[0] 是 [esp+4]），
// 所以第一个显式参数（DB_Element*）是 args[0]，不是 args[1]。
// `this` 要自己从 context.ecx 读。
function readRef(p) {
  if (p === null || p.isNull()) return null;
  try {
    const w0 = p.add(0x0c).readU32();
    const w1 = p.add(0x10).readU32();
    if (w0 === 0 && w1 === 0) return null;
    return w0.toString() + '/' + w1.toString();
  } catch (e) {
    return null;
  }
}

function ptrId(p) {
  if (p === null || p.isNull()) return null;
  try { return p.toString(); } catch (e) { return null; }
}

// 属性名走 DB_Attribute 的 const getter：
//   ?name@DB_Attribute@@QBEABV?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@XZ
// 返回 std::string const&。x86 MSVC std::string = 24 字节：
//   +0x00 _Bx（16B；capacity>=16 时前 4 字节是 char*，否则是内联缓冲）
//   +0x10 _Mysize, +0x14 _Myres
// 这是只读 getter，不改变 DB 状态。
let attrNameFn = null;

function readStdString(p) {
  if (p === null || p.isNull()) return null;
  try {
    const size = p.add(0x10).readU32();
    const res = p.add(0x14).readU32();
    if (size === 0 || size > 256) return null;
    const dataPtr = res >= 16 ? p.readPointer() : p;
    return dataPtr.readUtf8String(size);
  } catch (e) {
    return null;
  }
}

function attrName(attrPtr) {
  if (attrPtr === null || attrPtr.isNull()) return null;
  if (attrNameFn === null) {
    try {
      attrNameFn = new NativeFunction(
        core.getExportByName('?name@DB_Attribute@@QBEABV?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@XZ'),
        'pointer', ['pointer']);
    } catch (e) { attrNameFn = false; }
  }
  if (attrNameFn === false) return null;
  try { return readStdString(attrNameFn(attrPtr)); } catch (e) { return null; }
}

function emit(rec) {
  rec.t = Date.now();
  send(rec);
}

let core = null;
function resolveAll() {
  if (core === null) {
    try { core = Process.getModuleByName('core.dll'); } catch (e) { return false; }
  }
  return true;
}

const installed = {};

function install() {
  if (!resolveAll()) return false;
  let ok = 0;
  const missing = [];
  for (const h of HOOKS) {
    if (installed[h[1]]) { ok++; continue; }
    let addr;
    try { addr = core.getExportByName(h[1]); } catch (e) { missing.push(h[0]); continue; }
    try {
      Interceptor.attach(addr, {
        onEnter(args) {
          const rec = {
            event: h[0],
            elem: readRef(args[0]),
            elemPtr: ptrId(args[0])
          };
          if (h[2] >= 2) rec.elem2 = readRef(args[1]);
          if (h[0].indexOf('attributeModified') === 0) {
            rec.attrPtr = ptrId(args[1]);
            rec.attr = attrName(args[1]);
            rec.qualified = (h[0] === 'attributeModifiedQual');
          }
          emit(rec);
        }
      });
      installed[h[1]] = true;
      ok++;
    } catch (e) { missing.push(h[0] + ':' + e.message); }
  }
  send({ event: '_installed', count: ok, total: HOOKS.length,
         base: core.base.toString(), missing: missing });
  return ok > 0;
}

if (!install()) {
  const iv = setInterval(() => { if (install()) clearInterval(iv); }, 250);
}
"""


def find_pid(process_name: str, timeout: float) -> int | None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            for proc in frida.enumerate_processes():
                if proc.name.lower() == process_name.lower():
                    return proc.pid
        except Exception:
            pass
        time.sleep(0.5)
    return None


def main() -> int:
    ap = argparse.ArgumentParser(description="DB_UserChanges 只读事件采集器")
    ap.add_argument("--out", required=True, help="jsonl 输出路径")
    ap.add_argument("--process", default="des.exe")
    ap.add_argument("--pid", type=int, default=0)
    ap.add_argument("--module", default="core.dll")
    ap.add_argument("--timeout", type=float, default=900.0, help="等进程/等事件的秒数")
    args = ap.parse_args()

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)

    pid = args.pid or find_pid(args.process, min(args.timeout, 120.0))
    if not pid:
        print(f"[dc-events] 没等到进程 {args.process}", file=sys.stderr)
        return 1

    session = frida.attach(pid)
    hooks_js = json.dumps([[h[0], h[1], h[2]] for h in HOOKS])
    script = session.create_script(SCRIPT % {"hooks": hooks_js})

    counts: dict[str, int] = {}

    def on_message(message: dict[str, Any], data: Any) -> None:
        if message.get("type") != "send":
            return
        payload = message.get("payload") or {}
        event = str(payload.get("event") or "")
        counts[event] = counts.get(event, 0) + 1
        with out.open("a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")

    script.on("message", on_message)
    script.load()

    deadline = time.time() + args.timeout
    while time.time() < deadline:
        if any(k for k in counts if not k.startswith("_")):
            # 抓到事件后再多等 5 秒收尾（同一 SAVEWORK 内的后续调用）
            time.sleep(5.0)
            break
        time.sleep(0.5)

    try:
        script.unload()
    except Exception:
        pass
    session.detach()

    real = {k: v for k, v in counts.items() if not k.startswith("_")}
    print("[dc-events] " + json.dumps(real, ensure_ascii=False), file=sys.stderr)
    if not real:
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
