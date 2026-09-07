"""Generate the affected branches on plant-ui's backend and verify live bounds."""
from pathlib import Path
import csv
import json
import requests

root = Path(__file__).resolve().parents[1]
baseline = root/'output/hvac-sites-audit-20260905'
out = root/'output/hvac-flex-gril-fix-20260905'
affected = {r['refno'] for r in csv.DictReader((baseline/'vent-defects.csv').open(encoding='utf-8-sig'))}
nodes = {}
expected = {}
for target in json.loads((baseline/'targets.json').read_text(encoding='utf-8')):
    scope = target['refno'].replace('/','_')
    for node in json.loads((baseline/scope/'tree-nodes.json').read_text()):
        nodes[node['refno'].replace('_','/')] = node
    for item in json.loads((out/scope/'audit.json').read_text())['results']:
        if item.get('refno') in affected:
            assert item['status']=='bounds_pass', item
    # The native audit compares in the element's local frame. The API returns
    # world bounds, so use the already-verified offline world bounds here.
    for item in json.loads((out/scope/'generated/elements.json').read_text()):
        if item['refno'] in affected:
            expected[item['refno']] = item['aabb_min'] + item['aabb_max']
branches = sorted({nodes[ref]['owner'].replace('_','/') for ref in affected})
http = requests.Session()
results = []
for branch in branches:
    print('Generate branch',branch,flush=True)
    response = http.post('http://127.0.0.1:18206/api/v1/model/ensure',json={'refno':branch},timeout=150)
    (out/f'runtime-{branch.replace("/","_")}-ensure.json').write_text(response.text)
    response.raise_for_status()
    response = http.post('http://127.0.0.1:18206/api/v1/model/records',json={'generation_root':branch,'limit':5000},timeout=60)
    response.raise_for_status()
    page=response.json()
    assert not page.get('truncated')
    (out/f'runtime-{branch.replace("/","_")}-records.json').write_text(json.dumps(page,indent=2))
    for item in page['items']:
        ref=item['refno'].replace('_','/')
        if ref not in affected: continue
        bounds=item['world_aabb']['mins']+item['world_aabb']['maxs']
        error=max(abs(a-b) for a,b in zip(bounds,expected[ref]))
        results.append(dict(refno=ref,noun=nodes[ref]['noun'],error_mm=error,instance_parts=len(item['insts']),passed=error<0.05))
        print(results[-1],flush=True)
(out/'runtime-focused.json').write_text(json.dumps(results,indent=2))
assert {r['refno'] for r in results}==affected
assert all(r['passed'] for r in results)
