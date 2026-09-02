"""Snapshot core.dll's authoritative DB_Noun descriptor fields from live E3D.

The target process must already have a project/MDB open. Unknown field reads stay
explicit in the output; production treats only those unknown nouns conservatively.

Reads go through `DB_Noun`'s own exports rather than the `db_get_element_info` C
shim this script used until 2026-08-28. The shim is not a general field reader:
it forwards to core.dll `sub_5B05280`, which switches on exactly five field ids
(642215, 11037101, 243803617, 282170750, 297853135) and sets error 542 for
anything else, so every id outside that set reads back as "unknown" for all 1931
nouns. The exported member functions have no such gate:

    ?findNoun@DB_Noun@@SA_NHAAPBV1@@Z   static bool findNoun(int hash, const DB_Noun*&)
    ?fieldType@DB_Noun@@SAHH@Z          static int  fieldType(int fieldId)
    ?getField@DB_Noun@@QBE_NHAA_N@Z     bool __thiscall getField(int, bool&) const
    ?getField@DB_Noun@@QBE_NHAAH@Z      bool __thiscall getField(int, int&) const

Core takes the **out parameter**, not the return value: the return only says
whether the noun has that field registered at all, and an unregistered field
counts as false. The agent zeroes `out` before every call, exactly as core's own
call sites do (`docs/evidence/2026-08-27-ida-core3d-partial-update-model-impact.md`
section 4). Which overload to use is not a guess either - `fieldType` is core's
own oracle, and the export refuses to run if it disagrees with the requested kind.

One field produces the schema-1 payload this script has always written. Several
fields produce schema 2: the same per-field block, keyed by name, under one shared
core.dll identity. Nothing is combined at export time - `primitive` is
`primitive_a OR primitive_b` in E3D 3.1, but that pairing is version-bound (2.10
pairs `primitive_a` with a third id that does not exist in 3.1), so composing it
here would bake one version's vocabulary into the fixture.
"""

import argparse
import hashlib
import json
import pathlib
import sys

import frida


ROOT = pathlib.Path(__file__).resolve().parents[2]

# Field ids lifted from core's instruction stream; see the evidence document's
# section 4 for the call sites each one was read from.
KNOWN_FIELDS = {
    "primaryList": 297853135,  # 0x11BFB8CF - member-diff gate, ADR-009
    "significant": 90536458,  # 0x5657A0A  - PartialUpdateDesiMgr::IsSignificant
    "primitive_a": 659518,  # 0xA103E    - IsPrimitive, first read; stable across 2.10/3.1
    "primitive_b": 196958940,  # 0xBBD5ADC  - IsPrimitive, fallback read; 3.1 only
    # Registered for lookup, deliberately not part of the granularity export: its
    # only consumer is the m_granularityMode != 0 branch, which is dead code in
    # E3D 3.1 (evidence section 6.1). It is an int field, not a bool.
    "negative": 599651,  # 0x92663
}
GRANULARITY_FIELDS = ("significant", "primitive_a", "primitive_b")
DEFAULT_FIELD = "primaryList"

# core.dll's own `DB_Noun::fieldType` return values, as far as this script needs
# them. Anything else is refused rather than read through a guessed overload.
FIELD_TYPE_BOOL = 0
FIELD_TYPE_INT = 3

AGENT_SOURCE = r"""
const core = Process.getModuleByName('core.dll');
const findNoun = new NativeFunction(
    core.getExportByName('?findNoun@DB_Noun@@SA_NHAAPBV1@@Z'),
    'bool', ['int', 'pointer']);
const fieldType = new NativeFunction(
    core.getExportByName('?fieldType@DB_Noun@@SAHH@Z'),
    'int', ['int']);
const getBool = new NativeFunction(
    core.getExportByName('?getField@DB_Noun@@QBE_NHAA_N@Z'),
    'bool', ['pointer', 'int', 'pointer'], { abi: 'thiscall' });
const getInt = new NativeFunction(
    core.getExportByName('?getField@DB_Noun@@QBE_NHAAH@Z'),
    'bool', ['pointer', 'int', 'pointer'], { abi: 'thiscall' });

rpc.exports = {
  fieldType: function (fieldId) {
    return fieldType(fieldId);
  },
  dump: function (rows, fieldId, asInt) {
    const noun = Memory.alloc(Process.pointerSize);
    const out = Memory.alloc(8);
    return rows.map(function (row) {
      noun.writePointer(ptr(0));
      const found = findNoun(row.hash, noun);
      const self = noun.readPointer();
      if (!found || self.isNull()) {
        return { hash: row.hash, noun: row.noun, found: false, ok: false, value: 0 };
      }
      out.writeU32(0);
      const ok = asInt ? getInt(self, fieldId, out) : getBool(self, fieldId, out);
      return {
        hash: row.hash,
        noun: row.noun,
        found: true,
        ok: ok ? true : false,
        value: asInt ? out.readS32() : out.readU8()
      };
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


def parse_fields(specs: list[str] | None) -> dict[str, int]:
    """Resolve `--field` arguments into an ordered `{name: id}` table."""
    if not specs:
        return {DEFAULT_FIELD: KNOWN_FIELDS[DEFAULT_FIELD]}
    fields: dict[str, int] = {}
    for spec in specs:
        if spec == "granularity":
            for name in GRANULARITY_FIELDS:
                fields[name] = KNOWN_FIELDS[name]
            continue
        name, sep, raw = spec.partition("=")
        name = name.strip()
        if sep:
            fields[name] = int(raw, 0)
        elif name in KNOWN_FIELDS:
            fields[name] = KNOWN_FIELDS[name]
        else:
            known = ", ".join(sorted(KNOWN_FIELDS))
            raise ValueError(f"unknown field {name!r}; use name=id or one of: {known}")
    return fields


def collect_live(
    pid: int, rows: list[dict], fields: dict[str, int]
) -> tuple[pathlib.Path, dict[str, list[dict]], dict[str, int]]:
    session = frida.attach(pid)
    script = session.create_script(AGENT_SOURCE)
    script.on("message", lambda message, data: print("frida", message, file=sys.stderr))
    script.load()
    try:
        print("rpc_exports", script.list_exports_sync(), file=sys.stderr)
        module_info = script.exports_sync.module_info()
        per_field: dict[str, list[dict]] = {}
        field_types: dict[str, int] = {}
        for name, field_id in fields.items():
            kind = script.exports_sync.field_type(field_id)
            if kind not in (FIELD_TYPE_BOOL, FIELD_TYPE_INT):
                raise ValueError(
                    f"core reports fieldType={kind} for {name} ({field_id}); this "
                    "script only knows how to read the bool and int overloads"
                )
            field_types[name] = kind
            as_int = kind == FIELD_TYPE_INT
            results: list[dict] = []
            for offset in range(0, len(rows), 128):
                results.extend(
                    script.exports_sync.dump(
                        rows[offset : offset + 128], field_id, as_int
                    )
                )
            per_field[name] = results
            missing = sum(1 for row in results if not row["found"])
            print(
                f"collected {name} ({field_id}, fieldType={kind}): {len(results)} rows"
                f"{f', {missing} nouns not found' if missing else ''}",
                file=sys.stderr,
            )
    finally:
        session.detach()
    return pathlib.Path(module_info["path"]), per_field, field_types


def build_payload(
    results: list[dict],
    core_path: pathlib.Path,
    nouns_path: pathlib.Path,
    field_id: int = KNOWN_FIELDS[DEFAULT_FIELD],
) -> dict:
    success = [row for row in results if row["ok"]]
    failures = [row for row in results if not row["ok"]]
    non_binary = [row for row in success if row["value"] not in (0, 1)]
    return {
        "schema": 1,
        "source": f"core.dll!DB_Noun::getField({field_id}, &out) via findNoun(noun_hash)",
        "field_id": field_id,
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


SHARED_KEYS = (
    "schema",
    "core_file",
    "core_file_bytes",
    "core_sha256",
    "noun_source",
    "noun_source_bytes",
    "noun_source_sha256",
)


def build_field_block(
    results: list[dict],
    core_path: pathlib.Path,
    nouns_path: pathlib.Path,
    field_id: int,
    field_type: int,
) -> dict:
    """One field's block inside a schema-2 payload.

    Same shape as a schema-1 payload minus the metadata that is now shared, plus
    core's own `fieldType` verdict and the count of nouns `findNoun` could not
    resolve at all. Those two failure modes look identical in `unknown` - a noun
    core has never heard of, and a noun that simply does not carry this field -
    and only the second one is evidence about the field.
    """
    block = build_payload(results, core_path, nouns_path, field_id)
    ordered = {"source": block["source"], "field_id": field_id, "field_type": field_type}
    for key, value in block.items():
        if key in SHARED_KEYS or key in ordered:
            continue
        ordered[key] = value
        if key == "unknown_count":
            ordered["not_found_count"] = sum(1 for row in results if not row["found"])
    return ordered


def build_multi_payload(
    per_field: dict[str, list[dict]],
    fields: dict[str, int],
    field_types: dict[str, int],
    core_path: pathlib.Path,
    nouns_path: pathlib.Path,
) -> dict:
    """Several fields read in one attach, sharing one core.dll identity."""
    return {
        "schema": 2,
        "source": "core.dll!DB_Noun::getField(field_id, &out) via findNoun(noun_hash)",
        "core_file": core_path.name,
        "core_file_bytes": core_path.stat().st_size,
        "core_sha256": sha256(core_path),
        "noun_source": nouns_path.name,
        "noun_source_bytes": nouns_path.stat().st_size,
        "noun_source_sha256": sha256(nouns_path),
        "count": len(next(iter(per_field.values()))) if per_field else 0,
        "fields": {
            name: build_field_block(
                per_field[name], core_path, nouns_path, fields[name], field_types[name]
            )
            for name in fields
        },
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
    parser.add_argument(
        "--field",
        action="append",
        help=(
            "field to read, repeatable: a known name, `name=id`, or `granularity` "
            f"for {'/'.join(GRANULARITY_FIELDS)}. Default: {DEFAULT_FIELD}"
        ),
    )
    args = parser.parse_args()

    fields = parse_fields(args.field)
    out_path = pathlib.Path(args.out).resolve()
    nouns_path = pathlib.Path(args.noun_flags).resolve()
    nouns = json.loads(nouns_path.read_text(encoding="utf-8"))
    only = {name.strip().upper() for name in args.only.split(",")} if args.only else None
    rows = noun_rows(nouns, only)
    core_path, per_field, field_types = collect_live(args.pid, rows, fields)
    if len(fields) == 1:
        name, field_id = next(iter(fields.items()))
        payload = build_payload(per_field[name], core_path, nouns_path, field_id)
        summary = {
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
        }
    else:
        payload = build_multi_payload(
            per_field, fields, field_types, core_path, nouns_path
        )
        summary = {
            "core_sha256": payload["core_sha256"],
            "noun_source_sha256": payload["noun_source_sha256"],
            "count": payload["count"],
            "fields": {
                name: {
                    key: block[key]
                    for key in (
                        "field_id",
                        "field_type",
                        "resolved_count",
                        "unknown_count",
                        "not_found_count",
                        "true_count",
                        "false_count",
                    )
                }
                for name, block in payload["fields"].items()
            },
        }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
