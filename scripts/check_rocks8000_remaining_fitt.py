"""Verify AMS8000 structural FITT data/model increments against native RVM."""

import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / "output" / "rocksdb-8000-20260905" / "equi-increment"
PHASES = {
    "created": ("remaining-fitt-created-fixed", 535, 0.0, False, False),
    "rotated": ("remaining-fitt-rotated-fixed", 536, 15.0, True, True),
    "restored": ("remaining-fitt-restored", 537, 0.0, False, False),
    "cold_restart": ("cold-restart-fitt", 537, 0.0, False, False),
}
TARGET_RECORD = "24384_24948"


def load(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def attribute_map(payload):
    return {item["name"]: item["value"] for item in payload["attributes"]}


def model_item(payload):
    return next(item for item in payload["items"] if item["refno"] == TARGET_RECORD)


def rvm_fitting(payload):
    group = next(item for item in payload if item["name"].endswith("/F1"))
    assert len(group["geometries"]) == 1
    geometry = group["geometries"][0]
    assert geometry["kind"] == "Box"
    return geometry


results = {}
for phase, (folder_name, session, bang, shop, buil) in PHASES.items():
    folder = EVIDENCE / folder_name
    attrs_payload = load(folder / "attributes.json")
    attrs = attribute_map(attrs_payload)
    database = load(folder / "database.json")
    stored = database[0][0]
    watermark = database[1][0]
    records = load(folder / "sctn-records.json")
    item = model_item(records)
    rvm = rvm_fitting(load(folder / "rvm.json"))

    actual_bbox = item["world_aabb"]["mins"] + item["world_aabb"]["maxs"]
    expected_bbox = rvm["bbox_world_mm"]
    errors = [abs(actual - expected) for actual, expected in zip(actual_bbox, expected_bbox)]
    max_error = max(errors)

    assert attrs_payload["sesno"] == session
    assert (attrs["BANG"], attrs["SHOP"], attrs["BUIL"]) == (bang, shop, buil)
    assert (stored["BANG"], stored["SHOP"], stored["BUIL"]) == (bang, shop, buil)
    assert watermark["applied_sesno"] == session
    assert next(v["sesno"] for v in records["session_vector"] if v["dbnum"] == 8000) == session
    assert records["total"] == 1
    assert max_error <= 0.01, (phase, errors)

    results[phase] = {
        "session": session,
        "attributes": {"BANG": bang, "SHOP": shop, "BUIL": buil},
        "geo_hash": item["insts"][0]["geo_hash"],
        "runtime_bbox_mm": actual_bbox,
        "rvm_bbox_mm": expected_bbox,
        "max_aabb_error_mm": max_error,
    }

assert results["created"]["geo_hash"] != results["rotated"]["geo_hash"]
assert results["created"]["geo_hash"] == results["restored"]["geo_hash"]
assert results["restored"]["geo_hash"] == results["cold_restart"]["geo_hash"]

summary = {
    "ok": True,
    "target": "24384/24949",
    "published_as": "SCTN 24384/24948",
    "phases": results,
    "hash_changed_on_rotate": True,
    "hash_restored_exactly": True,
    "hash_persisted_after_cold_restart": True,
}
print(json.dumps(summary, ensure_ascii=False, indent=2))
