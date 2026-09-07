"""Verify the actual plant-ui backend and export the complete missing FITT list."""
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
import csv
import hashlib
import json
import requests
from cpv_site_audit import element_name, name_key

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'output/cpv-site-audit-20260905'
LIVE = ROOT / 'output/strt-cpv-20260905/live'
BASE = 'http://127.0.0.1:18206'

def records(root):
    r = requests.post(BASE + '/api/v1/model/records', json={'generation_root': root, 'limit': 5000}, timeout=30)
    r.raise_for_status()
    data = r.json()
    assert data['next_cursor'] is None
    return data['items']

ensure = json.loads((OUT/'site-ensure.json').read_text())
with ThreadPoolExecutor(8) as pool:
    pages = list(pool.map(records, ensure['generation_roots']))
items = {item['refno']: item for page in pages for item in page}
rendered = {ref: item for ref,item in items.items() if item['insts']}
assert len(rendered) == 806, len(rendered)
for item in rendered.values():
    for inst in item['insts']:
        assert (LIVE/'meshes'/(inst['geo_hash']+'.mesh')).is_file(), inst['geo_hash']
elements = json.loads((OUT/'generated/elements.json').read_text())
by_ref = {e['refno'].replace('/', '_'): e for e in elements}
missing = json.loads((OUT/'fitt-tree-nodes.json').read_text())
rows = []
for node in missing:
    assert node['refno'] not in rendered, node
    owner = by_ref[node['owner']]
    owner_name = name_key(element_name(owner))
    name = node['name'] if node['name'] and node['name'] != 'FITT' else f"FITT {node['order']+1} of {owner_name}"
    rows.append(dict(refno=node['refno'].replace('_', '/'), name=name,
                     owner=owner_name, status='missing_in_generator_and_runtime'))
assert len(rows) == 72
audit = json.loads((OUT/'audit.json').read_text())
assert {row['name'] for row in rows} == {row['name'] for row in audit['results'] if row['status']=='missing'}
with (OUT/'missing-fitt.csv').open('w', newline='', encoding='utf-8-sig') as file:
    writer = csv.DictWriter(file, fieldnames=list(rows[0]))
    writer.writeheader()
    writer.writerows(rows)
trns = [rendered[f'24381_{ref}'] for ref in [76829,76836]]
for item in trns:
    bounds = item['world_aabb']
    expected = by_ref[item['refno']]
    assert all(abs(a-b)<0.02 for a,b in zip(bounds['mins']+bounds['maxs'],expected['aabb_min']+expected['aabb_max'])), bounds
binary = (OUT/'aios-database.exe').read_bytes()
assert b'catalogue-pml-sqrt-atan-v1' in binary
report = dict(status='incomplete', backend=BASE, backend_sha256=hashlib.sha256(binary).hexdigest(),
              rendered=806, missing_fitt=72, trns=trns,
              screenshots=[str(OUT/f'plant-ui-trns-{ref}.png') for ref in [76829,76836]],
              note='Runtime records/mesh existence checked. UI screenshots inspected separately. Bounds do not prove surface equality.')
assert all(Path(path).is_file() for path in report['screenshots'])
(OUT/'runtime-audit.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
print(json.dumps({k:v for k,v in report.items() if k!='trns'},indent=2))
