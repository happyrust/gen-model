# -*- coding: utf-8 -*-
"""Instance-side probe for the P1 fix (728a7123): how many reverse-reference
edges did widening `reference_edge_eligible` actually add on real data?

`audit_ref_gap_probe.py` answers the static half - which ELEMENT-typed
attributes sit outside the curated CASCADE list and therefore built no
`ref_rev` edge before the fix. This probe answers the live half: for each of
those attributes, how many stored elements actually carry a non-null
reference, and on which nouns. Read-only: it issues nothing but SELECT count().

Usage (defaults come from DbOption.toml):
    python output/ref_edge_delta_probe.py
    python output/ref_edge_delta_probe.py --url http://127.0.0.1:8009 --db AvevaMarineSample
"""
import argparse
import base64
import json
import re
import sys
import urllib.error
import urllib.request

sys.stdout.reconfigure(encoding="utf-8")


def curated_table(src: str, name: str) -> set:
    m = re.search(r"pub const " + name + r": &\[&str\] = &\[(.*?)\];", src, re.S)
    body = re.sub(r"//[^\n]*", "", m.group(1))
    return set(re.findall(r'"([A-Z0-9:]+)"', body))


def db_option(key: str, default: str) -> str:
    try:
        text = open("DbOption.toml", encoding="utf-8").read()
    except OSError:
        return default
    m = re.search(rf"^{key}\s*=\s*\"?([^\"\n#]+)\"?", text, re.M)
    return m.group(1).strip() if m else default


class Surreal:
    def __init__(self, url: str, ns: str, db: str, user: str, pw: str):
        self.url = url.rstrip("/") + "/sql"
        token = base64.b64encode(f"{user}:{pw}".encode()).decode()
        self.headers = {
            "Authorization": f"Basic {token}",
            "Accept": "application/json",
            "surreal-ns": ns,
            "surreal-db": db,
            "Content-Type": "text/plain",
        }

    def query(self, sql: str):
        req = urllib.request.Request(
            self.url, data=sql.encode("utf-8"), headers=self.headers, method="POST"
        )
        with urllib.request.urlopen(req, timeout=300) as resp:
            return json.loads(resp.read().decode("utf-8"))

    def count(self, sql: str):
        """Return (count, error). A missing table is reported, not raised."""
        try:
            out = self.query(sql)[0]
        except (urllib.error.URLError, OSError) as exc:
            return None, str(exc)
        if out.get("status") != "OK":
            return None, str(out.get("result"))
        rows = out.get("result") or []
        return (rows[0].get("count", 0) if rows else 0), None


ap = argparse.ArgumentParser()
ap.add_argument("--url", default=f"http://127.0.0.1:{db_option('v_port', '8009')}")
ap.add_argument("--ns", default=db_option("surreal_ns", "1516"))
ap.add_argument("--db", default=db_option("project_name", "AvevaMarineSample"))
ap.add_argument("--user", default=db_option("v_user", "root"))
ap.add_argument("--pw", default=db_option("v_password", "root"))
args = ap.parse_args()

# ---- static half: which attributes the fix newly made edge-eligible ---------
src = open("src/data_interface/model_impact.rs", encoding="utf-8").read()
non_cascade = set()
for tname in [
    "DATA_ONLY_ATTR_NAMES",
    "STRUCTURAL_ATTR_NAMES",
    "TRANSFORM_ONLY_ATTR_NAMES",
    "DIRECT_GEOMETRY_ATTR_NAMES",
]:
    non_cascade |= curated_table(src, tname)

schema = json.load(open("all_attr_info.json", encoding="utf-8"))["named_attr_info_map"]
hosts = {}  # attr name -> set of nouns declaring it as ELEMENT
for noun, attrs in schema.items():
    for rec in attrs.values():
        name = (rec.get("name") or "").strip().upper()
        if name and rec.get("att_type") == "ELEMENT":
            hosts.setdefault(name, set()).add(noun.strip().upper())

# Only references curated into a NON-cascade table gained edges. An ELEMENT
# reference absent from every table already classified as Unknown and was
# upgraded to DependencyCascade by the A2 metadata rule, so the old admission
# reversed it too; OWNER stays out by design (the ownership graph carries it).
newly_eligible = sorted((set(hosts) & non_cascade) - {"OWNER"})

print("== attributes the P1 fix newly made edge-eligible ==")
for name in newly_eligible:
    print(f"  {name}: declared by {len(hosts[name])} noun(s)")
if not newly_eligible:
    print("  (none - every ELEMENT reference already built edges)")

# ---- live half: how many of those references actually exist -----------------
sur = Surreal(args.url, args.ns, args.db, args.user, args.pw)
print(f"\n== live counts on {args.url} ns={args.ns} db={args.db} ==")
tables, err = None, None
try:
    out = sur.query("INFO FOR DB;")[0]
    tables = (out.get("result") or {}).get("tables") or {}
except (urllib.error.URLError, OSError) as exc:
    err = str(exc)
if tables is None:
    print(f"  unreachable: {err}")
    raise SystemExit(1)

present = {t.upper() for t in tables}
print(f"  {len(present)} table(s) in database")

total = 0
for name in newly_eligible:
    targets = sorted(hosts[name] & present)
    if not targets:
        print(f"  {name}: no host noun stored here")
        continue
    per_noun, attr_total = [], 0
    for noun in targets:
        # A null reference is stored as `<table>:0_0` and builds no edge.
        sql = (
            f"SELECT count() FROM {noun} WHERE {name} != NONE "
            f"AND string::ends_with(<string>{name}, ':0_0') = false GROUP ALL;"
        )
        n, qerr = sur.count(sql)
        if qerr:
            per_noun.append(f"{noun}=ERR({qerr})")
        elif n:
            per_noun.append(f"{noun}={n}")
            attr_total += n
    total += attr_total
    detail = ", ".join(per_noun) if per_noun else "present but all null/empty"
    print(f"  {name}: {attr_total} edge(s)  [{detail}]")

print(f"\n  reverse edges the fix adds on this database: {total}")
if total == 0:
    print(
        "  (zero here means this dataset carries none of those references, "
        "not that the fix is inert - re-run against a full design database)"
    )
