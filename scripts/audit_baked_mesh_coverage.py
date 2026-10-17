#!/usr/bin/env python3
"""Audit baked-mesh coverage: every boole'd inst_relate row references
`<booled_id>.mesh` under the mesh directory; a missing file makes the
whole durable root fail room-membership recompute (issue #56).

Run on the node that owns the SurrealDB + mesh dir (or anywhere that can
reach both):

    SURREAL_HTTP=http://127.0.0.1:8009 SURREAL_NS=1516 \
    SURREAL_DB=AvevaMarineSample SURREAL_USER=... SURREAL_PASS=... \
    MESH_DIR=/var/lib/aios-database/assets/meshes \
    python3 scripts/audit_baked_mesh_coverage.py [missing-list.txt]

Exit 0 when coverage is complete, 1 when ids are missing, 2 on query
failure. Credentials never default — pass them via the environment.
"""
import base64
import json
import os
import sys
import urllib.request

SURREAL_HTTP = os.environ.get("SURREAL_HTTP", "http://127.0.0.1:8009")
SURREAL_NS = os.environ.get("SURREAL_NS", "1516")
SURREAL_DB = os.environ.get("SURREAL_DB", "AvevaMarineSample")
SURREAL_USER = os.environ.get("SURREAL_USER")
SURREAL_PASS = os.environ.get("SURREAL_PASS")
MESH_DIR = os.environ.get("MESH_DIR", "/var/lib/aios-database/assets/meshes")


def sql(q):
    req = urllib.request.Request(
        f"{SURREAL_HTTP}/sql",
        data=q.encode(),
        headers={
            "Accept": "application/json",
            "Content-Type": "text/plain",
            "surreal-ns": SURREAL_NS,
            "surreal-db": SURREAL_DB,
        },
        method="POST",
    )
    if SURREAL_USER is not None:
        token = base64.b64encode(f"{SURREAL_USER}:{SURREAL_PASS or ''}".encode())
        req.add_header("Authorization", f"Basic {token.decode()}")
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.loads(r.read())


def main():
    try:
        rows = sql(
            # `booled` marks instances swallowed by a boolean op; their only
            # geometry is the baked mesh file (see room_model.rs live query).
            "select <string>booled_id as bid, count() as c "
            "from inst_relate where boole" "d = true group by bid"
        )[0]["result"]
    except Exception as exc:  # noqa: BLE001 - ops script, report and bail
        print(f"query failed: {exc}", file=sys.stderr)
        return 2

    missing = [r["bid"] for r in rows if not os.path.exists(
        os.path.join(MESH_DIR, f"{r['bid']}.mesh"))]
    print(f"groups: {len(rows)}  missing: {len(missing)}  mesh_dir: {MESH_DIR}")

    out = sys.argv[1] if len(sys.argv) > 1 else None
    if missing and out:
        with open(out, "w") as f:
            for bid in missing:
                f.write(f"{bid}\n")
        print(f"wrote {out}")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
