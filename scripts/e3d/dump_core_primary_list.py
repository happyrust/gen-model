"""Snapshot core.dll's authoritative DB_Noun::primaryList field from live E3D.

The target process must already have a project/MDB open. Unknown field reads stay
explicit in the output; production treats only those unknown nouns conservatively.
"""

import argparse
import hashlib
import json
import pathlib
import sys

import frida


ROOT = pathlib.Path(__file__).resolve().parents[2]
FIELD_ID = 297853135
AGENT_SOURCE = r"""
const getInfoPtr = Module.getGlobalExportByName('db_get_element_info');
const clearErrorPtr = Module.getGlobalExportByName('db_clear_error');
const getInfo = new NativeFunction(getInfoPtr, 'int', ['int', 'int', 'pointer']);
const clearError = new NativeFunction(clearErrorPtr, 'void', []);
rpc.exports = {
  dump: function (rows, fieldId) {
    const out = Memory.alloc(4);
    return rows.map(function (row) {
      out.writeS32(0);
      const ok = getInfo(row.hash, fieldId, out);
      const value = out.readS32();
      if (!ok) clearError();
      return { hash: row.hash, noun: row.noun, ok: ok !== 0, value: value };
    });
  },
  moduleInfo: function () {
    const module = Process.getModuleByName('core.dll');
    return { path: module.path, base: module.base.toString(), size: module.size };
  }
};
"""


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def noun_rows(nouns: list[dict], only: set[str] | None = None) -> list[dict]:
    rows = [
        {"hash": int(item["noun_hash"]), "noun": str(item["noun_name"]).strip().upper()}
        for item in nouns
        if str(item.get("noun_name") or "").strip()
        and (only is None or str(item["noun_name"]).strip().upper() in only)
    ]
    names = [row["noun"] for row in rows]
    if len(names) != len(set(names)):
        raise ValueError("noun_flags contains duplicate normalized noun names")
    return rows


def collect_live(pid: int, rows: list[dict]) -> tuple[pathlib.Path, list[dict]]:
    session = frida.attach(pid)
    script = session.create_script(AGENT_SOURCE)
    script.on("message", lambda message, data: print("frida", message, file=sys.stderr))
    script.load()
    try:
        print("rpc_exports", script.list_exports_sync(), file=sys.stderr)
        module_info = script.exports_sync.module_info()
        results: list[dict] = []
        for offset in range(0, len(rows), 128):
            results.extend(script.exports_sync.dump(rows[offset : offset + 128], FIELD_ID))
    finally:
        session.detach()
    return pathlib.Path(module_info["path"]), results


def build_payload(
    results: list[dict], core_path: pathlib.Path, nouns_path: pathlib.Path
) -> dict:
    success = [row for row in results if row["ok"]]
    failures = [row for row in results if not row["ok"]]
    non_binary = [row for row in success if row["value"] not in (0, 1)]
    return {
        "schema": 1,
        "source": "core.dll!db_get_element_info(noun_hash, 297853135)",
        "field_id": FIELD_ID,
        "core_file": core_path.name,
        "core_file_bytes": core_path.stat().st_size,
        "core_sha256": sha256(core_path),
        "noun_source": nouns_path.name,
        "noun_source_bytes": nouns_path.stat().st_size,
        "noun_source_sha256": sha256(nouns_path),
        "count": len(results),
        "resolved_count": len(success),
        "unknown_count": len(failures),
        "true_count": sum(row["value"] == 1 for row in success),
        "false_count": sum(row["value"] != 1 for row in success),
        "non_binary_values": [
            {"noun": row["noun"], "hash": row["hash"], "value": row["value"]}
            for row in non_binary
        ],
        "unknown": [
            {"noun": row["noun"], "hash": row["hash"]} for row in failures
        ],
        "nouns": {row["noun"]: row["value"] == 1 for row in success},
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pid", required=True, type=int, help="live des.exe process id")
    parser.add_argument(
        "--out",
        default=str(ROOT / "tests/fixtures/core-primary-list-e3d31.json"),
        help="snapshot output path",
    )
    parser.add_argument("--noun-flags", default=str(ROOT / "noun_flags.json"))
    parser.add_argument("--only", help="optional comma-separated noun smoke subset")
    args = parser.parse_args()

    out_path = pathlib.Path(args.out).resolve()
    nouns_path = pathlib.Path(args.noun_flags).resolve()
    nouns = json.loads(nouns_path.read_text(encoding="utf-8"))
    only = {name.strip().upper() for name in args.only.split(",")} if args.only else None
    rows = noun_rows(nouns, only)
    core_path, results = collect_live(args.pid, rows)
    payload = build_payload(results, core_path, nouns_path)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(
        json.dumps(
            {
                key: payload[key]
                for key in (
                    "core_sha256",
                    "noun_source_sha256",
                    "count",
                    "resolved_count",
                    "unknown_count",
                    "true_count",
                    "false_count",
                )
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
