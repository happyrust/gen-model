"""Compare published model record identities with the offline audit."""
import json
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
import requests
import sys

out = Path(__file__).resolve().parents[1] / 'output/hvac-sites-audit-20260905'
folder = out / sys.argv[1].replace('/', '_')
ensure = json.loads((folder / 'runtime-ensure-current.json').read_text())

def read(root):
    with requests.Session() as http:
        response = http.post('http://127.0.0.1:18206/api/v1/model/records',
                             json={'generation_root': root, 'limit': 5000}, timeout=60)
        response.raise_for_status()
        data = response.json()
        if data.get('truncated'):
            raise RuntimeError(f'Pagination needed for {root}')
        return data

with ThreadPoolExecutor(4) as pool:
    pages = list(pool.map(read, ensure['generation_roots']))
(folder / 'runtime-records-current.json').write_text(json.dumps(pages, indent=2))
items = [item for page in pages for item in page['items']]
unique_items = {json.dumps(item, sort_keys=True): item for item in items}
items = list(unique_items.values())
live = {item['refno'].replace('_', '/') for item in items}
offline_elements = json.loads((folder / 'generated/elements.json').read_text())
offline = {item['refno'] for item in offline_elements}
report = dict(records=len(items), unique_records=len(live), offline_records=len(offline),
              extra=sorted(live-offline), missing=sorted(offline-live))
report['extra_records'] = [dict(refno=item['refno'].replace('_', '/'), generic=item.get('generic'),
                              is_tubi=any(i.get('is_tubi') for i in item['insts']))
                           for item in items if item['refno'].replace('_', '/') not in offline]
comparison = []
for element in offline_elements:
    matches = [item['world_aabb'] for item in items
               if item['refno'].replace('_', '/') == element['refno'] and item.get('world_aabb')]
    if not matches:
        continue
    actual = [min(b['mins'][i] for b in matches) for i in range(3)]
    actual += [max(b['maxs'][i] for b in matches) for i in range(3)]
    expected = element['aabb_min'] + element['aabb_max']
    comparison.append(dict(refno=element['refno'], error_mm=max(abs(a-b) for a,b in zip(actual,expected))))
report['bounds_compared'] = len(comparison)
report['bounds_max_error_mm'] = max((x['error_mm'] for x in comparison),default=0)
report['bounds_differences_over_0_05mm'] = [x for x in comparison if x['error_mm'] > 0.05]
(folder / 'runtime-identities-current.json').write_text(json.dumps(report, indent=2))
print(json.dumps(report))
