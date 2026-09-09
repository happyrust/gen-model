"""Write the current production net-window parser result in matrix format."""

from __future__ import annotations

import json
import sys
from pathlib import Path


REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "python"))

import aios_db


def main() -> int:
    if len(sys.argv) != 5:
        raise SystemExit("usage: net_window.py DB START END OUTPUT")
    database, start, end, output = sys.argv[1:]
    aios_db.set_config(str(REPO / "python/tests/DbOption-ci"))
    result = aios_db.parse.net_window(database, int(start), int(end))
    operations = [
        operation
        for session in result["window"].values()
        for operation in session
    ]
    for operation in operations:
        for source, target in (
            ("added", "added_attrs"),
            ("deleted", "deleted_attrs"),
            ("modified", "modified_attrs"),
        ):
            operation[target] = sorted(operation.get(source, []))
    summary = {
        "requested_start": result["requested_start"],
        "requested_end": result["requested_end"],
        "counts": result["counts"],
        "ops": operations,
        "warnings": result["warnings"],
        "unchanged_rewrites": result["unchanged_rewrites"],
        "unparseable_finals": result["unparseable_finals"],
    }
    Path(output).write_text(
        json.dumps({"summary": summary}, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
