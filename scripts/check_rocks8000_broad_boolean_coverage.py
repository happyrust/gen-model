"""Assert stable visible primitives and baked boolean topology for the broad VALV."""

import json
from collections import Counter
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "output" / "rocksdb-8000-20260905" / "equi-increment"
PHASES = ("remaining-broad-created", "remaining-broad-moved-again")
GROUP = "/CODEX_TYPES_REMAIN_BROAD_VALV"


def load(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


detail = (OUT / "remaining-catalogue-detail" / "tty.log").read_text(
    encoding="utf-8-sig"
)
for noun, following in (("SLINE", "SLOO"), ("SCON", "SCTO")):
    section = detail.split(f"DETAIL-{noun}", 1)[1].split(f"DETAIL-{following}", 1)[0]
    assert "Clflag true" in section
    assert "Tuflag false" in section


results = {}
for phase in PHASES:
    folder = OUT / phase
    group = next(item for item in load(folder / "rvm.json") if item["name"] == GROUP)
    kinds = Counter(item["kind"] for item in group["geometries"])
    topology = load(folder / "boolean-topology-audit.json")
    rays = load(folder / "union-ray-audit.json")
    record = next(
        item for item in load(folder / "records.json")["items"]
        if item["refno"] == "24384_26685"
    )

    assert kinds == {"Cylinder": 8, "Snout": 6, "Box": 3, "CircularTorus": 2}
    assert record["has_neg"] is True
    assert topology["topology_mode"] == "closed_component_assembly"
    assert topology["passed"] is True
    assert topology["boundary_edges"] == 0
    assert topology["odd_multiplicity_edges"] == 0
    assert topology["orientation_imbalanced_edges"] == 0
    # RVM stores the 19 positive display primitives. PlantMesh additionally
    # bakes catalogue cutters, so treating those primitives as a plain union
    # must remain diagnostic rather than an acceptance oracle.
    assert rays["rvm_parts"] == 19
    assert rays["rays"] == 600
    assert rays["failures"] == 4
    results[phase] = {
        "rvm_visible_primitives": dict(sorted(kinds.items())),
        "runtime_has_catalogue_negatives": record["has_neg"],
        "closed_component_assembly": True,
        "raw_positive_rvm_union_ray_differences": rays["failures"],
    }

assert results[PHASES[0]]["rvm_visible_primitives"] == results[PHASES[1]]["rvm_visible_primitives"]

print(json.dumps({
    "passed": True,
    "phases": results,
    "interpretation": (
        "SBOX is directly covered by three RVM Box primitives. SLINE and SCON are "
        "centreline-only under their native CLFL=true/TUFL=false flags and are "
        "correctly absent for TUBE ON / CL OFF / OBST OFF. Raw RVM positive "
        "primitives cannot validate the baked catalogue-negative cuts represented "
        "by has_neg."
    ),
}, ensure_ascii=False, indent=2))
