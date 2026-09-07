"""Strict, tool-independent acceptance of incremental and RVM artifacts."""
import argparse
import json
import math
from pathlib import Path


def read_json(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def key(geometry_id):
    if not isinstance(geometry_id, dict) or not geometry_id:
        raise ValueError("missing GeometryId")
    return json.dumps(geometry_id, sort_keys=True, separators=(",", ":"))


def elements_by_id(elements):
    if not isinstance(elements, list):
        raise ValueError("elements artifact must be an array")
    result = {}
    for element in elements:
        identity = key(element["geometry_id"])
        if identity in result:
            raise ValueError(f"duplicate GeometryId: {identity}")
        for field in ("aabb_min", "aabb_max", "world", "volume_mm3", "triangles"):
            if field not in element:
                raise ValueError(f"{identity}: missing {field}")
        result[identity] = element
    return result


def equivalent(left, right):
    if isinstance(left, (int, float)) and isinstance(right, (int, float)):
        return math.isfinite(left) and math.isfinite(right) and math.isclose(
            left, right, rel_tol=1e-9, abs_tol=1e-6)
    if isinstance(left, list) and isinstance(right, list):
        return len(left) == len(right) and all(equivalent(a, b) for a, b in zip(left, right))
    if isinstance(left, dict) and isinstance(right, dict):
        return left.keys() == right.keys() and all(equivalent(left[k], right[k]) for k in left)
    return left == right


def same_geometry(left, right):
    # update_ams does not export gen_ams's local AABB/components/ordinal fields.
    # Compare the shared wire contract, including transform and topology (the
    # older scripts only compared rounded AABB/volume/triangle counts).
    fields = ("geometry_id", "refno", "noun", "aabb_min", "aabb_max", "world",
              "volume_mm3", "triangles", "genus", "negatives_applied", "negatives_skipped")
    return all(equivalent(left.get(k), right.get(k)) for k in fields)


def verify_delta(before, after, upserts, removals):
    before, after, upserts = map(elements_by_id, (before, after, upserts))
    removed = {key(item) for item in removals}
    result = {
        "upsert_absent_at_target": sorted(upserts.keys() - after.keys()),
        "stale_upserts": sorted(k for k in upserts.keys() & after.keys()
                                if not same_geometry(upserts[k], after[k])),
        "missing_removals": sorted(before.keys() - after.keys() - removed),
        "live_removals": sorted(removed & after.keys()),
        "new_not_upserted": sorted(after.keys() - before.keys() - upserts.keys()),
        "changed_not_upserted": sorted(k for k in after.keys() & before.keys() - upserts.keys()
                                       if not same_geometry(before[k], after[k])),
    }
    result["passed"] = not any(result.values())
    result["counts"] = dict(before=len(before), after=len(after), upserts=len(upserts), removals=len(removed))
    return result


def verify_rvm(report):
    required = ("pairs", "only_generated", "only_baseline", "unkeyable_generated", "unkeyable_baseline")
    errors = [f"missing {field}" for field in required if field not in report]
    for field in required[1:]:
        if report.get(field):
            errors.append(field)
    pairs = report.get("pairs", [])
    if not pairs:
        errors.append("no paired geometry")
    for pair in pairs:
        delta = pair.get("max_axis_delta_mm")
        if (pair.get("verdict") not in ("exact", "chord") or
                not isinstance(delta, (int, float)) or not math.isfinite(delta) or delta > 1):
            errors.append(f"bad pair: {pair.get('key', '?')}")
    return {"passed": not errors, "pairs": len(pairs), "errors": errors}


def business_residuals(summary):
    residuals = []
    for op in summary["ops"]:
        if op["noun"] in ("DBMETA", "DBMDAT"):
            continue
        attributes = {a for field in ("added_attrs", "deleted_attrs", "modified_attrs",
            "added_explicit", "deleted_explicit", "modified_explicit") for a in op.get(field, [])}
        if op["op"] != "modified" or op.get("children_changed") or attributes - {"CACHID"}:
            residuals.append(op)
    return residuals


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", action="append", required=True)
    parser.add_argument("--after", action="append", required=True)
    parser.add_argument("--update", required=True, help="update_ams output directory")
    parser.add_argument("--rvm-report", action="append", default=[])
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    try:
        before = [e for p in args.before for e in read_json(p)]
        after = [e for p in args.after for e in read_json(p)]
        update = Path(args.update)
        result = {"delta": verify_delta(before, after, read_json(update / "upserts.json"),
                                         read_json(update / "removals.json")),
                  "rvm": [verify_rvm(read_json(p)) for p in args.rvm_report]}
        report = read_json(update / "update-report.json")
        failures = ("unresolved", "regen_failed", "derived_failed", "derived_stale_unreadable")
        result["generation_errors"] = {f: report[f] for f in failures if report.get(f)}
        result["passed"] = (result["delta"]["passed"] and not result["generation_errors"]
                            and all(r["passed"] for r in result["rvm"]))
        result["rvm_status"] = "checked" if args.rvm_report else "not_run"
    except (OSError, ValueError, KeyError, TypeError) as error:
        result = {"passed": False, "error": str(error)}
    Path(args.out).write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
